//! Call-site checking: argument counts and types for natives, user
//! functions, methods and enum variants, plus the member-access
//! diagnostics (nullable receiver, private access, unknown member).

use saule_ast::{CallArg, Expr, Spanned, Type};

use crate::TypeCheckError;
use crate::funcs;
use crate::state::{Scope, current_class, lookup_member, with_classes, with_enums};
use crate::to_source_span;

use super::*;

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
pub(crate) fn check_native_args(
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
    let required: usize = sig.params.iter().take_while(|p| !is_nullable(p)).count();
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
    let mut subst: std::collections::HashMap<String, Type> = std::collections::HashMap::new();

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
        // Table literals are checked entry-by-entry — same rule as the
        // user-method path; see `check_table_literal_compat`.
        if !mentions_unbound_param(&expected, &sig.type_params)
            && check_table_literal_compat(&expected, value_expr, scope, errors)
        {
            continue;
        }
        if !compatible_under_sig_params(&expected, &found_ty, &sig.type_params) {
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
/// Named arguments are resolved to the parameter that carries the name —
/// both for the arity check (a named arg fills a required slot) and for the
/// per-slot type check.
///
/// Lenient in the same ways: `any` slots accept anything, nullable slots
/// accept anything, and we bail silently when `infer` can't produce a type.
pub(crate) fn check_user_method_args(
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
    // A required slot counts as filled either by its position or by a named
    // argument carrying its name — `Box(w: 5, h: 6)` fills both of
    // `fn init(w, h = 2)` even though it passes nothing positionally.
    let filled: usize = sig.params[..required]
        .iter()
        .enumerate()
        .filter(|(i, p)| {
            *i < positional.len()
                || args
                    .iter()
                    .any(|a| matches!(a, CallArg::Named { name, .. } if name == &p.name))
        })
        .count();
    if filled < required {
        errors.push(TypeCheckError::NativeArity {
            callee: callee_display.to_string(),
            expected: required,
            found: filled,
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
    let mut subst: std::collections::HashMap<String, Type> = std::collections::HashMap::new();

    // Positional args consume parameter slots left-to-right; a named arg
    // targets the slot that carries its name, so both forms get their type
    // checked (`Text(color: 3)` is as wrong as `Text(3)`).
    let mut next_slot = 0usize;
    for arg in args.iter() {
        let (p, value_expr, i) = match arg {
            CallArg::Positional(e) => {
                let slot = next_slot;
                next_slot += 1;
                let Some(p) = sig.params.get(slot).or_else(|| {
                    if has_variadic {
                        sig.params.last()
                    } else {
                        None
                    }
                }) else {
                    break;
                };
                (p, e, slot)
            }
            CallArg::Named { name, value } => {
                // Unknown names are already reported above; skip quietly.
                let Some((slot, p)) = sig.params.iter().enumerate().find(|(_, p)| &p.name == name)
                else {
                    continue;
                };
                (p, value, slot)
            }
        };
        if p.variadic {
            continue;
        }
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
        // A table literal is checked entry-by-entry against the declared
        // shape instead of being inferred and compared whole — see
        // `check_table_literal_compat`. Runs after `unify` so a generic
        // slot still gets bound from the argument first, and is skipped
        // while the expected element type is still an unbound parameter.
        if !mentions_unbound_param(&expected, type_params)
            && check_table_literal_compat(&expected, value_expr, scope, errors)
        {
            continue;
        }
        if !compatible_under_sig_params(&expected, &found_ty, type_params) {
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

/// Resolve a semantic method's return type, substituting any generic type
/// parameters bound from the call's actual positional arguments. Non-generic
/// methods return their declared `return_ty` unchanged.
pub(crate) fn semantic_method_return(
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

pub(crate) fn report_if_nullable_receiver(
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
pub(crate) fn report_if_private(
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
pub(crate) fn report_if_unknown_member(
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
                                span: to_source_span(obj.span.end..obj.span.end + member.len() + 1),
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
pub(crate) fn report_if_enum_variant_arity(
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
            .and_then(|info| info.variants.get(variant).map(|v| v.arity()))
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

/// Check a call made *through a value* of function type — a parameter,
/// local or loop variable declared `fn(A, B) -> R` — rather than through
/// a name the checker can resolve to a declaration.
///
/// Nothing else covers this shape: `report_if_user_function_arity` looks
/// the name up in the top-level function table, and the native path
/// looks it up in the signature registry, so a callable *binding* fell
/// through both and its call was never checked at all — not arity, not
/// argument types. That is the hole that let
/// `fn map<T, U>(items: table<T>, f: fn(U) -> U)` apply `f` to a `T`
/// without complaint.
///
/// Deliberately narrow:
///
/// * Only the `Expr::Ident` callee shape. `obj.field(...)` parses as a
///   member call and is handled by the class-method paths above.
/// * A bare `function` annotation (as opposed to `fn(...) -> ...`)
///   carries no parameter list, so there is nothing to check.
/// * Named arguments are skipped — a function *type* records no
///   parameter names, so a named argument can't be matched to a slot.
///   The runtime still rejects it.
pub(crate) fn report_if_function_value_call(
    callee: &Spanned<Expr>,
    args: &[CallArg],
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
    span: std::ops::Range<usize>,
) {
    let Expr::Ident(name) = &callee.value else {
        return;
    };
    // A declaration of the same name wins: the binding paths above have
    // already checked the call, and re-checking would double-report.
    if funcs::lookup(name).is_some() || crate::sigs::lookup(name).is_some() {
        return;
    }
    let Some(ty) = scope.lookup(name) else {
        return;
    };
    let Type::Function {
        params: expected_params,
        ..
    } = strip_nullable(ty.clone())
    else {
        return;
    };
    if args.iter().any(|a| matches!(a, CallArg::Named { .. })) {
        return;
    }

    // A function type states its arity exactly — there are no defaults
    // and no variadic slot to write in one.
    if args.len() != expected_params.len() {
        errors.push(TypeCheckError::FunctionArity {
            callee: name.clone(),
            expected: expected_params.len(),
            found: args.len(),
            span: to_source_span(span),
        });
        return;
    }

    for (i, (arg, expected)) in args.iter().zip(expected_params.iter()).enumerate() {
        let CallArg::Positional(value_expr) = arg else {
            continue;
        };
        if is_any(expected) {
            continue;
        }
        let Some(found_ty) = infer(value_expr, scope) else {
            continue;
        };
        if !is_nullable(expected) && is_nullable(&found_ty) {
            errors.push(TypeCheckError::NativeArgTypeMismatch {
                callee: name.clone(),
                arg: i + 1,
                expected: type_to_string(expected),
                found: type_to_string(&found_ty),
                span: to_source_span(value_expr.span.clone()),
            });
            continue;
        }
        if !types_compatible(expected, &found_ty) {
            errors.push(TypeCheckError::NativeArgTypeMismatch {
                callee: name.clone(),
                arg: i + 1,
                expected: type_to_string(expected),
                found: type_to_string(&found_ty),
                span: to_source_span(value_expr.span.clone()),
            });
        }
    }
}

/// Emit [`TypeCheckError::FunctionArity`] for direct calls to top-level
/// user-defined functions when the supplied positional-argument count
/// can't match the declared signature.
pub(crate) fn report_if_user_function_arity(
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

    // Arity check. Either branch reports and returns on failure, so
    // reaching past this point means the count is good.
    let positional = args.len();
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
    }

    // Argument-type validation. Mirrors `check_user_method_args` /
    // `check_native_args`: walks left-to-right, unifying generic type
    // parameters as we go, then checks each slot for compatibility.
    let type_params = &info.type_params;
    let mut subst: std::collections::HashMap<String, Type> = std::collections::HashMap::new();

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
        if !compatible_under_sig_params(&expected, &found_ty, type_params) {
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
