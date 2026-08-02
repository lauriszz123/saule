//! `match` expression checks: pattern/scrutinee compat, exhaustiveness,
//! arm-body type unification. Pattern-bound variables are added to a per-arm
//! scope so guards and bodies can reference them.

use std::collections::HashSet;
use std::ops::Range;

use saule_ast::{Expr, MatchArm, MatchBody, Pattern, Spanned, Type};

use super::TypeCheckError;
use super::expr::{
    check_boolean_cond, check_expr, infer, is_nullable, strip_nullable, type_to_string,
    types_compatible,
};
use super::state::{Scope, is_type_param, with_enums};
use super::stmt::check_stmt;
use super::to_source_span;

pub(super) fn check_match(
    match_expr: &Spanned<Expr>,
    scrutinee: &Spanned<Expr>,
    arms: &[MatchArm],
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    let _ = is_nullable; // keep import predictable for future use
    check_expr(scrutinee, scope, errors);
    let scrut_ty = infer(scrutinee, scope);

    // A function-typed scrutinee is almost always a missing call —
    // `match obj.method case true ...` instead of `obj.method()`. Surface
    // a targeted diagnostic and skip exhaustiveness (which would also
    // complain, but less usefully).
    let scrut_is_function = matches!(
        scrut_ty.as_ref().map(|t| strip_nullable(t.clone())),
        Some(Type::Function { .. })
    );
    if scrut_is_function {
        errors.push(TypeCheckError::MatchOnFunction {
            span: to_source_span(scrutinee.span.clone()),
        });
    }

    // Determine which enum (if any) drives exhaustiveness — prefer the
    // scrutinee's static type; fall back to the enum referenced by any
    // `Variant` pattern in the arms.
    let scrut_enum_name = match &scrut_ty {
        Some(ty) => match strip_nullable(ty.clone()) {
            Type::Named(n) if with_enums(|e| e.contains_key(&n)) => Some(n),
            _ => None,
        },
        None => None,
    };
    let pattern_enum_name = arms.iter().find_map(|a| match &a.pattern.value {
        Pattern::Variant { enum_name, .. } if with_enums(|e| e.contains_key(enum_name)) => {
            Some(enum_name.clone())
        }
        _ => None,
    });
    let active_enum = scrut_enum_name.clone().or(pattern_enum_name);

    let mut covered_variants: HashSet<String> = HashSet::new();
    let mut has_fallback = false;
    let mut covered_true = false;
    let mut covered_false = false;
    let mut arm_types: Vec<Option<Type>> = Vec::new();
    let arm_bind_ty = scrut_ty.as_ref().map(|t| strip_nullable(t.clone()));

    for arm in arms {
        check_pattern(&arm.pattern, &scrut_ty, errors);

        let mut arm_scope = scope.clone();
        bind_pattern(&arm.pattern.value, arm_bind_ty.as_ref(), &mut arm_scope);

        if let Some(g) = &arm.guard {
            check_expr(g, &arm_scope, errors);
            check_boolean_cond("when", g, &arm_scope, errors);
        }

        let body_ty = match &arm.body {
            MatchBody::Expr(e) => {
                check_expr(e, &arm_scope, errors);
                infer(e, &arm_scope)
            }
            MatchBody::Block(stmts) => {
                let mut bs = arm_scope.clone();
                for s in stmts {
                    check_stmt(s, &mut bs, errors);
                }
                None
            }
        };
        arm_types.push(body_ty);

        // Only unguarded arms contribute to exhaustiveness.
        if arm.guard.is_none() {
            match &arm.pattern.value {
                Pattern::Wildcard | Pattern::Bind(_) => has_fallback = true,
                Pattern::Tuple(fields)
                    if fields
                        .iter()
                        .all(|p| matches!(p.value, Pattern::Wildcard | Pattern::Bind(_))) =>
                {
                    has_fallback = true;
                }
                Pattern::Variant {
                    enum_name, variant, ..
                } => {
                    if let Some(en) = &active_enum
                        && en == enum_name
                    {
                        covered_variants.insert(variant.clone());
                    }
                }
                Pattern::Bool(true) => covered_true = true,
                Pattern::Bool(false) => covered_false = true,
                _ => {}
            }
        }
    }

    // Exhaustiveness.
    let (exhaustive, missing_variants): (bool, Vec<String>) = if has_fallback {
        (true, vec![])
    } else if let Some(en) = &active_enum {
        with_enums(|e| {
            if let Some(info) = e.get(en) {
                let missing: Vec<String> = info
                    .variants
                    .keys()
                    .filter(|v| !covered_variants.contains(*v))
                    .cloned()
                    .collect();
                (missing.is_empty(), missing)
            } else {
                (false, vec![])
            }
        })
    } else if matches!(&scrut_ty, Some(Type::Named(n)) if n == "boolean")
        || (scrut_ty.is_none() && all_arms_bool_literals(arms))
    {
        // Either we know the scrutinee is `boolean`, or we couldn't
        // infer it but every arm is a bool literal — in which case the
        // user's intent is unambiguous and `true + false` exhausts it.
        (covered_true && covered_false, vec![])
    } else {
        (false, vec![])
    };

    if !exhaustive && !scrut_is_function {
        let reason = if let Some(en) = &active_enum {
            if missing_variants.is_empty() {
                format!("enum `{en}` is not fully covered")
            } else {
                format!(
                    "missing variant(s) of `{en}`: {}",
                    missing_variants.join(", ")
                )
            }
        } else if matches!(&scrut_ty, Some(Type::Named(n)) if n == "boolean")
            || (scrut_ty.is_none() && all_arms_bool_literals(arms))
        {
            "boolean match must cover both `true` and `false`".to_string()
        } else {
            "no unguarded wildcard / binding arm".to_string()
        };
        errors.push(TypeCheckError::MatchNonExhaustive {
            reason,
            span: to_source_span(match_expr.span.clone()),
        });
    }

    // Arm body type unification — flag the first pair that don't agree.
    let mut first_ty: Option<Type> = None;
    for ty_opt in &arm_types {
        if let Some(t) = ty_opt {
            if let Some(first) = &first_ty {
                if !types_compatible(first, t) && !types_compatible(t, first) {
                    errors.push(TypeCheckError::MatchArmTypeMismatch {
                        expected: type_to_string(first),
                        found: type_to_string(t),
                        span: to_source_span(match_expr.span.clone()),
                    });
                    break;
                }
            } else {
                first_ty = Some(t.clone());
            }
        }
    }
}

