//! Expression-level checks: nullable-receiver detection, private-member
//! access, native-call argument checking, lightweight type inference, and
//! the `?? != nil` style flow narrowing.

use saule_ast::{BinOp, CallArg, Expr, LambdaBody, PipeStage, Spanned, TableEntry, Type};

use super::TypeCheckError;
use super::funcs;
use super::matches::check_match;
use super::state::{
    Scope, class_implements, current_class, is_interface, is_subtype_named, is_type_param,
    lookup_member, with_classes, with_enums,
};
use super::stmt::{check_stmt, seed_params};
use super::to_source_span;

// ──────────────────────────────────────────────────────────────────────────────
// Expression checker — walks expressions looking for `obj.member` /
// `obj.method(...)` where `obj` has a statically-known nullable type.
// ──────────────────────────────────────────────────────────────────────────────

pub(super) fn check_expr(expr: &Spanned<Expr>, scope: &Scope, errors: &mut Vec<TypeCheckError>) {
    match &expr.value {
        Expr::Member { obj, name } => {
            check_expr(obj, scope, errors);
            report_if_nullable_receiver(obj, name, scope, errors);
            report_if_private(obj, name, scope, errors);
            report_if_unknown_member(obj, name, scope, errors);
        }
        Expr::MethodCall { obj, method, args } => {
            check_expr(obj, scope, errors);
            report_if_nullable_receiver(obj, method, scope, errors);
            report_if_private(obj, method, scope, errors);
            report_if_unknown_member(obj, method, scope, errors);
            for a in args {
                check_arg(a, scope, errors);
            }
            // Validate positional argument types against the user method sig.
            if let Some(ty) = infer(obj, scope)
                && let Type::Named(class_name) = strip_nullable(ty)
                && let Some(sig) = saule_semantic::lookup_method(&class_name, method)
            {
                check_user_method_args(
                    &format!("{class_name}.{method}"),
                    &sig,
                    args,
                    scope,
                    errors,
                    expr.span.clone(),
                );
            }
        }
        Expr::Call { callee, args } => {
            // `obj.method(args)` is parsed as Call(Member { obj, name }, args)
            // — same nullable-receiver rule applies.
            if let Expr::Member { obj, name } = &callee.value {
                check_expr(obj, scope, errors);
                report_if_nullable_receiver(obj, name, scope, errors);
                report_if_private(obj, name, scope, errors);
                report_if_unknown_member(obj, name, scope, errors);
                report_if_enum_variant_arity(obj, name, args, errors, expr.span.clone());
            } else {
                check_expr(callee, scope, errors);
                report_if_user_function_arity(callee, args, scope, errors, expr.span.clone());
            }
            for a in args {
                check_arg(a, scope, errors);
            }
            // Constructor call: `ClassName(args)` dispatches to `init`.
            // Validate args against the class's `init` signature so that
            // bogus extras (`Entry(item, dueDate)` against `fn init(todo)`)
            // and unknown named params get caught at typeck time.
            if let Expr::Ident(class_name) = &callee.value
                && with_classes(|r| r.contains_key(class_name))
                && let Some(sig) = saule_semantic::lookup_method(class_name, "init")
            {
                check_user_method_args(
                    &format!("{class_name}.init"),
                    &sig,
                    args,
                    scope,
                    errors,
                    expr.span.clone(),
                );
            }
            // If the callee resolves to a known native signature, check the
            // argument types positionally. Named arguments are skipped (those
            // aren't supported on natives anyway, and they error at runtime).
            if let Some(qname) = native_callee_name(callee, scope)
                && let Some(sig) = crate::sigs::lookup(&qname)
            {
                check_native_args(&qname, &sig, args, scope, errors, expr.span.clone());
            }
            // User-defined class methods: `Class.method(args)` (static) or
            // `instance.method(args)` (instance). The native-sig path above
            // never matches these because they aren't registered as natives.
            if let Expr::Member { obj, name } = &callee.value {
                let class_name = match &obj.value {
                    Expr::Ident(n) if with_classes(|r| r.contains_key(n)) => Some(n.clone()),
                    Expr::Self_ => current_class(),
                    _ => infer(obj, scope).and_then(|t| match strip_nullable(t) {
                        Type::Named(n) if with_classes(|r| r.contains_key(&n)) => Some(n),
                        _ => None,
                    }),
                };
                if let Some(class_name) = class_name
                    && let Some(sig) = saule_semantic::lookup_method(&class_name, name)
                {
                    check_user_method_args(
                        &format!("{class_name}.{name}"),
                        &sig,
                        args,
                        scope,
                        errors,
                        expr.span.clone(),
                    );
                }
            }
        }
        Expr::SafeMember { obj, .. } => check_expr(obj, scope, errors),
        Expr::Index { obj, index } => {
            check_expr(obj, scope, errors);
            check_expr(index, scope, errors);
        }
        Expr::Unary { rhs, .. } => check_expr(rhs, scope, errors),
        Expr::Binary { op, lhs, rhs } => {
            check_expr(lhs, scope, errors);
            check_expr(rhs, scope, errors);
            check_binary_op(*op, lhs, rhs, scope, errors);
        }
        Expr::ForceUnwrap(inner) => check_expr(inner, scope, errors),
        Expr::Table(items) => {
            for entry in items {
                match entry {
                    TableEntry::Positional(e) => check_expr(e, scope, errors),
                    TableEntry::Field { key, value } => {
                        check_expr(key, scope, errors);
                        check_expr(value, scope, errors);
                    }
                }
            }
        }
        Expr::Lambda { params, body, .. } => {
            for p in params {
                super::stmt::reject_nil_in_binding_type(&p.ty, p.span.clone(), errors);
            }
            let mut lscope = scope.clone();
            seed_params(&mut lscope, params);
            match body {
                LambdaBody::Expr(e) => check_expr(e, &lscope, errors),
                LambdaBody::Block(stmts) => {
                    for s in stmts {
                        check_stmt(s, &mut lscope, errors);
                    }
                }
            }
        }
        Expr::Match { scrutinee, arms } => {
            check_match(expr, scrutinee, arms, scope, errors);
        }
        Expr::Pipe { source, stages } => {
            check_pipe(source, stages, scope, errors);
        }
        _ => {}
    }
}

pub(super) fn check_arg(arg: &CallArg, scope: &Scope, errors: &mut Vec<TypeCheckError>) {
    match arg {
        CallArg::Positional(e) | CallArg::Named { value: e, .. } => check_expr(e, scope, errors),
    }
}

/// Check positional arguments of a native call against the registered
/// signature. Skips named arguments (natives don't support them; the runtime
/// will surface that as an error).
///
/// Heuristics intentionally lenient:
///   * `any` and `T?` parameters accept anything (incl. nil) — they're the
///     "I'll figure it out" slots.
///   * Variadic / over-supplied calls aren't penalised when the declared
///     param list runs out — many natives accept `...rest` (variadic) which
///     isn't expressed in the sig yet.
///   * If `infer` can't produce a type for the argument, we skip silently.
pub(super) fn check_native_args(
    callee: &str,
    sig: &crate::sigs::NativeSig,
    args: &[CallArg],
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
    call_span: std::ops::Range<usize>,
) {
    // Count required positional params. A parameter is *optional* only
    // when its declared type is nullable (`T?`); `any` slots are still
    // required — they just accept any value. Natives that genuinely
    // want an optional `any` should register `Nullable(any)` for that
    // slot (e.g. `Os.exit`).
    let required: usize = sig
        .params
        .iter()
        .take_while(|p| !is_nullable(p))
        .count();
    let positional: Vec<&CallArg> = args
        .iter()
        .filter(|a| matches!(a, CallArg::Positional(_)))
        .collect();
    if positional.len() < required {
        errors.push(TypeCheckError::NativeArity {
            callee: callee.to_string(),
            expected: required,
            found: positional.len(),
            span: to_source_span(call_span.clone()),
        });
        return;
    }

    // Reject extras when the native is not variadic. Don't bail though —
    // continue checking the known positions for type mismatches.
    if sig.variadic.is_none() && positional.len() > sig.params.len() {
        errors.push(TypeCheckError::NativeArity {
            callee: callee.to_string(),
            expected: sig.params.len(),
            found: positional.len(),
            span: to_source_span(call_span),
        });
    }

    // Build a substitution from this signature's type params (e.g. `V`) to
    // concrete types learned from the actual arguments. Walking left-to-right
    // means earlier args (the table, typically) seed the variable, and later
    // args (the element to insert) get checked against the bound type.
    let mut subst: std::collections::HashMap<String, Type> =
        std::collections::HashMap::new();

    for (i, arg) in args.iter().enumerate() {
        // Pick the expected type for slot `i`:
        //   - within declared params: use `params[i]`
        //   - past the end: use the variadic element type (or stop if absent)
        let expected_raw = match sig.params.get(i) {
            Some(t) => t,
            None => match &sig.variadic {
                Some(t) => t,
                None => break,
            },
        };
        let value_expr = match arg {
            CallArg::Positional(e) => e,
            CallArg::Named { .. } => continue,
        };
        // Substitute any already-bound type params before checking.
        let expected = substitute(expected_raw, &subst, &sig.type_params);
        let Some(found_ty) = infer(value_expr, scope) else {
            // Even without an inferred type, try to refine the substitution
            // from sibling args downstream — but we have nothing to do here.
            continue;
        };
        // Refine the substitution from this arg/expected pair before the
        // compatibility check, so a generic slot that's still free becomes
        // bound rather than rejected.
        unify(&expected, &found_ty, &sig.type_params, &mut subst);
        let expected = substitute(&expected, &subst, &sig.type_params);
        if is_any(&expected) {
            continue;
        }
        // See `check_user_method_args` for why we reject nullability here
        // even though `types_compatible` would accept it. Skip when the
        // expected type is still a free type parameter — `V` is allowed
        // to bind to `any?` so the nullable arg is legitimate.
        if !is_unbound_type_param(&expected, &sig.type_params)
            && !is_nullable(&expected)
            && is_nullable(&found_ty)
        {
            errors.push(TypeCheckError::NativeArgTypeMismatch {
                callee: callee.to_string(),
                arg: i + 1,
                expected: type_to_string(&expected),
                found: type_to_string(&found_ty),
                span: to_source_span(value_expr.span.clone()),
            });
            continue;
        }
        if !types_compatible(&expected, &found_ty) {
            errors.push(TypeCheckError::NativeArgTypeMismatch {
                callee: callee.to_string(),
                arg: i + 1,
                expected: type_to_string(&expected),
                found: type_to_string(&found_ty),
                span: to_source_span(value_expr.span.clone()),
            });
        }
    }
}

