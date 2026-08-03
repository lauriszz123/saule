//! `when(source):stage1():stage2()` pipelines.
//!
//! A stage is an ordinary call with the piped value spliced in as argument
//! 0, so each stage binds its type parameters from the value that reaches
//! it and hands the *instantiated* result to the next stage.

use saule_ast::{CallArg, Expr, PipeStage, Spanned, Type};

use crate::TypeCheckError;
use crate::funcs;
use crate::state::Scope;
use crate::to_source_span;

use super::*;

/// Bind a pipeline stage's type parameters from the value flowing into it
/// and from its explicit arguments.
///
/// A stage call is a normal call with the piped value spliced in as arg 0,
/// so it deserves the same generic instantiation every other call site
/// gets. Without it `filter<T>(items: table<T>, …)` compared its declared
/// `table<T>` against the incoming `table<integer>` as if `T` were a
/// concrete type nobody had ever heard of, and rejected the stage.
///
/// Returns the substitution; `params[0]` binds against `incoming`, and the
/// explicit args fill `params[1..]` (by position, or by name for a named
/// argument). Each parameter is substituted with what's already bound
/// before it unifies, so `map<T, U>(items: table<T>, f: fn(T) -> U)` sees
/// `f`'s slot as `fn(integer) -> U` once `T` is pinned down.
pub(crate) fn bind_stage_generics(
    info: &funcs::FunctionInfo,
    incoming: Option<&Type>,
    args: &[CallArg],
    scope: &Scope,
) -> std::collections::HashMap<String, Type> {
    let mut subst = std::collections::HashMap::new();
    if info.type_params.is_empty() {
        return subst;
    }
    if let (Some(first), Some(actual)) = (info.params.first(), incoming) {
        unify(&first.ty, actual, &info.type_params, &mut subst);
    }
    // The piped value already took slot 0, so explicit args start at 1.
    let mut next = 1usize;
    for arg in args {
        let param = match arg {
            CallArg::Positional(_) => {
                let p = info.params.get(next);
                next += 1;
                p
            }
            CallArg::Named { name, .. } => info.params.iter().find(|p| &p.name == name),
        };
        let (Some(param), CallArg::Positional(e) | CallArg::Named { value: e, .. }) = (param, arg)
        else {
            continue;
        };
        let expected = substitute(&param.ty, &subst, &info.type_params);
        if let Some(found) = infer(e, scope) {
            unify(&expected, &found, &info.type_params, &mut subst);
        }
    }
    subst
}

/// A stage's return type with [`bind_stage_generics`]'s bindings applied.
///
/// A type parameter nothing pinned down would come back as its own bare
/// name (`table<U>`), which isn't a type the user wrote or can act on —
/// report it as "unknown" rather than letting the parameter name escape.
pub(crate) fn stage_return(
    info: &funcs::FunctionInfo,
    subst: &std::collections::HashMap<String, Type>,
) -> Option<Type> {
    let ret = substitute(info.return_ty.as_ref()?, subst, &info.type_params);
    (!mentions_unbound_param(&ret, &info.type_params)).then_some(ret)
}

/// The type a `when(source):a():b()` chain produces: the value type
/// threaded through every stage, so each stage's generics are bound from
/// what actually reaches it rather than left as declared parameter names.
pub(crate) fn infer_pipe(
    source: &Spanned<Expr>,
    stages: &[PipeStage],
    scope: &Scope,
) -> Option<Type> {
    let mut current = infer(source, scope);
    for stage in stages {
        let info = funcs::lookup(&stage.name)?;
        let subst = bind_stage_generics(&info, current.as_ref(), &stage.args, scope);
        current = stage_return(&info, &subst);
    }
    current
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
///   * advance the "current type" to the function's *instantiated* return
///     type for the next stage.
pub(crate) fn check_pipe(
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
        let Some(info) = crate::funcs::lookup(&stage.name) else {
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
            current = stage_return(&info, &std::collections::HashMap::new());
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

        // Bind the stage's type parameters before comparing anything, so a
        // generic stage is checked as the caller actually instantiated it.
        let subst = bind_stage_generics(&info, current.as_ref(), &stage.args, scope);

        // First-arg type check — the headline pipeline diagnostic.
        if let (Some(actual), Some(expected_param)) = (current.as_ref(), info.params.first()) {
            let expected = substitute(&expected_param.ty, &subst, &info.type_params);
            let actual_base = strip_nullable(actual.clone());
            let expected_base = strip_nullable(expected.clone());
            let skip = is_any(&actual_base)
                || is_any(&expected_base)
                || matches!(&actual_base, Type::Named(n) if n == "nil");
            // Any type parameter still unbound is a slot the piped value is
            // free to define, so it must not be read as a concrete name that
            // matches nothing — `compatible_under_sig_params` scopes those
            // names in for the comparison exactly as the call path does.
            if !skip && !compatible_under_sig_params(&expected, actual, &info.type_params) {
                errors.push(TypeCheckError::PipeStageTypeMismatch {
                    stage: stage.name.clone(),
                    expected: type_to_string(&expected),
                    found: type_to_string(actual),
                    span: to_source_span(stage.span.clone()),
                });
            }
        }

        // Thread the instantiated return type into the next stage (so the
        // chain type-checks transitively). `None` propagates as "unknown"
        // and simply disables the next first-arg comparison.
        current = stage_return(&info, &subst);
    }
}