/// True when every arm pattern is an unguarded `case true` / `case false`
/// literal. Used by exhaustiveness to recognise an obviously-boolean
/// scrutinee even when type inference couldn't prove it — e.g.
/// `match maybeBool() case true ... case false ... end` where the
/// callee's return type isn't statically known.
fn all_arms_bool_literals(arms: &[MatchArm]) -> bool {
    !arms.is_empty()
        && arms
            .iter()
            .all(|a| a.guard.is_none() && matches!(&a.pattern.value, Pattern::Bool(_)))
}

fn check_pattern(
    pat: &Spanned<Pattern>,
    scrut_ty: &Option<Type>,
    errors: &mut Vec<TypeCheckError>,
) {
    match &pat.value {
        Pattern::Wildcard | Pattern::Bind(_) | Pattern::Nil => {}
        Pattern::Int(_) => check_pattern_literal_compat(scrut_ty, "integer", &pat.span, errors),
        Pattern::Float(_) => check_pattern_literal_compat(scrut_ty, "float", &pat.span, errors),
        Pattern::Bool(_) => check_pattern_literal_compat(scrut_ty, "boolean", &pat.span, errors),
        Pattern::Str(_) => check_pattern_literal_compat(scrut_ty, "string", &pat.span, errors),
        Pattern::Variant {
            enum_name,
            variant,
            fields,
        } => {
            let mut known_enum = false;
            with_enums(|e| {
                if let Some(info) = e.get(enum_name) {
                    known_enum = true;
                    if let Some(arity) = info.variants.get(variant).map(|v| v.arity()) {
                        if fields.len() != arity {
                            errors.push(TypeCheckError::MatchVariantArityMismatch {
                                variant: format!("{enum_name}.{variant}"),
                                expected: arity,
                                found: fields.len(),
                                span: to_source_span(pat.span.clone()),
                            });
                        }
                    } else {
                        errors.push(TypeCheckError::MatchUnknownVariant {
                            enum_name: enum_name.clone(),
                            variant: variant.clone(),
                            span: to_source_span(pat.span.clone()),
                        });
                    }
                }
            });
            let _ = known_enum;
            for f in fields {
                check_pattern(f, &None, errors);
            }
        }
        Pattern::Tuple(fields) => {
            for f in fields {
                check_pattern(f, &None, errors);
            }
        }
    }
}

fn check_pattern_literal_compat(
    scrut_ty: &Option<Type>,
    pat_ty_name: &str,
    span: &Range<usize>,
    errors: &mut Vec<TypeCheckError>,
) {
    let Some(ty) = scrut_ty else {
        return;
    };
    let stripped = strip_nullable(ty.clone());
    if let Type::Named(n) = &stripped {
        if n == "any" || n == pat_ty_name || is_type_param(n) {
            return;
        }
        // `number` accepts integer/float literals.
        if n == "number" && (pat_ty_name == "integer" || pat_ty_name == "float") {
            return;
        }
        errors.push(TypeCheckError::MatchPatternTypeMismatch {
            expected: n.clone(),
            found: pat_ty_name.to_string(),
            span: to_source_span(span.clone()),
        });
    }
}

/// Bind the variables introduced by a pattern into `scope`.
///
/// A top-level bind takes the scrutinee's type, and a variant's payload binds
/// take the types declared on the variant — so `case Shape.Rect(w, h)` gives
/// `w` and `h` whatever `Rect(w: float, h: float)` said they were, and using
/// them as floats needs no cast. Anything the declaration cannot answer for
/// (a tuple sub-pattern, an unknown enum) still falls back to `any`.
fn bind_pattern(pat: &Pattern, scrut_ty: Option<&Type>, scope: &mut Scope) {
    match pat {
        Pattern::Bind(name) => {
            let ty = scrut_ty.cloned().unwrap_or(Type::Named("any".into()));
            scope.bind(name.clone(), ty);
        }
        Pattern::Variant {
            enum_name,
            variant,
            fields,
        } => {
            let declared: Vec<Option<Type>> = with_enums(|e| {
                let shape = e.get(enum_name).and_then(|info| info.variants.get(variant));
                (0..fields.len())
                    .map(|i| shape.and_then(|s| s.field_ty(i)).cloned())
                    .collect()
            });

            for (f, ty) in fields.iter().zip(declared) {
                bind_pattern(&f.value, ty.as_ref(), scope);
            }
        }
        Pattern::Tuple(fields) => {
            for f in fields {
                bind_pattern(&f.value, None, scope);
            }
        }
        _ => {}
    }
}