/// Check positional arguments of a user-defined class method against its
/// declared parameter types. Mirrors [`check_native_args`] but reads
/// `Param` from the semantic registry. Generic methods (non-empty
/// `sig.type_params`, e.g. a native package's `find<T>`) have their type
/// parameters bound from the actual arguments and substituted before the
/// compatibility check.
///
/// Lenient in the same ways: `any` slots accept anything, nullable slots
/// accept anything, named args are skipped, and we bail silently when
/// `infer` can't produce a type.
fn check_user_method_args(
    callee_display: &str,
    sig: &saule_semantic::MethodSig,
    args: &[CallArg],
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
    call_span: std::ops::Range<usize>,
) {
    // Required: leading non-variadic, non-defaulted, non-nullable params.
    let required: usize = sig
        .params
        .iter()
        .take_while(|p| !p.variadic && p.default.is_none() && !is_nullable(&p.ty))
        .count();
    let positional: Vec<&CallArg> = args
        .iter()
        .filter(|a| matches!(a, CallArg::Positional(_)))
        .collect();
    let has_variadic = sig.params.last().is_some_and(|p| p.variadic);
    if positional.len() < required {
        errors.push(TypeCheckError::NativeArity {
            callee: callee_display.to_string(),
            expected: required,
            found: positional.len(),
            span: to_source_span(call_span.clone()),
        });
        return;
    }
    if !has_variadic && positional.len() > sig.params.len() {
        errors.push(TypeCheckError::NativeArity {
            callee: callee_display.to_string(),
            expected: sig.params.len(),
            found: positional.len(),
            span: to_source_span(call_span),
        });
    }

    // Reject named args whose name doesn't match any declared parameter.
    // Without this the call silently drops the arg at runtime, which is
    // a footgun (`obj.add(x, dueDate: y)` against `fn add(x)` looked OK).
    for arg in args {
        if let CallArg::Named { name, value } = arg
            && !sig.params.iter().any(|p| &p.name == name)
        {
            errors.push(TypeCheckError::UnknownNamedArg {
                callee: callee_display.to_string(),
                name: name.clone(),
                span: to_source_span(value.span.clone()),
            });
        }
    }

    // Bind the method's generic type parameters (e.g. `<T, U>`) from the
    // actual arguments left-to-right, then check each slot against the
    // substituted expected type. Non-generic methods skip the unify step
    // and behave like before.
    let type_params = &sig.type_params;
    let mut subst: std::collections::HashMap<String, Type> =
        std::collections::HashMap::new();

    for (i, arg) in args.iter().enumerate() {
        let Some(p) = sig.params.get(i).or_else(|| {
            if has_variadic {
                sig.params.last()
            } else {
                None
            }
        }) else {
            break;
        };
        if p.variadic {
            continue;
        }
        let value_expr = match arg {
            CallArg::Positional(e) => e,
            CallArg::Named { .. } => continue,
        };
        let expected = if type_params.is_empty() {
            p.ty.clone()
        } else {
            substitute(&p.ty, &subst, type_params)
        };
        if is_any(&expected) {
            continue;
        }
        let Some(found_ty) = infer(value_expr, scope) else {
            continue;
        };
        if !type_params.is_empty() {
            unify(&expected, &found_ty, type_params, &mut subst);
        }
        let expected = if type_params.is_empty() {
            expected
        } else {
            substitute(&expected, &subst, type_params)
        };
        if is_any(&expected) {
            continue;
        }
        // Pass-a-nullable-into-a-non-nullable-slot is rejected even when
        // the stripped bases match. `types_compatible` deliberately
        // strips `Nullable` on both sides (it's the structural compat
        // predicate), so the nullability check has to live here. Skip
        // when the expected slot is still a free generic parameter —
        // it can legitimately bind to a nullable type.
        if !is_unbound_type_param(&expected, type_params)
            && !is_nullable(&expected)
            && is_nullable(&found_ty)
        {
            errors.push(TypeCheckError::NativeArgTypeMismatch {
                callee: callee_display.to_string(),
                arg: i + 1,
                expected: type_to_string(&expected),
                found: type_to_string(&found_ty),
                span: to_source_span(value_expr.span.clone()),
            });
            continue;
        }
        if !types_compatible(&expected, &found_ty) {
            errors.push(TypeCheckError::NativeArgTypeMismatch {
                callee: callee_display.to_string(),
                arg: i + 1,
                expected: type_to_string(&expected),
                found: type_to_string(&found_ty),
                span: to_source_span(value_expr.span.clone()),
            });
        }
    }
}

/// True when `ty` is a `Named(n)` where `n` is one of the (still-unbound)
/// type parameters from the surrounding signature. Such a slot can bind
/// to any concrete type — including nullable ones — so the targeted
/// nullable-into-non-nullable rejection should not fire for it.
fn is_unbound_type_param(ty: &Type, params: &[String]) -> bool {
    matches!(ty, Type::Named(n) if params.iter().any(|p| p == n))
}

/// Substitute bound type variables in `ty` with their concrete types from
/// `subst`. Unbound variables (and non-parameter names) are returned as-is.
pub(super) fn substitute(
    ty: &Type,
    subst: &std::collections::HashMap<String, Type>,
    params: &[String],
) -> Type {
    match ty {
        Type::Named(n) if params.iter().any(|p| p == n) => {
            subst.get(n).cloned().unwrap_or_else(|| ty.clone())
        }
        Type::Named(_) => ty.clone(),
        Type::Nullable(inner) => Type::Nullable(Box::new(substitute(inner, subst, params))),
        Type::Table { key, value } => Type::Table {
            key: key.as_ref().map(|k| Box::new(substitute(k, subst, params))),
            value: Box::new(substitute(value, subst, params)),
        },
        Type::Tuple(items) => Type::Tuple(items.iter().map(|t| substitute(t, subst, params)).collect()),
        Type::Function { params: ps, ret } => Type::Function {
            params: ps.iter().map(|t| substitute(t, subst, params)).collect(),
            ret: Box::new(substitute(ret, subst, params)),
        },
    }
}

