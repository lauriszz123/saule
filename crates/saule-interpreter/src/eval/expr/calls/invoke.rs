//! Invoking a callable value and running a function body, including
//! the module-source bookkeeping that gives a runtime error the right
//! snippet.

use crate::env::Environment;
use crate::error::RuntimeError;
use crate::eval::Flow;
use crate::eval::expr::construct::construct;
use crate::eval::expr::{EvaluatedArg, eval, first_or_nil};
use crate::recycle::give_args;
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
                    EvaluatedArg::Positional(v) | EvaluatedArg::TrailingBlock(v) => {
                        positional.push(v.clone())
                    }
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
                .map(crate::recycle::values_of)
                .map_err(|message| RuntimeError::TypeError { message, span })
        }
        Value::Function(f) => call_function_multi(&f, args, span),
        Value::NativeClosure(nc) => {
            let positional = native_positional_args(nc.name, &nc.param_names, args, &span)?;
            (nc.func)(&positional).map_err(|message| RuntimeError::TypeError { message, span })
        }
        Value::Class(c) => construct(c, args, span).map(crate::recycle::values_of),
        // A function the *other* engine compiled. Reached whenever the
        // tree-walker's own code has to call a value the VM produced: a
        // native invoking its callable argument, an operator overload, a
        // `toString`. The VM runs it on a fresh register file over its
        // existing shared state (`VM_DESIGN.md` §22.1).
        Value::VmFunction(f) => {
            let positional = vm_positional_args(&f, args, &span)?;
            f.invoke(&positional, span)
        }
        other => Err(RuntimeError::TypeError {
            message: format!(
                "value of type `{}` is not callable — only functions, classes, and methods can be called",
                other.type_name()
            ),
            span,
        }),
    }
}

/// Flatten arguments for a bytecode callee.
///
/// A `Proto` carries parameter *slots*, not names — §19's named-argument
/// binding is compiled into the callee's prologue, and the compiler refuses
/// a named argument outright today. So a named argument reaching a bytecode
/// function is refused here rather than silently bound positionally, which
/// would pass the right count of arguments in the wrong order.
pub(crate) fn vm_positional_args(
    f: &Rc<crate::value::VmFunctionRef>,
    args: &[EvaluatedArg],
    span: &std::ops::Range<usize>,
) -> Result<Vec<Value>, RuntimeError> {
    let mut out = Vec::with_capacity(args.len());
    for arg in args {
        match arg {
            EvaluatedArg::Positional(v) | EvaluatedArg::TrailingBlock(v) => out.push(v.clone()),
            EvaluatedArg::Named { name, .. } => {
                return Err(RuntimeError::TypeError {
                    message: format!(
                        "named argument `{name}` is not supported when calling `{}` \
                         through a built-in — use positional arguments instead",
                        f.vm_name().unwrap_or("<lambda>")
                    ),
                    span: span.clone(),
                });
            }
        }
    }
    Ok(out)
}

pub(crate) fn run_function_body(
    f: &FunctionObject,
    scope: &Rc<RefCell<Environment>>,
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    Ok(first_or_nil(run_function_body_multi(f, scope, span)?))
}

/// The scope a call to `f` runs in: parented to its closure, with the
/// owning class's statics injected and the self-recursion name bound.
///
/// Extracted so the tail-call trampoline builds a callee's scope exactly
/// the way an ordinary call does — a second, subtly different copy of this
/// is precisely how a tail call would stop being the same call.
pub(crate) fn scope_for(f: &Rc<FunctionObject>) -> Rc<RefCell<Environment>> {
    let scope = Environment::with_parent(f.closure.clone());
    if let Some(class) = f.resolved_owner() {
        inject_class_statics(&scope, &class);
    }
    // A self-recursive local closure reaches itself through the call scope
    // rather than through a captured binding, so the function and its
    // captured scope never point at each other. See
    // `FunctionObject::self_name`.
    if let Some(name) = f.self_name.borrow().clone() {
        scope
            .borrow_mut()
            .define(name, Value::Function(Rc::clone(f)));
    }
    scope
}

