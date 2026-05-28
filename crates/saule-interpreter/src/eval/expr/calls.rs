//! Calling user functions, native functions, instance/static methods, and
//! dispatching `obj.member(args)` / `obj:method(args)` calls.

use std::cell::RefCell;
use std::rc::Rc;

use saule_ast::{CallArg, Expr, Spanned};

use crate::env::Environment;
use crate::error::RuntimeError;
use crate::value::{ClassObject, FunctionBody, FunctionObject, Value};

use super::super::Flow;
use super::construct::construct;
use super::members::read_member;
use super::{EvaluatedArg, SUPER_OWNER_BINDING, eval, first_or_nil, is_nullable_type};

/// Invoke a callable [`Value`]. Handles both native and user-defined
/// functions.
pub(super) fn call_value(
    callee: Value,
    args: &[EvaluatedArg],
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    Ok(first_or_nil(call_value_multi(callee, args, span)?))
}

pub(crate) fn call_value_multi(
    callee: Value,
    args: &[EvaluatedArg],
    span: std::ops::Range<usize>,
) -> Result<Vec<Value>, RuntimeError> {
    match callee {
        Value::Native(nf) => {
            let mut positional = Vec::with_capacity(args.len());
            for arg in args {
                match arg {
                    EvaluatedArg::Positional(v) => positional.push(v.clone()),
                    EvaluatedArg::Named { name: _, .. } => {
                        return Err(RuntimeError::TypeError {
                            message: format!(
                                "named arguments are not supported for built-in function `{}` — use positional arguments instead",
                                nf.name
                            ),
                            span,
                        });
                    }
                }
            }
            (nf.func)(&positional)
                .map(|v| vec![v])
                .map_err(|message| RuntimeError::TypeError { message, span })
        }
        Value::Function(f) => call_function_multi(&f, args, span),
        Value::NativeClosure(nc) => {
            let mut positional = Vec::with_capacity(args.len());
            for arg in args {
                match arg {
                    EvaluatedArg::Positional(v) => positional.push(v.clone()),
                    EvaluatedArg::Named { name: _, .. } => {
                        return Err(RuntimeError::TypeError {
                            message: format!(
                                "named arguments are not supported for built-in function `{}` — use positional arguments instead",
                                nc.name
                            ),
                            span,
                        });
                    }
                }
            }
            (nc.func)(&positional).map_err(|message| RuntimeError::TypeError { message, span })
        }
        Value::Class(c) => construct(c, args, span).map(|v| vec![v]),
        other => Err(RuntimeError::TypeError {
            message: format!(
                "value of type `{}` is not callable — only functions, classes, and methods can be called",
                other.type_name()
            ),
            span,
        }),
    }
}

