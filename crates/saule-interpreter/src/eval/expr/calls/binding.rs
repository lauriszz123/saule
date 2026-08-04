//! Matching a call's arguments to a callee's parameters: positional
//! and named args, defaults, variadics, and the arity errors.

use crate::env::Environment;
use crate::error::RuntimeError;
use crate::eval::expr::{EvaluatedArg, eval, is_nullable_type};
use crate::value::{FunctionObject, Value};
use saule_ast::CallArg;
use std::cell::RefCell;
use std::rc::Rc;

/// Flatten a native closure's arguments into positional values.
///
/// When `param_names` is empty the closure only accepts positional
/// arguments, so any named argument is an error (preserving the historical
/// behaviour for stdlib closures). When `param_names` is non-empty (dynamic
/// native packages, which declare their parameter names in the manifest)
/// named arguments are reordered into their declared slots; gaps left by
/// omitted optional parameters are filled with `nil`.
pub(crate) fn native_positional_args(
    name: &str,
    param_names: &[String],
    args: &[EvaluatedArg],
    span: &std::ops::Range<usize>,
) -> Result<Vec<Value>, RuntimeError> {
    let has_named = args.iter().any(|a| matches!(a, EvaluatedArg::Named { .. }));

    if !has_named {
        return Ok(args
            .iter()
            .map(|a| match a {
                EvaluatedArg::Positional(v) | EvaluatedArg::TrailingBlock(v) => v.clone(),
                // No named args present, so this branch is unreachable, but
                // map exhaustively rather than panic.
                EvaluatedArg::Named { value, .. } => value.clone(),
            })
            .collect());
    }

    if param_names.is_empty() {
        return Err(RuntimeError::TypeError {
            message: format!(
                "named arguments are not supported for built-in function `{name}` — use positional arguments instead"
            ),
            span: span.clone(),
        });
    }

    // Reorder into declared slots. Positional args fill left-to-right; named
    // args drop into the slot matching their name. Positional-after-named is
    // rejected, matching user-function call rules.
    let mut slots: Vec<Option<Value>> = vec![None; param_names.len()];
    let mut next_positional = 0usize;
    let mut seen_named = false;

    for arg in args {
        match arg {
            EvaluatedArg::Positional(value) => {
                if seen_named {
                    return Err(RuntimeError::TypeError {
                        message: format!(
                            "positional argument cannot follow named arguments in call to `{name}` — all positional arguments must come first"
                        ),
                        span: span.clone(),
                    });
                }
                if next_positional >= slots.len() {
                    return Err(RuntimeError::TypeError {
                        message: format!(
                            "too many arguments to `{name}`: expected at most {} but got more",
                            param_names.len()
                        ),
                        span: span.clone(),
                    });
                }
                slots[next_positional] = Some(value.clone());
                next_positional += 1;
            }
            // Written after the parentheses, so it is exempt from the
            // positional-before-named rule and binds to the last parameter.
            EvaluatedArg::TrailingBlock(value) => {
                let Some(idx) = slots.len().checked_sub(1) else {
                    return Err(RuntimeError::TypeError {
                        message: format!(
                            "`{name}` takes no parameters, so it cannot take a trailing block"
                        ),
                        span: span.clone(),
                    });
                };
                if slots[idx].is_some() {
                    return Err(RuntimeError::TypeError {
                        message: format!(
                            "duplicate argument for parameter `{}` — a trailing block binds to the last parameter, which was already supplied",
                            param_names[idx]
                        ),
                        span: span.clone(),
                    });
                }
                slots[idx] = Some(value.clone());
            }
            EvaluatedArg::Named {
                name: arg_name,
                value,
            } => {
                seen_named = true;
                let Some(idx) = param_names.iter().position(|p| p == arg_name) else {
                    return Err(RuntimeError::TypeError {
                        message: format!(
                            "unknown named argument `{arg_name}` for `{name}` — valid parameters: {}",
                            param_names.join(", ")
                        ),
                        span: span.clone(),
                    });
                };
                if slots[idx].is_some() {
                    return Err(RuntimeError::TypeError {
                        message: format!(
                            "argument `{arg_name}` specified more than once in call to `{name}`"
                        ),
                        span: span.clone(),
                    });
                }
                slots[idx] = Some(value.clone());
            }
        }
    }

    // Trim trailing unfilled (optional) slots; fill interior gaps with nil so
    // a later named arg still lands in the right position.
    let last_filled = slots.iter().rposition(|s| s.is_some());
    let len = last_filled.map_or(0, |i| i + 1);
    Ok(slots
        .into_iter()
        .take(len)
        .map(|s| s.unwrap_or(Value::Nil))
        .collect())
}

