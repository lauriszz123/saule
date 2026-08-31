//! Expression-level checks: nullable-receiver detection, private-member
//! access, native-call argument checking, lightweight type inference, and
//! the `?? != nil` style flow narrowing.

mod calls;
mod compat;
pub(crate) mod generics;
pub(crate) mod infer;
mod narrow;
mod operators;
mod pipe;

pub(crate) use calls::*;
pub(crate) use compat::*;
pub(crate) use generics::*;
pub(crate) use infer::*;
pub(crate) use narrow::*;
pub(crate) use operators::*;
pub(crate) use pipe::*;

use saule_ast::{CallArg, Expr, LambdaBody, Param, Spanned, TableEntry, Type};

use super::TypeCheckError;
use super::matches::check_match;
use super::state::{Scope, current_class, set_return_ty, with_classes};
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
        Expr::Call {
            callee,
            args,
            type_args,
        } => {
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
                report_if_user_function_arity(
                    callee,
                    args,
                    type_args.as_deref(),
                    scope,
                    errors,
                    expr.span.clone(),
                );
                report_if_function_value_call(callee, args, scope, errors, expr.span.clone());
            }
            // Each argument is walked against the type its slot expects,
            // which is what types an untyped lambda's parameters — see
            // `expected_arg_types`. Every other expression ignores the
            // expectation and checks exactly as before.
            let expected = expected_arg_types(&callee.value, args, scope);
            for (i, a) in args.iter().enumerate() {
                check_arg_expecting(a, expected.get(i).and_then(|t| t.as_ref()), scope, errors);
            }
            // Constructor call: `ClassName(args)` dispatches to `init`.
            // Validate args against the class's `init` signature so that
            // bogus extras (`Entry(item, dueDate)` against `fn init(todo)`)
            // and unknown named params get caught at typeck time.
            if let Expr::Ident(class_name) = &callee.value
                && with_classes(|r| r.contains_key(class_name))
                && let Some(mut sig) = saule_semantic::lookup_method(class_name, "init")
            {
                // A generic class's parameters are inference variables at its
                // constructor, exactly as they are in `callee_signature`:
                // `Box(5)` has to bind `T := integer` rather than compare an
                // `integer` against a rigid `T` and reject it.
                let class_params = with_classes(|r| {
                    r.get(class_name)
                        .map(|c| c.type_params.clone())
                        .unwrap_or_default()
                });
                sig.type_params = class_params
                    .into_iter()
                    .chain(sig.type_params.iter().cloned())
                    .collect();
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
            if let Some(qname) = native_callee_name(callee, scope) {
                if let Some(selected) = calls::select_native_sig(&qname, args, scope) {
                    // An overloaded native that no form's arity accepts is
                    // reported once, against the whole set — checking the
                    // closest form as well would add a second, narrower
                    // arity error contradicting the first.
                    if let Some(arities) = selected.arity_mismatch {
                        errors.push(TypeCheckError::NativeArityOverload {
                            callee: qname,
                            expected: arities,
                            found: args.len(),
                            span: to_source_span(expr.span.clone()),
                        });
                    } else {
                        check_native_args(
                            &qname,
                            &selected.sig,
                            args,
                            scope,
                            errors,
                            expr.span.clone(),
                        );
                    }
                } else if crate::sigs::lookup_const(&qname).is_some() {
                    // `Math.huge()` — the member exists but holds a value.
                    // Say so directly; otherwise the call infers as `any`
                    // and surfaces as a baffling mismatch at the binding.
                    errors.push(TypeCheckError::CallOfConstant {
                        name: qname,
                        span: to_source_span(expr.span.clone()),
                    });
                }
            }
            // `self.super(args)` delegates to the parent's constructor.
            // The receiver-based path below can't see it: `super` is not a
            // member of the current class, so `lookup_method` walks the
            // whole chain and finds nothing. `super_init_target` resolves
            // it the way the interpreter does — nearest ancestor that
            // actually declares `init`.
            if let Expr::Member { obj, name } = &callee.value
                && name == "super"
                && matches!(obj.value, Expr::Self_)
                && let Some(class) = current_class()
                && let Some((owner, sig)) = saule_semantic::super_init_target(&class)
            {
                check_user_method_args(
                    &format!("{owner}.init"),
                    &sig,
                    args,
                    scope,
                    errors,
                    expr.span.clone(),
                );
            }
            // `obj?.method(args)`. The `Expr::Call` arm dispatches on the
            // *shape* of the callee rather than walking into it, so a safe
            // method call is the one receiver position nothing infers — and
            // an uninferred node has no `TypeTable` entry, which leaves the
            // bytecode compiler unable to resolve the method's vtable slot
            // (§21.1 0.5). Inferring it here records the type; no diagnostic
            // is produced, so `check` and `check_with_types` still agree
            // byte for byte.
            if let Expr::SafeMember { obj, .. } = &callee.value {
                let _ = infer(obj, scope);
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
            super::ops::check_index_receiver(obj, index, scope, errors);
        }
        Expr::Unary { op, rhs } => {
            check_expr(rhs, scope, errors);
            // `-v` / `#v` on a class instance need an `OpNeg` / `OpLen`
            // overload; anything else is a runtime type error.
            super::ops::check_unary(*op, rhs, scope, errors);
        }
        Expr::Binary { op, lhs, rhs } => {
            check_expr(lhs, scope, errors);
            check_expr(rhs, scope, errors);
            check_binary_op(*op, lhs, rhs, scope, errors);
        }
        Expr::ForceUnwrap(inner) => check_expr(inner, scope, errors),
        // `x as T` reads two ways, and `casts::resolve` picks which — a
        // type test when the checker cannot know what `x` holds (`any`,
        // and a generic type parameter, which is exactly as unknown inside
        // the body: it stands for whatever the caller chose), a conversion
        // when it can. The two error rules are the ends of that spectrum:
        // a cast that would do nothing, and one with no reading at all.
        Expr::Cast { value, ty, .. } => {
            check_expr(value, scope, errors);
            // `x as function` used to be a bare callability test. The target
            // of a cast is a type like any other, so it has to name the
            // signature the value is being narrowed to.
            super::stmt::reject_non_types(ty, expr.span.clone(), errors);
            let source = infer(value, scope);
            let rule = crate::casts::resolve(source.as_ref(), ty);
            // Publish the decision before reporting on it: an erroring
            // module never runs, and recording unconditionally keeps the
            // "every cast the checker saw has a kind" invariant simple.
            crate::casts::record(expr.id, &rule);
            match rule {
                crate::casts::CastRule::Redundant => errors.push(TypeCheckError::RedundantCast {
                    found: type_to_string(source.as_ref().unwrap_or(ty)),
                    span: to_source_span(value.span.clone()),
                }),
                crate::casts::CastRule::Impossible => errors.push(TypeCheckError::ImpossibleCast {
                    from: type_to_string(source.as_ref().unwrap_or(ty)),
                    to: type_to_string(ty),
                    span: to_source_span(expr.span.clone()),
                }),
                _ => {}
            }
        }
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
        // No target type here, so the lambda's own annotation is the only
        // contract its body has — without it a declared `-> T` on a lambda
        // that isn't being assigned anywhere would go unchecked.
        Expr::Lambda {
            params,
            body,
            return_ty,
        } => {
            check_lambda_return_ty(return_ty.as_ref(), expr.span.clone(), errors);
            check_lambda_body(params, body, return_ty.as_ref(), scope, errors)
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

/// Check a lambda's body in a scope seeded with its parameters.
///
/// `expected_ret` is the return type the lambda's body is required to
/// produce: its own `-> T` annotation when it has one, otherwise the return
/// type of whatever target it is being checked against. `None` means neither
/// exists and the body's returns are unconstrained.
/// A lambda's own `-> T` annotation is a type ascription like any other, so
/// it goes through the same rejection as a declaration's.
fn check_lambda_return_ty(
    return_ty: Option<&Type>,
    span: std::ops::Range<usize>,
    errors: &mut Vec<TypeCheckError>,
) {
    if let Some(rt) = return_ty {
        super::stmt::reject_non_types(rt, span, errors);
    }
}

fn check_lambda_body(
    params: &[Param],
    body: &LambdaBody,
    expected_ret: Option<&Type>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    for p in params {
        super::stmt::check_binding_type(&p.ty, p.span.clone(), errors);
    }
    let mut lscope = scope.clone();
    seed_params(&mut lscope, params);
    // A `return` inside the lambda returns from the *lambda*, so the body is
    // walked under the lambda's own return type — `None` included. Inheriting
    // the enclosing function's would check these returns against a signature
    // they have nothing to do with.
    let prev_ret = set_return_ty(expected_ret.cloned());
    match body {
        LambdaBody::Expr(e) => {
            check_expr(e, &lscope, errors);
            // `x => expr` — the expression *is* the return value.
            if let Some(rt) = expected_ret {
                check_assignment_compat(rt, e, &lscope, errors);
            }
        }
        LambdaBody::Block(stmts) => {
            for s in stmts.iter() {
                check_stmt(s, &mut lscope, errors);
            }
        }
    }
    set_return_ty(prev_ret);
}

/// [`check_expr`], plus the type the expression is expected to produce.
///
/// The expectation matters for one construct: a lambda whose parameters were
/// written without types. Those parse as `any`, and the target's function
/// type is the only place their real types can come from — so
/// `local f: fn(integer) -> integer = fn(x) ... end` checks the body with
/// `x: integer` rather than `x: any`, and a misuse inside the body is caught.
/// Every other expression ignores the expectation and checks as usual.
pub(super) fn check_expr_expecting(
    expr: &Spanned<Expr>,
    expected: Option<&Type>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    if let Expr::Lambda {
        params,
        body,
        return_ty,
    } = &expr.value
        && let Some(Type::Function {
            params: want,
            ret: want_ret,
        }) = expected
    {
        check_lambda_return_ty(return_ty.as_ref(), expr.span.clone(), errors);
        let refined: Vec<Param> = params
            .iter()
            .enumerate()
            .map(|(i, p)| match want.get(i) {
                // Only fill in what the writer left off; an explicit
                // annotation on the lambda always wins.
                Some(t) if is_any(&p.ty) => Param {
                    ty: t.clone(),
                    ..p.clone()
                },
                _ => p.clone(),
            })
            .collect();
        // A declared return type governs the body: it is the contract the
        // `return` statements have to meet, and `types_compatible` separately
        // checks it against the target. Only fall back to the target's return
        // type when the lambda omitted one.
        let expected_ret = match return_ty {
            Some(rt) => Some(rt),
            None if is_any(want_ret) => None,
            None => Some(&**want_ret),
        };
        check_lambda_body(&refined, body, expected_ret, scope, errors);
        return;
    }
    check_expr(expr, scope, errors);
}

pub(super) fn check_arg(arg: &CallArg, scope: &Scope, errors: &mut Vec<TypeCheckError>) {
    check_arg_expecting(arg, None, scope, errors)
}

/// [`check_arg`] carrying the type the argument's slot expects, so a
/// lambda written without parameter types is checked as the callee
/// declared it rather than as `any`.
pub(super) fn check_arg_expecting(
    arg: &CallArg,
    expected: Option<&Type>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    match arg {
        CallArg::Positional(e) | CallArg::Named { value: e, .. } => {
            check_expr_expecting(e, expected, scope, errors)
        }
    }
}

pub(super) fn type_to_string(ty: &Type) -> String {
    match ty {
        // A call check renames the callee's type parameters apart from
        // the caller's; undo that here so a diagnostic quotes `T`, not
        // the internal `T$`.
        Type::Named(n) => unfreshen_name(n).to_string(),
        // A function under `?` needs parens: `fn() -> nil?` reads as a
        // function returning `nil?`, which is not the nullable function the
        // annotation declares — and a diagnostic has to quote a type the
        // reader can paste back into the source.
        Type::Nullable(inner) => match &**inner {
            Type::Function { .. } => format!("({})?", type_to_string(inner)),
            _ => format!("{}?", type_to_string(inner)),
        },
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
            // `->`, not the legacy `: R` — the parser stopped accepting that
            // spelling, so quoting it handed the reader a type they could not
            // write down.
            format!("fn({}) -> {}", parts.join(", "), type_to_string(ret))
        }
        Type::Generic(g) => {
            let parts: Vec<String> = g.args.iter().map(type_to_string).collect();
            // The head is unfreshened for the same reason a bare name is:
            // a diagnostic about `Box<T>` must not quote the internal `T$`.
            format!("{}<{}>", unfreshen_name(&g.name), parts.join(", "))
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