/// Bind `params` into `scope` from `args`, evaluating defaults lazily. Used
/// by every kind of call (free function, instance method, static method,
/// constructor) so the rules stay in one place.
pub(super) fn bind_params(
    scope: &Rc<RefCell<Environment>>,
    params: &[saule_ast::Param],
    args: &[EvaluatedArg],
    span: &std::ops::Range<usize>,
) -> Result<(), RuntimeError> {
    let variadic_indices: Vec<usize> = params
        .iter()
        .enumerate()
        .filter_map(|(i, p)| p.variadic.then_some(i))
        .collect();
    if variadic_indices.len() > 1 {
        return Err(RuntimeError::TypeError {
            message: "a function can declare at most one variadic parameter".to_string(),
            span: params[variadic_indices[1]].span.clone(),
        });
    }
    let variadic_idx = variadic_indices.first().copied();
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

    let missing_required: Vec<String> = params
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            if assigned[i].is_none()
                && !p.variadic
                && p.default.is_none()
                && !is_nullable_type(&p.ty)
            {
                Some(p.name.clone())
            } else {
                None
            }
        })
        .collect();

    for (i, param) in params.iter().enumerate() {
        if param.variadic {
            scope.borrow_mut().define(
                param.name.clone(),
                Value::Table(Rc::new(RefCell::new(crate::value::TableObject::from_array(
                    variadic_values.clone(),
                )))),
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
            let help = if missing_required.len() == 1 {
                Some(format!(
                    "add argument `{}` to this call (positionally or by name)",
                    missing_required[0]
                ))
            } else {
                Some(format!(
                    "add required arguments to this call: {}",
                    missing_required
                        .iter()
                        .map(|n| format!("`{n}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            };
            return Err(RuntimeError::ArgumentError {
                message: if missing_required.len() == 1 {
                    format!("missing required argument for parameter `{}`", param.name)
                } else {
                    format!(
                        "missing {} required argument(s): {}",
                        missing_required.len(),
                        missing_required.join(", ")
                    )
                },
                help,
                span: span.clone(),
            });
        };
        scope.borrow_mut().define(param.name.clone(), value);
    }
    Ok(())
}

pub(super) fn eval_call_args(
    args: &[CallArg],
    env: &Rc<RefCell<Environment>>,
) -> Result<Vec<EvaluatedArg>, RuntimeError> {
    let mut out = Vec::with_capacity(args.len());
    for arg in args {
        match arg {
            CallArg::Positional(expr) => out.push(EvaluatedArg::Positional(eval(expr, env)?)),
            CallArg::Named { name, value } => out.push(EvaluatedArg::Named {
                name: name.clone(),
                value: eval(value, env)?,
            }),
        }
    }
    Ok(out)
}

pub(super) fn run_function_body(
    f: &FunctionObject,
    scope: &Rc<RefCell<Environment>>,
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    Ok(first_or_nil(run_function_body_multi(f, scope, span)?))
}

pub(super) fn run_function_body_multi(
    f: &FunctionObject,
    scope: &Rc<RefCell<Environment>>,
    span: std::ops::Range<usize>,
) -> Result<Vec<Value>, RuntimeError> {
    let raw = run_function_body_multi_inner(f, scope, span);
    match (raw, f.source.as_ref()) {
        (Err(e), Some(src)) => Err(attach_module_source(e, src)),
        (other, _) => other,
    }
}

/// Wrap a `RuntimeError` with the module's source so the offending span
/// resolves against the right file.
fn attach_module_source(
    err: RuntimeError,
    src: &Rc<miette::NamedSource<String>>,
) -> RuntimeError {
    match err {
        RuntimeError::ImportFailed { .. } | RuntimeError::InModule { .. } => err,
        other => {
            let inner = crate::error::ImportedDiagnostic::from_inner(
                &other,
                src.name().to_string(),
                source_text(src),
            );
            RuntimeError::InModule {
                module_label: src.name().to_string(),
                inner: Box::new(inner),
            }
        }
    }
}

fn source_text(src: &miette::NamedSource<String>) -> String {
    src.inner().clone()
}

fn run_function_body_multi_inner(
    f: &FunctionObject,
    scope: &Rc<RefCell<Environment>>,
    span: std::ops::Range<usize>,
) -> Result<Vec<Value>, RuntimeError> {
    let outcome = match &f.body {
        FunctionBody::Block(stmts) => crate::eval::stmt::exec_block(stmts, scope)?,
        FunctionBody::Expr(e) => Flow::Return(vec![eval(e, scope)?]),
    };
    match outcome {
        Flow::Return(values) => Ok(values),
        Flow::Normal(_) => Ok(vec![Value::Nil]),
        // `break` / `continue` escaping a function body is rejected by
        // `saule_semantic`'s control-flow walker before we ever evaluate.
        // Reaching this arm means the caller skipped semantic — surface as
        // a generic type error rather than carry a dedicated variant.
        Flow::Break => Err(RuntimeError::TypeError {
            message: "internal: `break` escaped a function body — \
                      `saule_semantic::analyze` was not run on this module"
                .to_string(),
            span,
        }),
        Flow::Continue => Err(RuntimeError::TypeError {
            message: "internal: `continue` escaped a function body — \
                      `saule_semantic::analyze` was not run on this module"
                .to_string(),
            span,
        }),
    }
}

/// Skip a leading `self` parameter (so explicit-self style still works
/// alongside the implicit-self style this language prefers).
pub(super) fn user_params(f: &FunctionObject) -> &[saule_ast::Param] {
    if f.params.first().map(|p| p.name == "self").unwrap_or(false) {
        &f.params[1..]
    } else {
        &f.params
    }
}

/// Call a user-defined function: binds args (with defaults), executes the
/// body in a fresh scope parented to the function's closure, and converts
/// the resulting [`Flow`] into a return value.
pub(crate) fn call_function(
    f: &FunctionObject,
    args: &[EvaluatedArg],
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    Ok(first_or_nil(call_function_multi(f, args, span)?))
}

fn call_function_multi(
    f: &FunctionObject,
    args: &[EvaluatedArg],
    span: std::ops::Range<usize>,
) -> Result<Vec<Value>, RuntimeError> {
    let scope = Environment::with_parent(f.closure.clone());
    if let Some(class) = f.resolved_owner() {
        inject_class_statics(&scope, &class);
    }
    bind_params(&scope, &f.params, args, &span)?;
    run_function_body_multi(f, &scope, span)
}

pub(super) fn call_instance_method_multi(
    f: &FunctionObject,
    receiver: Value,
    args: &[EvaluatedArg],
    span: std::ops::Range<usize>,
) -> Result<Vec<Value>, RuntimeError> {
    let scope = Environment::with_parent(f.closure.clone());
    if let Value::Instance(inst) = &receiver {
        inject_class_statics(&scope, &inst.borrow().class);
    }
    scope.borrow_mut().define("self".to_string(), receiver);
    bind_params(&scope, user_params(f), args, &span)?;
    run_function_body_multi(f, &scope, span)
}

/// Invoke a static method with `self` bound to the class itself.
fn call_static_method(
    f: &FunctionObject,
    class: &Rc<ClassObject>,
    args: &[EvaluatedArg],
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    Ok(first_or_nil(call_static_method_multi(f, class, args, span)?))
}

pub(super) fn call_static_method_multi(
    f: &FunctionObject,
    class: &Rc<ClassObject>,
    args: &[EvaluatedArg],
    span: std::ops::Range<usize>,
) -> Result<Vec<Value>, RuntimeError> {
    let scope = Environment::with_parent(f.closure.clone());
    inject_class_statics(&scope, class);
    scope
        .borrow_mut()
        .define("self".to_string(), Value::Class(class.clone()));
    bind_params(&scope, user_params(f), args, &span)?;
    run_function_body_multi(f, &scope, span)
}

/// Make the class's static fields and methods directly visible inside a
/// method body. Parent statics are seeded first and overridden by child
/// statics, matching the lookup order used for member access.
pub(super) fn inject_class_statics(scope: &Rc<RefCell<Environment>>, class: &Rc<ClassObject>) {
    let mut chain: Vec<Rc<ClassObject>> = Vec::new();
    let mut cur = Some(class.clone());
    while let Some(c) = cur {
        chain.push(c.clone());
        cur = c.parent.clone();
    }
    for c in chain.iter().rev() {
        for (n, v) in c.static_fields.borrow().iter() {
            scope.borrow_mut().define(n.clone(), v.clone());
        }
        for (n, f) in &c.static_methods {
            scope
                .borrow_mut()
                .define(n.clone(), Value::Function(f.clone()));
        }
    }
}

/// Public re-export of [`call_static_method`] for embedders.
pub(crate) fn call_static_method_public(
    f: &FunctionObject,
    class: &Rc<ClassObject>,
    args: &[EvaluatedArg],
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    call_static_method(f, class, args, span)
}

/// Resolve `obj.name(args)` where the lookup intent is to *invoke* the
/// result.
pub(super) fn dispatch_member_call(
    receiver: &Value,
    name: &str,
    args: Vec<EvaluatedArg>,
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    Ok(first_or_nil(dispatch_member_call_multi(
        receiver, name, args, span,
    )?))
}

pub(super) fn dispatch_member_call_multi(
    receiver: &Value,
    name: &str,
    args: Vec<EvaluatedArg>,
    span: std::ops::Range<usize>,
) -> Result<Vec<Value>, RuntimeError> {
    match receiver {
        Value::Instance(inst) => {
            let class = inst.borrow().class.clone();
            if let Some(m) = class.lookup_method(name) {
                return call_instance_method_multi(&m, receiver.clone(), &args, span);
            }
            if let Some(m) = class.lookup_static_method(name) {
                return call_static_method_multi(&m, &class, &args, span);
            }
            if let Some(v) = inst.borrow().fields.get(name).cloned() {
                return call_value_multi(v, &args, span);
            }
            Err(RuntimeError::TypeError {
                message: format!(
                    "no method or field `{name}` on instance of class `{}` — instance members are case-sensitive",
                    class.name
                ),
                span,
            })
        }
        Value::Class(class) => {
            if let Some(m) = class.lookup_static_method(name) {
                return call_static_method_multi(&m, class, &args, span);
            }
            if let Some(v) = class.lookup_static_field(name) {
                return call_value_multi(v, &args, span);
            }
            Err(RuntimeError::TypeError {
                message: format!(
                    "no static member `{name}` on class `{}` — check the class definition for the correct name",
                    class.name
                ),
                span,
            })
        }
        Value::EnumVariant(_variant) => {
            let v = read_member(receiver, name, span.clone())?;
            match v {
                Value::Function(m) => {
                    call_instance_method_multi(&m, receiver.clone(), &args, span)
                }
                _ => call_value_multi(v, &args, span),
            }
        }
        Value::File(handle) => {
            let mut positional = Vec::with_capacity(args.len());
            for a in args {
                match a {
                    EvaluatedArg::Positional(v) => positional.push(v),
                    EvaluatedArg::Named { .. } => {
                        return Err(RuntimeError::TypeError {
                            message: format!(
                                "named arguments are not supported on file methods (`{name}`) — use positional arguments instead"
                            ),
                            span,
                        });
                    }
                }
            }
            crate::stdlib::io::dispatch_file_method(handle, name, &positional)
                .map_err(|message| RuntimeError::TypeError { message, span })
        }
        _ => {
            let v = read_member(receiver, name, span.clone())?;
            call_value_multi(v, &args, span)
        }
    }
}

/// `self.super(args)` — call the parent class's constructor on the current
/// instance.
pub(super) fn super_call(
    obj: &Spanned<Expr>,
    args: &[CallArg],
    env: &Rc<RefCell<Environment>>,
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    let receiver = eval(obj, env)?;
    let inst = match receiver {
        Value::Instance(i) => i,
        other => {
            return Err(RuntimeError::TypeError {
                message: format!(
                    "`super(...)` requires `self` (an instance receiver), but got a `{}` — use `self.super(...)` inside a constructor",
                    other.type_name()
                ),
                span,
            });
        }
    };
    let owner_class = super_owner_class(env).unwrap_or_else(|| inst.borrow().class.clone());
    let parent = owner_class
        .parent
        .clone()
        .ok_or_else(|| RuntimeError::TypeError {
            message: format!(
                "class `{}` has no parent to call `super` on",
                owner_class.name
            ),
            span: span.clone(),
        })?;
    let ctor = super::construct::constructor_chain(&parent).ok_or_else(|| RuntimeError::TypeError {
        message: format!("parent class `{}` has no constructor", parent.name),
        span: span.clone(),
    })?;

    let vs = eval_super_args(args, env, &span)?;

    let scope = Environment::with_parent(ctor.closure.clone());
    scope
        .borrow_mut()
        .define("self".to_string(), Value::Instance(inst));
    scope.borrow_mut().define(
        SUPER_OWNER_BINDING.to_string(),
        Value::Class(parent.clone()),
    );
    bind_params(&scope, user_params(&ctor), &vs, &span)?;
    run_function_body(&ctor, &scope, span).map(|_| Value::Nil)
}

fn eval_super_args(
    args: &[CallArg],
    env: &Rc<RefCell<Environment>>,
    span: &std::ops::Range<usize>,
) -> Result<Vec<EvaluatedArg>, RuntimeError> {
    let mut out = Vec::with_capacity(args.len());
    for arg in args {
        match arg {
            CallArg::Positional(expr) => out.push(EvaluatedArg::Positional(eval(expr, env)?)),
            CallArg::Named { .. } => {
                return Err(RuntimeError::TypeError {
                    message: "named arguments are not supported in `super(...)`".to_string(),
                    span: span.clone(),
                });
            }
        }
    }
    Ok(out)
}

fn super_owner_class(env: &Rc<RefCell<Environment>>) -> Option<Rc<ClassObject>> {
    match env.borrow().get(SUPER_OWNER_BINDING) {
        Some(Value::Class(c)) => Some(c),
        _ => None,
    }
}

/// Dispatch `receiver:name(args)` (the colon-call form). Delegates to the
/// shared [`dispatch_member_call`] so the dot and colon forms behave the
/// same way at runtime.
pub(super) fn invoke_method(
    receiver: &Value,
    name: &str,
    args: Vec<EvaluatedArg>,
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    dispatch_member_call(receiver, name, args, span)
}

pub(crate) fn invoke_method_multi(
    receiver: &Value,
    name: &str,
    args: Vec<EvaluatedArg>,
    span: std::ops::Range<usize>,
) -> Result<Vec<Value>, RuntimeError> {
    dispatch_member_call_multi(receiver, name, args, span)
}

/// `eval_values` lives in the parent module but its body forwards into the
/// dispatch helpers in this file when it sees a method call.
pub(crate) fn eval_values(
    expr: &Spanned<Expr>,
    env: &Rc<RefCell<Environment>>,
) -> Result<Vec<Value>, RuntimeError> {
    let span = expr.span.clone();
    match &expr.value {
        Expr::Call { callee, args } => {
            if let Expr::Member { obj, name } = &callee.value {
                if name == "super" {
                    return Ok(vec![super_call(obj, args, env, span)?]);
                }
                let receiver = eval(obj, env)?;
                let vs = eval_call_args(args, env)?;
                return dispatch_member_call_multi(&receiver, name, vs, span);
            }

            let cv = eval(callee, env)?;
            let vs = eval_call_args(args, env)?;
            call_value_multi(cv, &vs, span)
        }
        Expr::MethodCall { obj, method, args } => {
            let receiver = eval(obj, env)?;
            let evaled = eval_call_args(args, env)?;
            invoke_method_multi(&receiver, method, evaled, span)
        }
        _ => Ok(vec![eval(expr, env)?]),
    }
}

// Re-export the helpers `eval` in mod.rs needs to call.
pub(super) use call_value as call_value_pub;
pub(super) use eval_call_args as eval_call_args_pub;