/// Bind `params` into `scope` from `args`, evaluating defaults lazily. Used
/// by every kind of call (free function, instance method, static method,
/// constructor) so the rules stay in one place.
pub(crate) fn bind_params(
    scope: &Rc<RefCell<Environment>>,
    params: &[saule_ast::Param],
    keys: &[Rc<str>],
    args: &[EvaluatedArg],
    span: &std::ops::Range<usize>,
) -> Result<(), RuntimeError> {
    // Locate the variadic parameter (and reject a second one) without
    // building a `Vec` — this runs on every single call.
    let mut variadic_idx = None;
    for (i, p) in params.iter().enumerate() {
        if p.variadic {
            if variadic_idx.is_some() {
                return Err(RuntimeError::TypeError {
                    message: "a function can declare at most one variadic parameter".to_string(),
                    span: p.span.clone(),
                });
            }
            variadic_idx = Some(i);
        }
    }
    if let Some(idx) = variadic_idx
        && idx + 1 != params.len()
    {
        return Err(RuntimeError::TypeError {
            message: format!(
                "variadic parameter `{}` must be the last parameter in the list",
                params[idx].name
            ),
            span: params[idx].span.clone(),
        });
    }

    // Fast path: plain positional call against a non-variadic signature —
    // the shape of nearly every call in a program. Arguments map onto
    // parameters by index, so there's nothing to reorder and none of the
    // buffers the general path needs.
    // A trailing block sits at its positional index only when the call fills
    // every parameter; otherwise it has to be reordered onto the last slot, and
    // the general path below does that.
    if variadic_idx.is_none()
        && !args.iter().any(|a| matches!(a, EvaluatedArg::Named { .. }))
        && (args.len() == params.len()
            || !args
                .iter()
                .any(|a| matches!(a, EvaluatedArg::TrailingBlock(_))))
    {
        if args.len() > params.len() {
            return Err(RuntimeError::TypeError {
                message: format!(
                    "too many arguments: expected {} but got at least {}",
                    params.len(),
                    params.len() + 1
                ),
                span: span.clone(),
            });
        }
        for (i, param) in params.iter().enumerate() {
            let value = match args.get(i) {
                Some(EvaluatedArg::Positional(v) | EvaluatedArg::TrailingBlock(v)) => v.clone(),
                // `any(Named)` above ruled this out; map it exhaustively
                // rather than panic.
                Some(EvaluatedArg::Named { value, .. }) => value.clone(),
                None => match &param.default {
                    Some(default) => eval(default, scope)?,
                    None if is_nullable_type(&param.ty) => Value::Nil,
                    None => {
                        return Err(missing_argument_error(
                            params,
                            |j| j < args.len(),
                            param,
                            span,
                        ));
                    }
                },
            };
            scope.borrow_mut().define(Rc::clone(&keys[i]), value);
        }
        return Ok(());
    }

    let mut assigned: Vec<Option<Value>> = vec![None; params.len()];
    let mut variadic_values: Vec<Value> = Vec::new();
    let mut next_positional = 0usize;
    let mut seen_named = false;
    let mut named_arg_count = 0usize;

    for arg in args {
        match arg {
            EvaluatedArg::Positional(value) => {
                if seen_named {
                    return Err(RuntimeError::TypeError {
                        message: format!(
                            "positional argument #{} cannot follow named arguments; all positional arguments must come first",
                            next_positional + 1
                        ),
                        span: span.clone(),
                    });
                }
                if let Some(var_idx) = variadic_idx
                    && next_positional >= var_idx
                {
                    variadic_values.push(value.clone());
                    next_positional += 1;
                    continue;
                }
                if next_positional >= params.len() {
                    return Err(RuntimeError::TypeError {
                        message: format!(
                            "too many arguments: expected {} but got at least {}",
                            params.len(),
                            next_positional + 1
                        ),
                        span: span.clone(),
                    });
                }
                assigned[next_positional] = Some(value.clone());
                next_positional += 1;
            }
            // See `saule_ast::resolve_arg_slots`: a trailing block binds to
            // the callee's last parameter, so `panel(title: "Stats") do … end`
            // reaches `body` even when a defaulted `spacing` sits in between.
            EvaluatedArg::TrailingBlock(value) => {
                let Some(idx) = params.len().checked_sub(1) else {
                    return Err(RuntimeError::TypeError {
                        message:
                            "this function takes no parameters, so it cannot take a trailing block"
                                .to_string(),
                        span: span.clone(),
                    });
                };
                if assigned[idx].is_some() {
                    return Err(RuntimeError::TypeError {
                        message: format!(
                            "duplicate argument for parameter `{}` — a trailing block binds to the last parameter, which was already supplied",
                            params[idx].name
                        ),
                        span: span.clone(),
                    });
                }
                assigned[idx] = Some(value.clone());
            }
            EvaluatedArg::Named { name, value } => {
                seen_named = true;
                named_arg_count += 1;
                let Some(idx) = params.iter().position(|p| p.name == *name) else {
                    let valid_params: Vec<String> = params.iter().map(|p| p.name.clone()).collect();
                    return Err(RuntimeError::TypeError {
                        message: format!(
                            "unknown named argument `{name}` (argument #{} of {}) — valid parameters: {}",
                            named_arg_count,
                            args.len(),
                            valid_params.join(", ")
                        ),
                        span: span.clone(),
                    });
                };
                if params[idx].variadic {
                    return Err(RuntimeError::TypeError {
                        message: format!(
                            "variadic parameter `{name}` cannot be passed by name — provide any extra arguments positionally"
                        ),
                        span: span.clone(),
                    });
                }
                if assigned[idx].is_some() {
                    return Err(RuntimeError::TypeError {
                        message: format!(
                            "duplicate argument for parameter `{name}` — this parameter was already provided"
                        ),
                        span: span.clone(),
                    });
                }
                assigned[idx] = Some(value.clone());
            }
        }
    }

    for (i, param) in params.iter().enumerate() {
        if param.variadic {
            scope.borrow_mut().define(
                Rc::clone(&keys[i]),
                Value::Table(Rc::new(RefCell::new(
                    crate::value::TableObject::from_array(variadic_values.clone()),
                ))),
            );
            continue;
        }
        let value = if let Some(v) = assigned[i].clone() {
            v
        } else if let Some(default) = &param.default {
            eval(default, scope)?
        } else if is_nullable_type(&param.ty) {
            Value::Nil
        } else {
            return Err(missing_argument_error(
                params,
                |j| assigned[j].is_some(),
                param,
                span,
            ));
        };
        scope.borrow_mut().define(Rc::clone(&keys[i]), value);
    }
    Ok(())
}