/// One-way unification: bind type-param names in `expected` to corresponding
/// concrete shapes from `found`. Conservative — if shapes don't line up,
/// silently skip (the regular compatibility check will surface the mismatch).
pub(super) fn unify(
    expected: &Type,
    found: &Type,
    params: &[String],
    subst: &mut std::collections::HashMap<String, Type>,
) {
    // A free type-param on the expected side binds to whatever the actual
    // argument's type is. Strip a leading `Nullable` from `found` so
    // `table<V>` against `table<Entry>?` still binds `V := Entry`.
    if let Type::Named(n) = expected
        && params.iter().any(|p| p == n)
        && !subst.contains_key(n)
    {
        let bound = match found {
            Type::Nullable(inner) => (**inner).clone(),
            other => other.clone(),
        };
        // Don't bind `V := any` — that would erase the constraint for the
        // remaining args. Leave it unbound so later args can refine it.
        if !is_any(&bound) {
            subst.insert(n.clone(), bound);
        }
        return;
    }
    match (expected, found) {
        (Type::Nullable(e_inner), Type::Nullable(f_inner)) => {
            unify(e_inner, f_inner, params, subst);
        }
        (Type::Nullable(e_inner), other) => unify(e_inner, other, params, subst),
        (
            Type::Table {
                value: e_val,
                key: e_key,
            },
            Type::Table {
                value: f_val,
                key: f_key,
            },
        ) => {
            unify(e_val, f_val, params, subst);
            if let (Some(ek), Some(fk)) = (e_key, f_key) {
                unify(ek, fk, params, subst);
            }
        }
        (Type::Tuple(es), Type::Tuple(fs)) if es.len() == fs.len() => {
            for (e, f) in es.iter().zip(fs.iter()) {
                unify(e, f, params, subst);
            }
        }
        (
            Type::Function {
                params: ep,
                ret: er,
            },
            Type::Function {
                params: fp,
                ret: fr,
            },
        ) if ep.len() == fp.len() => {
            for (e, f) in ep.iter().zip(fp.iter()) {
                unify(e, f, params, subst);
            }
            unify(er, fr, params, subst);
        }
        _ => {}
    }
}

pub(super) fn is_any(t: &Type) -> bool {
    matches!(t, Type::Named(n) if n == "any")
}

/// Resolve a semantic method's return type, substituting any generic type
/// parameters bound from the call's actual positional arguments. Non-generic
/// methods return their declared `return_ty` unchanged.
fn semantic_method_return(
    sig: &saule_semantic::MethodSig,
    args: &[CallArg],
    scope: &Scope,
) -> Option<Type> {
    let ret = sig.return_ty.clone()?;
    if sig.type_params.is_empty() {
        return Some(ret);
    }
    let mut subst: std::collections::HashMap<String, Type> = std::collections::HashMap::new();
    let positional = args.iter().filter_map(|a| match a {
        CallArg::Positional(e) => Some(e),
        CallArg::Named { .. } => None,
    });
    for (p, arg_expr) in sig.params.iter().zip(positional) {
        if let Some(found_ty) = infer(arg_expr, scope) {
            unify(&p.ty, &found_ty, &sig.type_params, &mut subst);
        }
    }
    Some(substitute(&ret, &subst, &sig.type_params))
}

fn report_if_nullable_receiver(
    obj: &Spanned<Expr>,
    member: &str,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    if let Some(ty) = infer(obj, scope)
        && is_nullable(&ty)
    {
        errors.push(TypeCheckError::NullableMemberAccess {
            ty: type_to_string(&ty),
            member: member.to_string(),
            span: to_source_span(obj.span.clone()),
        });
    }
}

/// Reject access to `local` (private) members from outside the owning class.
/// `self.foo` is allowed only when the *owning* class of `foo` is the class
/// currently being checked — a private field inherited from a parent is
/// **not** visible to the child.
fn report_if_private(
    obj: &Spanned<Expr>,
    member: &str,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    // Resolve the class name we're reading the member off:
    //   * `self.member` → the current class (lookup walks the parent chain).
    //   * `obj.member` where `obj` is an *instance* → infer the receiver type.
    //   * `Class.member` where the receiver is the class itself
    //     (e.g. `Bank.secret`) → use the ident directly.
    let class_name = match &obj.value {
        Expr::Self_ => match current_class() {
            Some(n) => n,
            None => return,
        },
        Expr::Ident(n) if with_classes(|reg| reg.contains_key(n)) => n.clone(),
        _ => match infer(obj, scope) {
            Some(ty) => match strip_nullable(ty) {
                Type::Named(n) => n,
                _ => return,
            },
            None => return,
        },
    };
    let Some((owning, is_private)) = lookup_member(&class_name, member) else {
        return;
    };
    if is_private && current_class().as_deref() != Some(owning.as_str()) {
        errors.push(TypeCheckError::PrivateMemberAccess {
            class: owning,
            member: member.to_string(),
            span: to_source_span(obj.span.end..obj.span.end + member.len() + 1),
        });
    }
}

/// Emit [`TypeCheckError::UnknownMember`] / [`UnknownEnumVariant`] when the
/// receiver's static type is a known class or enum but the member name
/// isn't present. Unknown receivers, generic params, primitives, and types
/// `infer` couldn't resolve are conservatively ignored so we don't generate
/// false positives.
fn report_if_unknown_member(
    obj: &Spanned<Expr>,
    member: &str,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    // `self.super(...)` is a magic parent-ctor delegation form, not a
    // real member access. Same for the bare-receiver counterpart.
    if member == "super" {
        return;
    }

    // Stdlib module access: `Table.insert`, `String.byte`, etc. The
    // module isn't a class or enum, so the class/enum dispatch below
    // would skip it silently. Catch typos like `Table.insertttt` here
    // by consulting the members registry (which knows both callable
    // sigs *and* value-only fields like `Math.pi`).
    if let Expr::Ident(n) = &obj.value
        && crate::sigs::is_module(n)
    {
        // Bare `Name.member` access. For real modules (`Table`, `Io`, …)
        // accept any member listed in the registry. For *value types*
        // (`File`) the registry holds instance methods, so bare static
        // access is always invalid — emit the diagnostic regardless.
        if crate::sigs::is_value_type(n) || !crate::sigs::has_member(n, member) {
            errors.push(TypeCheckError::UnknownMember {
                receiver: n.clone(),
                member: member.to_string(),
                span: to_source_span(obj.span.end..obj.span.end + member.len() + 1),
            });
        }
        return;
    }

    // Resolve the receiver to either:
    //   * an enum *class* (lookup the variant) — only via a bare Ident
    //     receiver, since `Color.Red` is the access path; OR
    //   * a regular class (lookup the member, walking inheritance).
    //
    // Receivers whose inferred type is an enum-valued local (e.g.
    // `local s: Status = Status.Alive` then `s.describe()`) intentionally
    // fall through: we don't yet track enum method names statically, so
    // emitting here would generate false positives.
    let (receiver_name, is_enum_class) = match &obj.value {
        Expr::Self_ => match current_class() {
            Some(n) => (n, false),
            None => return,
        },
        Expr::Ident(n) if with_classes(|r| r.contains_key(n)) => (n.clone(), false),
        Expr::Ident(n) if with_enums(|r| r.contains_key(n)) => (n.clone(), true),
        _ => match infer(obj, scope) {
            Some(ty) => match strip_nullable(ty) {
                Type::Named(n) => {
                    if with_classes(|r| r.contains_key(&n)) {
                        (n, false)
                    } else if crate::sigs::is_module(&n) {
                        // Native instance type (e.g. `File`) whose method
                        // set is recorded in the members registry. Catch
                        // typos like `file.readAll` here.
                        if !crate::sigs::has_member(&n, member) {
                            errors.push(TypeCheckError::UnknownMember {
                                receiver: n,
                                member: member.to_string(),
                                span: to_source_span(
                                    obj.span.end..obj.span.end + member.len() + 1,
                                ),
                            });
                        }
                        return;
                    } else {
                        // Enum-typed locals, primitives, generics, etc.
                        return;
                    }
                }
                _ => return,
            },
            None => return,
        },
    };

    if is_enum_class {
        let known = with_enums(|r| {
            r.get(&receiver_name)
                .is_some_and(|info| info.variants.contains_key(member))
        });
        if !known {
            errors.push(TypeCheckError::UnknownEnumVariant {
                enum_name: receiver_name,
                variant: member.to_string(),
                span: to_source_span(obj.span.end..obj.span.end + member.len() + 1),
            });
        }
    } else if lookup_member(&receiver_name, member).is_none() {
        errors.push(TypeCheckError::UnknownMember {
            receiver: receiver_name,
            member: member.to_string(),
            span: to_source_span(obj.span.end..obj.span.end + member.len() + 1),
        });
    }
}

