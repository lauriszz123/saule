//! Expression-level checks: nullable-receiver detection, private-member
//! access, native-call argument checking, lightweight type inference, and
//! the `?? != nil` style flow narrowing.

mod calls;
mod compat;
mod generics;
mod infer;
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
use super::state::{Scope, current_class, is_type_param, set_return_ty, with_classes};
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
            if let Some(qname) = native_callee_name(callee, scope) {
                if let Some(sig) = crate::sigs::lookup(&qname) {
                    check_native_args(&qname, &sig, args, scope, errors, expr.span.clone());
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
        // `x as T` is only meaningful when the checker cannot know what
        // `x` holds. That is `any` — and a generic type parameter, which
        // is exactly as unknown inside the body: it stands for whatever
        // the caller chose. Since a rigid `T` no longer flows into a
        // concrete slot on its own, the cast is the only checked way to
        // narrow one, and rejecting it would leave no way at all.
        //
        // On an already-typed value the cast is noise at best and a false
        // sense of safety at worst, so say so rather than allow it.
        Expr::Cast { value, .. } => {
            check_expr(value, scope, errors);
            if let Some(vt) = infer(value, scope) {
                let base = strip_nullable(vt.clone());
                let narrowable =
                    is_any(&base) || matches!(&base, Type::Named(n) if is_type_param(n));
                if !narrowable {
                    errors.push(TypeCheckError::RedundantCast {
                        found: type_to_string(&vt),
                        span: to_source_span(value.span.clone()),
                    });
                }
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
        Expr::Lambda { params, body, .. } => check_lambda_body(params, body, None, scope, errors),
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
/// `expected_ret` is the return type the lambda is required to produce, and
/// is only supplied when the lambda itself omitted one — a block-bodied
/// lambda's return type is otherwise unknown, and comparing the target
/// against that unknown would accept anything.
fn check_lambda_body(
    params: &[Param],
    body: &LambdaBody,
    expected_ret: Option<&Type>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    for p in params {
        super::stmt::reject_nil_in_binding_type(&p.ty, p.span.clone(), errors);
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
        // A lambda that declared its own return type is compared against the
        // target by `types_compatible`; only fill one in when it omitted one.
        let expected_ret = match return_ty {
            Some(_) => None,
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