pub(crate) fn run_function_body_multi(
    f: &FunctionObject,
    scope: &Rc<RefCell<Environment>>,
    span: std::ops::Range<usize>,
) -> Result<Vec<Value>, RuntimeError> {
    // Every user-defined body — free function, method, constructor, lambda —
    // funnels through here, which makes it the one place a recursion counter
    // has to live. Held for the duration of the body so the guard's `Drop`
    // unwinds the count on the error paths too.
    //
    // It is also why the **tail-call trampoline** lives here rather than in
    // `call_function_multi`: the guard is entered once and held across the
    // whole tail chain, so a tail-recursive loop costs one unit of depth
    // however many times it iterates — which is the entire point. Putting
    // the loop a level up would have left methods and constructors, which
    // enter through the other two call sites of this function, still
    // recursing.
    let _depth = crate::eval::DepthGuard::enter(&span)?;

    // `src` tracks whichever function is *currently* running, so an error
    // raised three tail calls into a chain resolves its span against the
    // module that function came from rather than the one that started it.
    let mut src = f.source.clone();
    let mut raw = run_function_body_multi_inner(f, scope, span);

    loop {
        let (callee, args, call_span) = match raw {
            Ok(BodyOutcome::Tail { callee, args, span }) => (callee, args, span),
            Ok(BodyOutcome::Values(v)) => return Ok(v),
            Err(e) => {
                return Err(match src.as_ref() {
                    Some(s) => attach_module_source(e, s),
                    None => e,
                });
            }
        };

        // The tail call *replaces* the frame that produced it: the previous
        // scope is already released, and no native frame is added here.
        let next = scope_for(&callee);
        if let Err(e) = bind_params(
            &next,
            &callee.params,
            &callee.param_keys,
            &args,
            &call_span,
        ) {
            Environment::release(next);
            return Err(match callee.source.as_ref() {
                Some(s) => attach_module_source(e, s),
                None => e,
            });
        }
        src = callee.source.clone();
        raw = run_function_body_multi_inner(&callee, &next, call_span);
        Environment::release(next);
    }
}

/// What a function body produced: values, or a tail call not yet made.
pub(crate) enum BodyOutcome {
    Values(Vec<Value>),
    Tail {
        callee: Rc<FunctionObject>,
        args: Vec<crate::eval::expr::EvaluatedArg>,
        span: std::ops::Range<usize>,
    },
}

pub(crate) fn run_function_body_multi_inner(
    f: &FunctionObject,
    scope: &Rc<RefCell<Environment>>,
    span: std::ops::Range<usize>,
) -> Result<BodyOutcome, RuntimeError> {
    let outcome = match &f.body {
        FunctionBody::Block(stmts) => crate::eval::stmt::exec_block(stmts, scope)?,
        FunctionBody::Expr(e) => Flow::Return(crate::recycle::values_of(eval(e, scope)?)),
    };
    match outcome {
        Flow::Return(values) => Ok(BodyOutcome::Values(values)),
        Flow::TailCall { callee, args, span } => Ok(BodyOutcome::Tail { callee, args, span }),
        Flow::Normal(_) => Ok(BodyOutcome::Values(crate::recycle::values_of(Value::Nil))),
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
    f: &Rc<FunctionObject>,
    args: &[EvaluatedArg],
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    Ok(first_or_nil(call_function_multi(f, args, span)?))
}

pub(crate) fn call_function_multi(
    f: &Rc<FunctionObject>,
    args: &[EvaluatedArg],
    span: std::ops::Range<usize>,
) -> Result<Vec<Value>, RuntimeError> {
    let scope = scope_for(f);
    bind_params(&scope, &f.params, &f.param_keys, args, &span)?;
    let result = run_function_body_multi(f, &scope, span);
    Environment::release(scope);
    result
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
                    return Ok(crate::recycle::values_of(super_call(obj, args, env, span)?));
                }
                let receiver = eval(obj, env)?;
                let vs = eval_call_args(args, env)?;
                let out = dispatch_member_call_multi(&receiver, name, &vs, span);
                give_args(vs);
                return out;
            }

            // `obj?.method(args)` — same short-circuit as in `eval`'s
            // `Expr::Call` arm. Without this, `read_member` returns the
            // bare unbound method and `call_value_multi` invokes it
            // without binding `self`, producing a confusing internal
            // "`self` reached evaluation outside a method" error.
            if let Expr::SafeMember { obj, name } = &callee.value {
                let receiver = eval(obj, env)?;
                if matches!(receiver, Value::Nil) {
                    return Ok(crate::recycle::values_of(Value::Nil));
                }
                let vs = eval_call_args(args, env)?;
                let out = dispatch_member_call_multi(&receiver, name, &vs, span);
                give_args(vs);
                return out;
            }

            let cv = eval(callee, env)?;
            let vs = eval_call_args(args, env)?;
            let out = call_value_multi(cv, &vs, span);
            give_args(vs);
            out
        }
        _ => Ok(crate::recycle::values_of(eval(expr, env)?)),
    }
}