/// Emit [`TypeCheckError::EnumVariantArity`] for `Enum.Variant(args)` calls
/// when the variant is a known tuple-style variant with a fixed arity.
fn report_if_enum_variant_arity(
    obj: &Spanned<Expr>,
    variant: &str,
    args: &[CallArg],
    errors: &mut Vec<TypeCheckError>,
    span: std::ops::Range<usize>,
) {
    let Expr::Ident(enum_name) = &obj.value else {
        return;
    };
    let Some(arity) = with_enums(|r| {
        r.get(enum_name)
            .and_then(|info| info.variants.get(variant).copied())
    }) else {
        return;
    };
    // Arity 0 means a bare/valued variant — those aren't constructed with
    // call syntax, but we'd still error elsewhere. Skip to avoid noise.
    if arity == 0 {
        return;
    }
    if args.len() != arity {
        errors.push(TypeCheckError::EnumVariantArity {
            enum_name: enum_name.clone(),
            variant: variant.to_string(),
            expected: arity,
            found: args.len(),
            span: to_source_span(span),
        });
    }
}

/// Emit [`TypeCheckError::FunctionArity`] for direct calls to top-level
/// user-defined functions when the supplied positional-argument count
/// can't match the declared signature.
fn report_if_user_function_arity(
    callee: &Spanned<Expr>,
    args: &[CallArg],
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
    span: std::ops::Range<usize>,
) {
    let Expr::Ident(name) = &callee.value else {
        return;
    };
    let Some(info) = funcs::lookup(name) else {
        return;
    };

    // Skip when any argument is named — those may legitimately fill in
    // defaults out of order. The runtime still validates names.
    if args.iter().any(|a| matches!(a, CallArg::Named { .. })) {
        return;
    }

    let positional = args.len();
    let arity_ok;
    if info.variadic {
        // With a variadic last param, `total - 1 - defaults` is the
        // minimum required positional count.
        let min_required = info.total.saturating_sub(1).saturating_sub(info.defaults);
        if positional < min_required {
            errors.push(TypeCheckError::FunctionArity {
                callee: name.clone(),
                expected: min_required,
                found: positional,
                span: to_source_span(span),
            });
            return;
        }
        arity_ok = true;
    } else {
        let min_required = info.total.saturating_sub(info.defaults);
        if positional < min_required || positional > info.total {
            errors.push(TypeCheckError::FunctionArity {
                callee: name.clone(),
                expected: info.total,
                found: positional,
                span: to_source_span(span),
            });
            return;
        }
        arity_ok = true;
    }

    if !arity_ok {
        return;
    }

    // Argument-type validation. Mirrors `check_user_method_args` /
    // `check_native_args`: walks left-to-right, unifying generic type
    // parameters as we go, then checks each slot for compatibility.
    let type_params = &info.type_params;
    let mut subst: std::collections::HashMap<String, Type> =
        std::collections::HashMap::new();

    for (i, arg) in args.iter().enumerate() {
        let Some(p) = info.params.get(i).or_else(|| {
            if info.variadic {
                info.params.last()
            } else {
                None
            }
        }) else {
            break;
        };
        if p.variadic {
            continue;
        }
        let CallArg::Positional(value_expr) = arg else {
            continue;
        };
        let expected = if type_params.is_empty() {
            p.ty.clone()
        } else {
            substitute(&p.ty, &subst, type_params)
        };
        if is_any(&expected) {
            continue;
        }
        let Some(found_ty) = infer(value_expr, scope) else {
            continue;
        };
        if !type_params.is_empty() {
            unify(&expected, &found_ty, type_params, &mut subst);
        }
        let expected = if type_params.is_empty() {
            expected
        } else {
            substitute(&expected, &subst, type_params)
        };
        if is_any(&expected) {
            continue;
        }
        if !types_compatible(&expected, &found_ty) {
            errors.push(TypeCheckError::NativeArgTypeMismatch {
                callee: name.clone(),
                arg: i + 1,
                expected: type_to_string(&expected),
                found: type_to_string(&found_ty),
                span: to_source_span(value_expr.span.clone()),
            });
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Assignment compatibility — only flags the cases we can prove are wrong.
// ──────────────────────────────────────────────────────────────────────────────

/// Reject `lhs ?? rhs` when the fallback's type is incompatible with the
/// stripped base type of the left-hand side. Stays conservative: if either
/// side's type can't be inferred, or either side is `any`, the check is
/// skipped (matches how the rest of the typechecker handles `any`).
pub(super) fn check_coalesce_fallback(
    lhs: &Spanned<Expr>,
    rhs: &Spanned<Expr>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    let (Some(lt), Some(rt)) = (infer(lhs, scope), infer(rhs, scope)) else {
        return;
    };
    let base = strip_nullable(lt);
    // `nil` fallback collapses to `T?` and is always fine.
    if matches!(&rt, Type::Named(n) if n == "nil") {
        return;
    }
    // Literal-`nil` lhs (or any expression typed exactly as `nil`) is a
    // degenerate `??` — the fallback is always taken, so any type is
    // valid. Don't second-guess it.
    if matches!(&base, Type::Named(n) if n == "nil") {
        return;
    }
    // Don't flag when either side is `any` — that's the explicit escape
    // hatch; flagging would just spam diagnostics in dynamic code.
    if is_any(&base) || is_any(&strip_nullable(rt.clone())) {
        return;
    }
    if !types_compatible(&base, &strip_nullable(rt.clone())) {
        errors.push(TypeCheckError::CoalesceFallbackTypeMismatch {
            expected: type_to_string(&base),
            found: type_to_string(&rt),
            span: to_source_span(rhs.span.clone()),
        });
    }
}

/// True for numeric scalars: `integer`, `float`, the `number` sentinel, and
/// `any` (which acts as a wildcard).
fn is_numeric_like(ty: &Type) -> bool {
    matches!(ty, Type::Named(n) if n == "integer" || n == "float" || n == "number" || n == "any")
}

/// True for operands of `..` (string concatenation). Saule follows Lua and
/// coerces numbers to strings, so all numeric types are accepted too.
fn is_concat_like(ty: &Type) -> bool {
    is_string_like(ty) || is_numeric_like(ty)
}

/// Friendly printable name of a binary operator, used in diagnostics.
fn binop_name(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Mod => "%",
        BinOp::Eq => "==",
        BinOp::NotEq => "!=",
        BinOp::Lt => "<",
        BinOp::LtEq => "<=",
        BinOp::Gt => ">",
        BinOp::GtEq => ">=",
        BinOp::And => "and",
        BinOp::Or => "or",
        BinOp::Concat => "..",
        BinOp::Coalesce => "??",
    }
}

/// Per-operator type validation. Each branch checks just enough to flag
/// obvious mistakes (string + integer, table < 5, "x" and y, etc.) while
/// staying conservative for `any`, `nil`, and uninferable expressions.
pub(super) fn check_binary_op(
    op: BinOp,
    lhs: &Spanned<Expr>,
    rhs: &Spanned<Expr>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    match op {
        BinOp::Coalesce => check_coalesce_fallback(lhs, rhs, scope, errors),
        BinOp::Eq | BinOp::NotEq => check_equality_compat(op, lhs, rhs, scope, errors),
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
            let name = binop_name(op);
            check_operand_kind(
                name,
                "`integer` or `float`",
                is_numeric_like,
                lhs,
                scope,
                errors,
            );
            check_operand_kind(
                name,
                "`integer` or `float`",
                is_numeric_like,
                rhs,
                scope,
                errors,
            );
            check_numeric_kinds_match(lhs, rhs, scope, errors);
        }
        BinOp::And | BinOp::Or => {
            // Saule follows Lua semantics for `and`/`or`: they short-circuit
            // on truthiness and accept any value (`nil or "x"` → `"x"`),
            // so the operands aren't type-restricted.
        }
        BinOp::Concat => {
            check_operand_kind(
                "..",
                "`string` or numeric",
                is_concat_like,
                lhs,
                scope,
                errors,
            );
            check_operand_kind(
                "..",
                "`string` or numeric",
                is_concat_like,
                rhs,
                scope,
                errors,
            );
        }
        BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => {
            check_ordering_operands(op, lhs, rhs, scope, errors);
        }
    }
}

/// Check that a single operand satisfies a predicate; emit if not. Stays
/// silent when inference fails, the operand is `any`, or the operand is
/// `nil` (nullable misuse is reported elsewhere).
fn check_operand_kind(
    op: &'static str,
    expected: &'static str,
    pred: impl Fn(&Type) -> bool,
    arg: &Spanned<Expr>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    let Some(t) = infer(arg, scope) else {
        return;
    };
    let base = strip_nullable(t.clone());
    if is_any(&base) {
        return;
    }
    if matches!(&base, Type::Named(n) if n == "nil") {
        return;
    }
    if !pred(&base) {
        errors.push(TypeCheckError::BinaryOperandTypeMismatch {
            op,
            expected,
            found: type_to_string(&t),
            span: to_source_span(arg.span.clone()),
        });
    }
}

/// `<`, `<=`, `>`, `>=` require both sides to be in the same family —
/// numeric/numeric or string/string. Anything else is flagged on the side
/// that breaks the family.
fn check_ordering_operands(
    op: BinOp,
    lhs: &Spanned<Expr>,
    rhs: &Spanned<Expr>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    let name = binop_name(op);
    // First, each side individually must be orderable (numeric or string).
    check_operand_kind(
        name,
        "`integer`, `float`, or `string`",
        is_orderable,
        lhs,
        scope,
        errors,
    );
    check_operand_kind(
        name,
        "`integer`, `float`, or `string`",
        is_orderable,
        rhs,
        scope,
        errors,
    );
    // Then, both sides must agree on the family.
    let (Some(lt), Some(rt)) = (infer(lhs, scope), infer(rhs, scope)) else {
        return;
    };
    let lb = strip_nullable(lt.clone());
    let rb = strip_nullable(rt.clone());
    if is_any(&lb) || is_any(&rb) {
        return;
    }
    if matches!(&lb, Type::Named(n) if n == "nil") || matches!(&rb, Type::Named(n) if n == "nil") {
        return;
    }
    let l_num = is_numeric_like(&lb);
    let r_num = is_numeric_like(&rb);
    let l_str = is_string_like(&lb);
    let r_str = is_string_like(&rb);
    let same_family = (l_num && r_num) || (l_str && r_str);
    if !same_family && (l_num || l_str) && (r_num || r_str) {
        // Both individually orderable but in different families — flag rhs
        // with the lhs family as the expected one.
        let expected = if l_num {
            "`integer` or `float`"
        } else {
            "`string`"
        };
        errors.push(TypeCheckError::BinaryOperandTypeMismatch {
            op: name,
            expected,
            found: type_to_string(&rt),
            span: to_source_span(rhs.span.clone()),
        });
    }
}

fn is_orderable(ty: &Type) -> bool {
    is_numeric_like(ty) || is_string_like(ty)
}

/// Saule is strict on numeric kinds — `integer` and `float` never mix
/// implicitly. When both operands have a known *concrete* numeric kind
/// (i.e. not the `number` sentinel, not `any`, not generic), flag the
/// pair when they disagree. The error mirrors `RuntimeError::NumericMix`
/// so the user sees the same diagnostic at compile time.
fn check_numeric_kinds_match(
    lhs: &Spanned<Expr>,
    rhs: &Spanned<Expr>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    let (Some(lt), Some(rt)) = (infer(lhs, scope), infer(rhs, scope)) else {
        return;
    };
    let lb = strip_nullable(lt);
    let rb = strip_nullable(rt);
    let kind = |t: &Type| -> Option<&'static str> {
        if let Type::Named(n) = t {
            match n.as_str() {
                "integer" => Some("integer"),
                "float" => Some("float"),
                _ => None,
            }
        } else {
            None
        }
    };
    let (Some(lk), Some(rk)) = (kind(&lb), kind(&rb)) else {
        return;
    };
    if lk != rk {
        // Span covers both sides so the underline brackets the whole
        // expression — matches the runtime diagnostic.
        let span_start = lhs.span.start.min(rhs.span.start);
        let span_end = lhs.span.end.max(rhs.span.end);
        errors.push(TypeCheckError::NumericMix {
            span: to_source_span(span_start..span_end),
        });
    }
}