/// Build the "missing required argument" diagnostic.
///
/// Listing every unfilled parameter means walking the whole signature, so
/// this is deliberately only reached on the failure path — the happy path
/// never pays for it. `is_filled` reports whether parameter `i` received an
/// argument, which the positional and reorder paths answer differently.
pub(crate) fn missing_argument_error(
    params: &[saule_ast::Param],
    is_filled: impl Fn(usize) -> bool,
    param: &saule_ast::Param,
    span: &std::ops::Range<usize>,
) -> RuntimeError {
    let missing: Vec<&str> = params
        .iter()
        .enumerate()
        .filter(|(i, p)| {
            !is_filled(*i) && !p.variadic && p.default.is_none() && !is_nullable_type(&p.ty)
        })
        .map(|(_, p)| p.name.as_str())
        .collect();

    let (message, help) = if missing.len() == 1 {
        (
            format!("missing required argument for parameter `{}`", param.name),
            format!(
                "add argument `{}` to this call (positionally or by name)",
                missing[0]
            ),
        )
    } else {
        (
            format!(
                "missing {} required argument(s): {}",
                missing.len(),
                missing.join(", ")
            ),
            format!(
                "add required arguments to this call: {}",
                missing
                    .iter()
                    .map(|n| format!("`{n}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
    };

    RuntimeError::ArgumentError {
        message,
        help: Some(help),
        span: span.clone(),
    }
}

/// Index of the argument that binds as a trailing block rather than by
/// position: the final argument, when it is a block-bodied lambda. It targets
/// the callee's last parameter, which coincides with its positional index only
/// when the call supplies every parameter.
pub(crate) fn trailing_block_index(args: &[CallArg]) -> Option<usize> {
    args.len()
        .checked_sub(1)
        .filter(|&i| args[i].is_trailing_block())
}

pub(crate) fn eval_call_args(
    args: &[CallArg],
    env: &Rc<RefCell<Environment>>,
) -> Result<Vec<EvaluatedArg>, RuntimeError> {
    let mut out = Vec::with_capacity(args.len());
    let trailing = trailing_block_index(args);
    for (i, arg) in args.iter().enumerate() {
        match arg {
            CallArg::Positional(expr) if Some(i) == trailing => {
                out.push(EvaluatedArg::TrailingBlock(eval(expr, env)?))
            }
            CallArg::Positional(expr) => out.push(EvaluatedArg::Positional(eval(expr, env)?)),
            CallArg::Named { name, value } => out.push(EvaluatedArg::Named {
                name: name.clone(),
                value: eval(value, env)?,
            }),
        }
    }
    Ok(out)
}

/// Skip a leading `self` parameter (so explicit-self style still works
/// alongside the implicit-self style this language prefers).
pub(crate) fn user_params(f: &FunctionObject) -> &[saule_ast::Param] {
    if f.params.first().map(|p| p.name == "self").unwrap_or(false) {
        &f.params[1..]
    } else {
        &f.params
    }
}

thread_local! {
    // `self` is bound on every method and constructor call. Interning it once
    // keeps that from allocating a `String` each time.
    static SELF_KEY: Rc<str> = Rc::from("self");
}

/// The shared `self` binding key.
pub(crate) fn self_key() -> Rc<str> {
    SELF_KEY.with(Rc::clone)
}
