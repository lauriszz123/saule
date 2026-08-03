//! Invoking a callable value and running a function body, including
//! the module-source bookkeeping that gives a runtime error the right
//! snippet.

use crate::env::Environment;
use crate::error::RuntimeError;
use crate::eval::Flow;
use crate::eval::expr::construct::construct;
use crate::eval::expr::{EvaluatedArg, eval, first_or_nil};
use crate::value::{FunctionBody, FunctionObject, Value};
use saule_ast::{Expr, Spanned};
use std::cell::RefCell;
use std::rc::Rc;

use super::*;

/// Invoke a callable [`Value`]. Handles both native and user-defined
/// functions.
pub(crate) fn call_value(
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
            let positional = native_positional_args(nc.name, &nc.param_names, args, &span)?;
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

pub(crate) fn run_function_body(
    f: &FunctionObject,
    scope: &Rc<RefCell<Environment>>,
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    Ok(first_or_nil(run_function_body_multi(f, scope, span)?))
}

pub(crate) fn run_function_body_multi(
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

pub(crate) fn run_function_body_multi_inner(
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

/// Wrap a `RuntimeError` with the module's source so the offending span
/// resolves against the right file.
pub(crate) fn attach_module_source(
    err: RuntimeError,
    src: &Rc<miette::NamedSource<String>>,
) -> RuntimeError {
    match err {
        // Already-wrapped errors keep their original module context.
        RuntimeError::ImportFailed { .. } | RuntimeError::InModule { .. } => err,
        // `Thrown` must stay un-wrapped so an outer `try ... catch` in the
        // caller's module can still intercept it. The diagnostic will lose
        // its module source if it escapes to the top level, but correctness
        // (catch actually fires) wins over presentation.
        RuntimeError::Thrown { .. } => err,
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

pub(crate) fn source_text(src: &miette::NamedSource<String>) -> String {
    src.inner().clone()
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

pub(crate) fn call_function_multi(
    f: &FunctionObject,
    args: &[EvaluatedArg],
    span: std::ops::Range<usize>,
) -> Result<Vec<Value>, RuntimeError> {
    let scope = Environment::with_parent(f.closure.clone());
    if let Some(class) = f.resolved_owner() {
        inject_class_statics(&scope, &class);
    }
    bind_params(&scope, &f.params, &f.param_keys, args, &span)?;
    run_function_body_multi(f, &scope, span)
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

            // `obj?.method(args)` — same short-circuit as in `eval`'s
            // `Expr::Call` arm. Without this, `read_member` returns the
            // bare unbound method and `call_value_multi` invokes it
            // without binding `self`, producing a confusing internal
            // "`self` reached evaluation outside a method" error.
            if let Expr::SafeMember { obj, name } = &callee.value {
                let receiver = eval(obj, env)?;
                if matches!(receiver, Value::Nil) {
                    return Ok(vec![Value::Nil]);
                }
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