/// Validate every stage of a `when(source):stage1():stage2()…` pipeline.
///
/// For each stage we:
///   * recursively check the stage's argument expressions;
///   * look up `stage.name` in the top-level function registry — if it
///     isn't a known free function, emit [`TypeCheckError::UnknownPipeStage`]
///     and stop threading types through (subsequent stages get `None`);
///   * verify the *piped* arg-0 matches `params[0].ty` — this is the
///     spec's "Expected 'number' as first argument to 'square', got
///     'string'" diagnostic;
///   * verify the explicit arg count agrees with the declared arity
///     (remembering that the piped value covers one of the parameters);
///   * advance the "current type" to the function's declared return
///     type for the next stage.
fn check_pipe(
    source: &Spanned<Expr>,
    stages: &[PipeStage],
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    check_expr(source, scope, errors);
    for stage in stages {
        for a in &stage.args {
            check_arg(a, scope, errors);
        }
    }

    // Walk the chain left-to-right, threading the "current value type"
    // through each stage. `None` means "we lost the trail" — we keep
    // walking so syntactic errors in later stages still surface, but
    // skip arg-0 type compatibility checks.
    let mut current: Option<Type> = infer(source, scope);
    for stage in stages {
        let Some(info) = super::funcs::lookup(&stage.name) else {
            errors.push(TypeCheckError::UnknownPipeStage {
                stage: stage.name.clone(),
                span: to_source_span(stage.span.clone()),
            });
            current = None;
            continue;
        };

        // Arity check: piped value counts as one argument. The declared
        // function must therefore have at least one parameter, and the
        // explicit args must fit into `params[1..]` (respecting defaults
        // and variadics).
        if info.total == 0 && !info.variadic {
            errors.push(TypeCheckError::PipeStageArity {
                stage: stage.name.clone(),
                expected: 1,
                found: stage.args.len() + 1,
                span: to_source_span(stage.span.clone()),
            });
            current = info.return_ty.clone();
            continue;
        }
        let explicit = stage.args.len();
        let total = info.total;
        let defaults = info.defaults;
        let min_explicit = total.saturating_sub(1 + defaults);
        let max_explicit = total - 1;
        let arity_ok = if info.variadic {
            // Last param is `...rest`; only enforce the lower bound.
            explicit + 1 >= total.saturating_sub(1)
        } else {
            explicit >= min_explicit && explicit <= max_explicit
        };
        if !arity_ok {
            errors.push(TypeCheckError::PipeStageArity {
                stage: stage.name.clone(),
                expected: total,
                found: explicit + 1,
                span: to_source_span(stage.span.clone()),
            });
        }

        // First-arg type check — the headline pipeline diagnostic.
        if let (Some(actual), Some(expected_param)) = (current.as_ref(), info.params.first()) {
            let actual_base = strip_nullable(actual.clone());
            let expected_base = strip_nullable(expected_param.ty.clone());
            let skip = is_any(&actual_base)
                || is_any(&expected_base)
                || matches!(&actual_base, Type::Named(n) if n == "nil");
            if !skip && !types_compatible(&expected_param.ty, actual) {
                errors.push(TypeCheckError::PipeStageTypeMismatch {
                    stage: stage.name.clone(),
                    expected: type_to_string(&expected_param.ty),
                    found: type_to_string(actual),
                    span: to_source_span(stage.span.clone()),
                });
            }
        }

        // Thread the return type into the next stage (so the chain
        // type-checks transitively). `None` propagates as "unknown" and
        // simply disables the next first-arg comparison.
        current = info.return_ty.clone();
    }
}

/// Reject equality comparisons whose two sides have provably-disjoint
/// types (e.g. comparing a `table?` value to a string literal). Skips when
/// either side involves `any` or `nil`, since `T? == nil` is the idiomatic
/// nullability check.
pub(super) fn check_equality_compat(
    op: BinOp,
    lhs: &Spanned<Expr>,
    rhs: &Spanned<Expr>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    let (Some(lt), Some(rt)) = (infer(lhs, scope), infer(rhs, scope)) else {
        return;
    };
    let lb = strip_nullable(lt.clone());
    let rb = strip_nullable(rt.clone());
    // `nil` on either side is legitimate — `x == nil` is how you check
    // nullability.
    if matches!(&lb, Type::Named(n) if n == "nil") || matches!(&rb, Type::Named(n) if n == "nil") {
        return;
    }
    if is_any(&lb) || is_any(&rb) {
        return;
    }
    // Compatible in either direction → OK.
    if types_compatible(&lb, &rb) || types_compatible(&rb, &lb) {
        return;
    }
    let result = if matches!(op, BinOp::Eq) {
        "false"
    } else {
        "true"
    };
    // Span covers both sides.
    let span_start = lhs.span.start.min(rhs.span.start);
    let span_end = lhs.span.end.max(rhs.span.end);
    errors.push(TypeCheckError::DisjointEquality {
        left: type_to_string(&lt),
        right: type_to_string(&rt),
        result,
        span: to_source_span(span_start..span_end),
    });
}

pub(super) fn check_assignment_compat(
    decl_ty: &Type,
    value: &Spanned<Expr>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    // When the value is `lhs ?? rhs`, the declared type tells us what the
    // whole expression is supposed to produce — use its stripped base as
    // the expected type for the fallback. This catches mismatches even
    // when `lhs` has lost type info (e.g. returns `any?`).
    if let Expr::Binary {
        op: BinOp::Coalesce,
        rhs,
        ..
    } = &value.value
    {
        let expected_base = strip_nullable(decl_ty.clone());
        if !matches!(&expected_base, Type::Named(n) if n == "nil" || n == "any")
            && let Some(rt) = infer(rhs, scope)
        {
            let rt_base = strip_nullable(rt.clone());
            let rt_is_nil = matches!(&rt_base, Type::Named(n) if n == "nil");
            let rt_is_any = is_any(&rt_base);
            if !rt_is_nil && !rt_is_any && !types_compatible(&expected_base, &rt_base) {
                errors.push(TypeCheckError::CoalesceFallbackTypeMismatch {
                    expected: type_to_string(&expected_base),
                    found: type_to_string(&rt),
                    span: to_source_span(rhs.span.clone()),
                });
            }
        }
    }

    if is_nullable(decl_ty) {
        // Even for nullable slots we still want to reject obviously
        // incompatible value types — e.g. `local x: string? = some_entry`
        // where `some_entry: Entry?`. `nil` and `any` stay permissive.
        if let Some(value_ty) = infer(value, scope)
            && !matches!(&value_ty, Type::Named(n) if n == "nil")
            && !is_any(&value_ty)
            && !types_compatible(decl_ty, &value_ty)
        {
            errors.push(TypeCheckError::AssignmentTypeMismatch {
                expected: type_to_string(decl_ty),
                found: type_to_string(&value_ty),
                span: to_source_span(value.span.clone()),
            });
        }
        return;
    }
    if matches!(value.value, Expr::Nil) {
        errors.push(TypeCheckError::NilToNonNullable {
            ty: type_to_string(decl_ty),
            span: to_source_span(value.span.clone()),
        });
        return;
    }

    // Table-aware checks for table literals assigned to a typed table.
    // Splits the literal into its positional and field halves so each can
    // be validated against the declared table shape independently.
    if let (
        Type::Table {
            key,
            value: elem_ty,
        },
        Expr::Table(items),
    ) = (decl_ty, &value.value)
    {
        let has_positional = items.iter().any(|e| matches!(e, TableEntry::Positional(_)));
        let has_field = items.iter().any(|e| matches!(e, TableEntry::Field { .. }));

        // `{a, b, c}` literal cannot fill a map-typed table whose key is not
        // integer-compatible.
        if let Some(k) = key
            && !is_integer_like(k)
            && has_positional
        {
            errors.push(TypeCheckError::TableArrayLiteralForMap {
                key: type_to_string(k),
                value: type_to_string(elem_ty),
                span: to_source_span(value.span.clone()),
            });
            return;
        }
        // Field entries (`name: ...`) require the table to declare a key
        // type compatible with `string`. Array-typed `table<T>` rejects them.
        if has_field {
            let key_ok = match key {
                None => false,
                Some(k) => is_string_like(k),
            };
            if !key_ok {
                errors.push(TypeCheckError::TableArrayLiteralForMap {
                    key: key
                        .as_deref()
                        .map(type_to_string)
                        .unwrap_or_else(|| "integer".to_string()),
                    value: type_to_string(elem_ty),
                    span: to_source_span(value.span.clone()),
                });
                return;
            }
        }
        // Each value must match the declared value type.
        for item in items {
            match item {
                TableEntry::Positional(e) | TableEntry::Field { value: e, .. } => {
                    check_element_compat(elem_ty, e, scope, errors);
                }
            }
        }
        return;
    }

    if let Some(value_ty) = infer(value, scope) {
        if is_nullable(&value_ty) {
            errors.push(TypeCheckError::NullableToNonNullable {
                from: type_to_string(&value_ty),
                to: type_to_string(decl_ty),
                span: to_source_span(value.span.clone()),
            });
            return;
        }
        // General incompatibility (e.g. `table<Storage>` vs `table<string>`
        // from `Os.args()`). `any` / `nil` stay permissive — they're the
        // "I don't know" sentinels. Skip when the declared slot is `any`
        // or a free type parameter for the same reason.
        if !matches!(&value_ty, Type::Named(n) if n == "nil")
            && !is_any(&value_ty)
            && !is_any(decl_ty)
            && !types_compatible(decl_ty, &value_ty)
        {
            errors.push(TypeCheckError::AssignmentTypeMismatch {
                expected: type_to_string(decl_ty),
                found: type_to_string(&value_ty),
                span: to_source_span(value.span.clone()),
            });
        }
    }
}

/// Index key must match the table's declared key type.
pub(super) fn check_table_key_compat(
    expected: &Type,
    index: &Spanned<Expr>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    if let Some(idx_ty) = infer(index, scope)
        && !types_compatible(expected, &idx_ty)
    {
        errors.push(TypeCheckError::TableKeyTypeMismatch {
            expected: type_to_string(expected),
            found: type_to_string(&idx_ty),
            span: to_source_span(index.span.clone()),
        });
    }
}

/// True for `integer` and `any` — the key types that an array-style literal
/// can satisfy.
fn is_integer_like(ty: &Type) -> bool {
    matches!(ty, Type::Named(n) if n == "integer" || n == "any")
}

/// True for `string` and `any` — the key types that a field-style literal
/// (`name: expr` / `"text": expr`) can satisfy.
fn is_string_like(ty: &Type) -> bool {
    matches!(ty, Type::Named(n) if n == "string" || n == "any")
}

/// Element-of-table compatibility — accepts literals/`Ident`s whose inferred
/// type matches, and stays quiet otherwise (conservative).
pub(super) fn check_element_compat(
    expected: &Type,
    value: &Spanned<Expr>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    if let Some(value_ty) = infer(value, scope)
        && !types_compatible(expected, &value_ty)
    {
        errors.push(TypeCheckError::TableElementTypeMismatch {
            expected: type_to_string(expected),
            found: type_to_string(&value_ty),
            span: to_source_span(value.span.clone()),
        });
    }
}

/// Conservative type compatibility — names match (or either is `any`), or
/// `value_ty` is `Nullable` of a compatible inner, etc.
pub(super) fn types_compatible(expected: &Type, value_ty: &Type) -> bool {
    // Suppress `is_interface` unused-warning when state moves; reference here.
    let _ = is_interface;
    match (expected, value_ty) {
        // Same-name primitives, plus `any` on either side, plus `nil` on the
        // value side (nil is universally assignable; nullable-rejection is
        // handled separately by `NullableToNonNullable`).
        (Type::Named(a), Type::Named(b)) => {
            if a == b || a == "any" || b == "any" || b == "nil" {
                return true;
            }
            // Generic type parameters in scope match anything — they're
            // effectively `any` from the body's point of view.
            if is_type_param(a) || is_type_param(b) {
                return true;
            }
            // `number` is the sentinel used in native sigs to mean
            // "integer or float" — accept either.
            if a == "number" && (b == "integer" || b == "float" || b == "number") {
                return true;
            }
            // Class/interface subtyping: a value of type `b` is assignable to
            // a slot of type `a` if `b` is a subtype of `a` (class implements
            // interface, class extends class, interface extends interface).
            if is_subtype_named(b, a) {
                let _ = class_implements; // keep import even if unused later
                return true;
            }
            false
        }
        // `table<any>` (or `table<any, any>`) matches any table — used by
        // native sigs like `Table.insert(t, ...)` to mean "any table".
        (Type::Table { key: ek, value: ev }, Type::Table { key: vk, value: vv }) => {
            let key_ok = match (ek, vk) {
                (None, None) => true,
                (Some(a), Some(b)) => types_compatible(a, b),
                // Cross-shape (`table<T>` vs `table<K, V>`) only when one
                // side is the `any` wildcard.
                _ => is_any(ev),
            };
            key_ok && (is_any(ev) || types_compatible(ev, vv))
        }
        // Expected table, but value is the bare type-name `table`, `any` or
        // `nil` — accept (caller has erased the element type, or it's nil).
        (Type::Table { .. }, Type::Named(n)) if n == "table" || n == "any" || n == "nil" => true,
        // Expected `any` / `table` / `nil` named slot, value is a table —
        // accept (we widen to the named slot).
        (Type::Named(n), Type::Table { .. }) if n == "table" || n == "any" => true,
        // Bare `function` (or `any` / `nil`) named slot vs an actual function
        // value — mirrors the `table` arms. Native sigs erase the precise
        // shape (e.g. an `SFunction` parameter renders as `function`).
        (Type::Function { .. }, Type::Named(n)) if n == "function" || n == "any" || n == "nil" => {
            true
        }
        (Type::Named(n), Type::Function { .. }) if n == "function" || n == "any" => true,
        (Type::Nullable(a), b) => types_compatible(a, b),
        (a, Type::Nullable(b)) => types_compatible(a, b),
        // Function / Tuple shapes — only equal-shape is strictly compatible,
        // but the checker doesn't track those precisely yet. Accept rather
        // than emit false positives.
        (Type::Function { .. }, Type::Function { .. }) => true,
        (Type::Tuple(_), Type::Tuple(_)) => true,
        // Different kinds (e.g. table vs integer) — reject.
        _ => false,
    }
}

/// Reject `if`/`while`/`until` conditions that the type system can prove are
/// not `boolean`. Conservative: when `infer` can't determine a type, we skip
/// silently so calls and dynamic expressions keep working.
pub(super) fn check_boolean_cond(
    construct: &'static str,
    cond: &Spanned<Expr>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    let Some(ty) = infer(cond, scope) else {
        return;
    };
    let is_bool = matches!(&ty, Type::Named(n) if n == "boolean" || n == "any");
    if !is_bool {
        errors.push(TypeCheckError::NonBooleanCondition {
            construct,
            found: type_to_string(&ty),
            span: to_source_span(cond.span.clone()),
        });
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Lightweight type inference.
//
// Returns `Some(ty)` only when we can prove the type. Anything we can't see
// through (calls, member reads on unknown classes, indexing) returns `None`,
// and callers treat that as "don't know, don't complain".
// ──────────────────────────────────────────────────────────────────────────────

pub(super) fn infer(expr: &Spanned<Expr>, scope: &Scope) -> Option<Type> {
    match &expr.value {
        Expr::Nil => Some(Type::Named("nil".into())),
        Expr::Int(_) => Some(Type::Named("integer".into())),
        Expr::Float(_) => Some(Type::Named("float".into())),
        Expr::Bool(_) => Some(Type::Named("boolean".into())),
        Expr::Str(_) => Some(Type::Named("string".into())),
        Expr::Ident(n) => scope.lookup(n).cloned(),
        Expr::Self_ => current_class().map(Type::Named),
        // `obj.field` — when `obj` resolves to a known class, return the
        // declared type of that field (walks parents). Methods aren't fields,
        // so this only fires for stored slots declared with `local x: T`.
        Expr::Member { obj, name } => {
            let ty = infer(obj, scope)?;
            let Type::Named(class_name) = strip_nullable(ty) else {
                return None;
            };
            if let Some(t) = saule_semantic::lookup_field_type(&class_name, name) {
                return Some(t);
            }
            // Not a field — fall back to method-as-value: a bare `obj.method`
            // reference (no parens) yields a function value. We surface this
            // so downstream checks (e.g. `match` against a method ref) can
            // detect a missing call.
            let sig = saule_semantic::lookup_method(&class_name, name)?;
            Some(Type::Function {
                params: sig.params.iter().map(|p| p.ty.clone()).collect(),
                ret: Box::new(sig.return_ty.clone().unwrap_or(Type::Named("any".into()))),
            })
        }
        Expr::SafeMember { .. } => Some(Type::Nullable(Box::new(Type::Named("any".into())))),
        // `t[k]` — return the declared element type as-is. If the table
        // was declared `table<V?>` the result is nullable and member
        // access on it will trip the nullable-receiver check; if it was
        // declared `table<V>` we trust the declaration and yield `V`.
        Expr::Index { obj, index: _ } => {
            let ty = infer(obj, scope)?;
            match strip_nullable(ty) {
                Type::Table { value, .. } => Some(*value),
                _ => None,
            }
        }
        Expr::ForceUnwrap(inner) => match infer(inner, scope)? {
            Type::Nullable(t) => Some(*t),
            other => Some(other),
        },
        // A table literal infers to a typed `table<…>` so a bare `table`
        // annotation can be refined (`local xs: table = {"a"}` → `table<string>`)
        // and a literal passed straight to a native is checked element-wise.
        Expr::Table(items) => Some(infer_table_literal(items, scope)),
        Expr::Binary { op, lhs, rhs } => match op {
            BinOp::Coalesce => {
                // `lhs ?? rhs`: result is non-nullable iff `rhs` is non-nullable.
                match (infer(lhs, scope), infer(rhs, scope)) {
                    (Some(lt), Some(rt)) => {
                        let base = strip_nullable(lt);
                        if is_nullable(&rt) {
                            Some(Type::Nullable(Box::new(base)))
                        } else {
                            Some(base)
                        }
                    }
                    _ => None,
                }
            }
            BinOp::Eq | BinOp::NotEq | BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => {
                Some(Type::Named("boolean".into()))
            }
            BinOp::And | BinOp::Or => None,
            BinOp::Concat => Some(Type::Named("string".into())),
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
                infer(lhs, scope).or_else(|| infer(rhs, scope))
            }
        },
        // `Foo(args)` where `Foo` is a known class → produces a `Foo`.
        // Otherwise, look up the qualified callee name in:
        //   1. the local class registry as a user-defined method
        //      (`Foo.bar(...)` → return type of `Foo.bar`); or
        //   2. the native signature table (`String.byte`, `Math.tointeger`,
        //      `assert`).
        Expr::Call { callee, args } => {
            if let Expr::Ident(n) = &callee.value
                && with_classes(|reg| reg.contains_key(n))
            {
                return Some(Type::Named(n.clone()));
            }
            // `Class.method(args)` — receiver is the class itself.
            if let Expr::Member { obj, name } = &callee.value
                && let Expr::Ident(class_name) = &obj.value
                && let Some(sig) = saule_semantic::lookup_method(class_name, name)
            {
                return semantic_method_return(&sig, args, scope);
            }
            // `instance.method(args)` — receiver inferred to a class.
            if let Expr::Member { obj, name } = &callee.value
                && let Some(ty) = infer(obj, scope)
                && let Type::Named(class_name) = strip_nullable(ty)
                && let Some(sig) = saule_semantic::lookup_method(&class_name, name)
            {
                return semantic_method_return(&sig, args, scope);
            }
            if let Some(qname) = native_callee_name(callee, scope)
                && let Some(sig) = crate::sigs::lookup(&qname)
            {
                // Generic native: bind type params from the actual args, then
                // substitute the return list. Falls back to the raw returns
                // for non-generic sigs.
                let returns = if sig.type_params.is_empty() {
                    sig.returns
                } else {
                    let mut subst: std::collections::HashMap<String, Type> =
                        std::collections::HashMap::new();
                    let positional: Vec<&Spanned<Expr>> = args
                        .iter()
                        .filter_map(|a| match a {
                            CallArg::Positional(e) => Some(e),
                            CallArg::Named { .. } => None,
                        })
                        .collect();
                    for (i, expected) in sig.params.iter().enumerate() {
                        let Some(arg_expr) = positional.get(i) else {
                            break;
                        };
                        if let Some(found_ty) = infer(arg_expr, scope) {
                            unify(expected, &found_ty, &sig.type_params, &mut subst);
                        }
                    }
                    sig.returns
                        .iter()
                        .map(|t| substitute(t, &subst, &sig.type_params))
                        .collect()
                };
                return first_or_tuple(returns);
            }
            None
        }
        // `obj:method(args)` — same lookup as the `obj.method(args)` case.
        Expr::MethodCall { obj, method, .. } => {
            if let Some(ty) = infer(obj, scope)
                && let Type::Named(class_name) = strip_nullable(ty)
                && let Some(sig) = saule_semantic::lookup_method(&class_name, method)
            {
                return sig.return_ty;
            }
            None
        }
        // A `match` expression has the type of its unified arm bodies.
        // We collect every arm's inferred type and:
        //   * if any arm yields nil / Nullable → the result is `Nullable(base)`;
        //   * if arm bases agree → that base (wrapped if nullable seen);
        //   * if arm bases disagree → `any`;
        //   * if no arm could be inferred → `None` (give up conservatively).
        // Block-bodied arms aren't inferable today; they're skipped, but
        // their presence still widens the result to `any` rather than
        // silently dropping a possibly-different branch.
        Expr::Match { arms, .. } => {
            let mut any_nullable = false;
            let mut bases: Vec<Type> = Vec::new();
            let mut had_block = false;
            for a in arms {
                match &a.body {
                    saule_ast::MatchBody::Expr(e) => {
                        let Some(t) = infer(e, scope) else { continue };
                        if matches!(&t, Type::Named(n) if n == "nil") || is_nullable(&t) {
                            any_nullable = true;
                        }
                        bases.push(strip_nullable(t));
                    }
                    saule_ast::MatchBody::Block(_) => {
                        had_block = true;
                    }
                }
            }
            if bases.is_empty() {
                return None;
            }
            let first = bases[0].clone();
            let same = bases.iter().all(|t| matches_base(t, &first));
            let base = if same && !had_block {
                first
            } else {
                Type::Named("any".into())
            };
            Some(if any_nullable {
                Type::Nullable(Box::new(base))
            } else {
                base
            })
        }
        // `when(x):a():b():c()` has the return type of the last stage's
        // declared function, regardless of any earlier inference failures.
        Expr::Pipe { stages, .. } => stages
            .last()
            .and_then(|s| super::funcs::lookup(&s.name))
            .and_then(|info| info.return_ty.clone()),
        _ => None,
    }
}

pub(super) fn strip_nullable(ty: Type) -> Type {
    match ty {
        Type::Nullable(t) => *t,
        other => other,
    }
}

/// Infer the static type of a table literal.
///
/// * All-positional (`{a, b, c}`) → `table<T>` when every element shares the
///   same inferred base type, otherwise `table<any>`.
/// * All-field (`{x: a, y: b}`) → `table<string, V>` with `V` unified the same
///   way.
/// * Empty (`{}`) or mixed positional+field → `table<any>` (element type
///   can't be pinned down without false positives).
///
/// Any element whose type can't be inferred widens the result to `any`, so the
/// outcome is always at least as permissive as the old `None` behaviour.
fn infer_table_literal(items: &[TableEntry], scope: &Scope) -> Type {
    let mut has_positional = false;
    let mut has_field = false;
    let mut elem: Option<Type> = None;
    let mut unknown = false;

    for item in items {
        let value_expr = match item {
            TableEntry::Positional(e) => {
                has_positional = true;
                e
            }
            TableEntry::Field { value, .. } => {
                has_field = true;
                value
            }
        };
        match infer(value_expr, scope) {
            Some(t) => {
                let base = strip_nullable(t);
                elem = match elem {
                    None => Some(base),
                    Some(prev) if matches_base(&prev, &base) => Some(prev),
                    Some(_) => Some(Type::Named("any".into())),
                };
            }
            None => unknown = true,
        }
    }

    let value_ty = match elem {
        Some(t) if !unknown => t,
        _ => Type::Named("any".into()),
    };

    if has_field && !has_positional {
        Type::Table {
            key: Some(Box::new(Type::Named("string".into()))),
            value: Box::new(value_ty),
        }
    } else if has_positional && !has_field {
        Type::Table {
            key: None,
            value: Box::new(value_ty),
        }
    } else {
        // Empty or mixed — known to be a table, element type left open.
        Type::Table {
            key: None,
            value: Box::new(Type::Named("any".into())),
        }
    }
}

/// Structural equality on stripped bases — used by `Match` inference to
/// decide whether all arms produce the same shape (in which case we keep
/// that type) or whether to widen to `any`.
fn matches_base(a: &Type, b: &Type) -> bool {
    match (a, b) {
        (Type::Named(x), Type::Named(y)) => x == y,
        (Type::Nullable(x), Type::Nullable(y)) => matches_base(x, y),
        (Type::Tuple(xs), Type::Tuple(ys)) => {
            xs.len() == ys.len() && xs.iter().zip(ys).all(|(a, b)| matches_base(a, b))
        }
        (
            Type::Table {
                key: kx,
                value: vx,
            },
            Type::Table {
                key: ky,
                value: vy,
            },
        ) => {
            let key_ok = match (kx, ky) {
                (None, None) => true,
                (Some(a), Some(b)) => matches_base(a, b),
                _ => false,
            };
            key_ok && matches_base(vx, vy)
        }
        (
            Type::Function {
                params: px,
                ret: rx,
            },
            Type::Function {
                params: py,
                ret: ry,
            },
        ) => {
            px.len() == py.len()
                && px.iter().zip(py).all(|(a, b)| matches_base(a, b))
                && matches_base(rx, ry)
        }
        _ => false,
    }
}

/// Build a qualified callee name suitable for `stdlib::sigs::lookup`:
/// `assert`, `String.byte`, or — for instance calls on stdlib value types
/// — `File.read` (resolved by looking at the receiver's inferred type).
fn native_callee_name(callee: &Spanned<Expr>, scope: &Scope) -> Option<String> {
    match &callee.value {
        Expr::Ident(n) => Some(n.clone()),
        Expr::Member { obj, name } => {
            // Prefer the receiver Ident as a module name only when it
            // actually denotes a known module / value-type. Otherwise
            // (e.g. `file.read` where `file` is a local of type `File`)
            // fall through to the inferred-type path below so we build
            // `File.read`, not `file.read`.
            if let Expr::Ident(class) = &obj.value
                && (crate::sigs::is_module(class) || crate::sigs::lookup(class).is_some())
            {
                return Some(format!("{class}.{name}"));
            }
            // `instance.method(...)` where `instance` has a stdlib
            // value-type (e.g. `File`). Build the qname so the existing
            // sig-based arg / return checks apply to instance methods
            // the same way they do for static `Class.method` calls.
            let ty = infer(obj, scope)?;
            if let Type::Named(n) = strip_nullable(ty)
                && crate::sigs::is_value_type(&n)
            {
                return Some(format!("{n}.{name}"));
            }
            None
        }
        _ => None,
    }
}

/// Collapse a returns-list into a single inferred type: single returns stay
/// as-is, multi-returns become a `Type::Tuple`.
fn first_or_tuple(returns: Vec<Type>) -> Option<Type> {
    match returns.len() {
        0 => None,
        1 => returns.into_iter().next(),
        _ => Some(Type::Tuple(returns)),
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Flow narrowing.
//
// `narrow_truthy(cond, scope)` — apply the assumptions that hold when `cond`
// is true. Today: `x != nil` and `nil != x` strip `Nullable` off `x`. `and`
// chains compose.
//
// `narrow_falsy(cond, scope)` — apply the assumptions when `cond` is false.
// Used by the else-branch: `if x == nil then ... else ... end` narrows `x`
// in the else.
// ──────────────────────────────────────────────────────────────────────────────

pub(super) fn narrow_truthy(cond: &Spanned<Expr>, scope: &mut Scope) {
    match &cond.value {
        Expr::Binary {
            op: BinOp::NotEq,
            lhs,
            rhs,
        } => {
            if let Some(name) = pick_ident_compared_to_nil(lhs, rhs)
                && let Some(Type::Nullable(t)) = scope.lookup(name).cloned()
            {
                scope.bind(name.to_string(), *t);
            }
        }
        Expr::Binary {
            op: BinOp::And,
            lhs,
            rhs,
        } => {
            narrow_truthy(lhs, scope);
            narrow_truthy(rhs, scope);
        }
        _ => {}
    }
}

pub(super) fn narrow_falsy(cond: &Spanned<Expr>, scope: &mut Scope) {
    if let Expr::Binary {
        op: BinOp::Eq,
        lhs,
        rhs,
    } = &cond.value
        && let Some(name) = pick_ident_compared_to_nil(lhs, rhs)
        && let Some(Type::Nullable(t)) = scope.lookup(name).cloned()
    {
        scope.bind(name.to_string(), *t);
    }
}

/// If `lhs` / `rhs` is `(Ident(x), Nil)` in either order, return `Some(x)`.
fn pick_ident_compared_to_nil<'a>(
    lhs: &'a Spanned<Expr>,
    rhs: &'a Spanned<Expr>,
) -> Option<&'a str> {
    match (&lhs.value, &rhs.value) {
        (Expr::Ident(n), Expr::Nil) => Some(n),
        (Expr::Nil, Expr::Ident(n)) => Some(n),
        _ => None,
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Misc helpers.
// ──────────────────────────────────────────────────────────────────────────────

pub(super) fn type_to_string(ty: &Type) -> String {
    match ty {
        Type::Named(n) => n.clone(),
        Type::Nullable(inner) => format!("{}?", type_to_string(inner)),
        Type::Table { key: None, value } => format!("table<{}>", type_to_string(value)),
        Type::Table {
            key: Some(k),
            value,
        } => format!("table<{}, {}>", type_to_string(k), type_to_string(value)),
        Type::Tuple(elems) => {
            let parts: Vec<String> = elems.iter().map(type_to_string).collect();
            format!("({})", parts.join(", "))
        }
        Type::Function { params, ret } => {
            let parts: Vec<String> = params.iter().map(type_to_string).collect();
            format!("fn({}): {}", parts.join(", "), type_to_string(ret))
        }
    }
}

pub(super) fn is_nullable(ty: &Type) -> bool {
    match ty {
        Type::Nullable(_) => true,
        Type::Named(n) => n == "nil",
        _ => false,
    }
}
