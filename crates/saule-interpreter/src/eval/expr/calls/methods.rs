//! Method dispatch: instance and static calls, member-call
//! resolution, and `self.super(...)` delegation.

use crate::env::Environment;
use crate::error::RuntimeError;
use crate::eval::expr::members::read_member;
use crate::eval::expr::{EvaluatedArg, SUPER_OWNER_BINDING, eval, first_or_nil};
use crate::value::{ClassObject, FunctionObject, MethodRef, Value};
use saule_ast::{CallArg, Expr, Spanned};
use std::cell::RefCell;
use std::rc::Rc;

use super::*;

pub(crate) fn call_instance_method_multi(
    f: &FunctionObject,
    receiver: Value,
    args: &[EvaluatedArg],
    span: std::ops::Range<usize>,
) -> Result<Vec<Value>, RuntimeError> {
    let scope = Environment::with_parent(f.closure.clone());
    if let Value::Instance(inst) = &receiver {
        inject_class_statics(&scope, &inst.borrow().class);
    }
    scope.borrow_mut().define(self_key(), receiver);
    bind_params(&scope, user_params(f), f.user_param_keys(), args, &span)?;
    let result = run_function_body_multi(f, &scope, span);
    Environment::release(scope);
    result
}

/// Invoke an instance method that may have come from either engine.
///
/// The bytecode arm is where the tree-walker calls into the VM: a compiled
/// method takes `self` as parameter 0 (`VM_DESIGN.md` §6.2), so binding the
/// receiver is simply prepending it to the argument list — no scope, no
/// `self` definition, no statics injection, because a compiled body reaches
/// all three through slots the compiler already resolved.
pub(crate) fn call_method_ref_multi(
    m: &MethodRef,
    receiver: Value,
    args: &[EvaluatedArg],
    span: std::ops::Range<usize>,
) -> Result<Vec<Value>, RuntimeError> {
    match m {
        MethodRef::Tree(f) => call_instance_method_multi(f, receiver, args, span),
        MethodRef::Vm(f) => {
            let positional = vm_positional_args(f, args, &span)?;
            let mut all = Vec::with_capacity(positional.len() + 1);
            all.push(receiver);
            all.extend(positional);
            f.invoke(&all, span)
        }
    }
}

/// [`call_method_ref_multi`] for a static, which takes no receiver at all —
/// `CALLSTAT` starts its frame at the arguments.
pub(crate) fn call_static_method_ref_multi(
    m: &MethodRef,
    class: &Rc<ClassObject>,
    args: &[EvaluatedArg],
    span: std::ops::Range<usize>,
) -> Result<Vec<Value>, RuntimeError> {
    match m {
        MethodRef::Tree(f) => call_static_method_multi(f, class, args, span),
        MethodRef::Vm(f) => {
            let positional = vm_positional_args(f, args, &span)?;
            f.invoke(&positional, span)
        }
    }
}

/// Invoke a static method with `self` bound to the class itself.
pub(crate) fn call_static_method_multi(
    f: &FunctionObject,
    class: &Rc<ClassObject>,
    args: &[EvaluatedArg],
    span: std::ops::Range<usize>,
) -> Result<Vec<Value>, RuntimeError> {
    let scope = Environment::with_parent(f.closure.clone());
    inject_class_statics(&scope, class);
    scope
        .borrow_mut()
        .define(self_key(), Value::Class(class.clone()));
    bind_params(&scope, user_params(f), f.user_param_keys(), args, &span)?;
    let result = run_function_body_multi(f, &scope, span);
    Environment::release(scope);
    result
}

/// Make the class's static fields and methods directly visible inside a
/// method body.
///
/// This used to copy every static from the whole inheritance chain into the
/// fresh scope — a `String` clone and a map insert per static, on *every*
/// method call. Now it just hands the scope a pointer to the class and lets
/// `Environment::get` consult it on a miss, which is free at call time and
/// costs one extra probe only on names that aren't locals.
pub(crate) fn inject_class_statics(scope: &Rc<RefCell<Environment>>, class: &Rc<ClassObject>) {
    scope.borrow_mut().set_statics_owner(class.clone());
}

/// Resolve `obj.name(args)` where the lookup intent is to *invoke* the
/// result.
pub(crate) fn dispatch_member_call(
    receiver: &Value,
    name: &str,
    args: &[EvaluatedArg],
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    Ok(first_or_nil(dispatch_member_call_multi(
        receiver, name, args, span,
    )?))
}

pub(crate) fn dispatch_member_call_multi(
    receiver: &Value,
    name: &str,
    args: &[EvaluatedArg],
    span: std::ops::Range<usize>,
) -> Result<Vec<Value>, RuntimeError> {
    match receiver {
        Value::Instance(inst) => {
            let class = inst.borrow().class.clone();
            if let Some(m) = class.lookup_method(name) {
                return call_method_ref_multi(&m, receiver.clone(), args, span);
            }
            if let Some(m) = class.lookup_static_method(name) {
                return call_static_method_ref_multi(&m, &class, args, span);
            }
            if let Some(v) = inst.borrow().field(name).cloned() {
                return call_value_multi(v, args, span);
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
                return call_static_method_ref_multi(&m, class, args, span);
            }
            if let Some(v) = class.lookup_static_field(name) {
                return call_value_multi(v, args, span);
            }
            Err(RuntimeError::TypeError {
                message: format!(
                    "no static member `{name}` on class `{}` — check the class definition for the correct name",
                    class.name
                ),
                span,
            })
        }
        Value::EnumVariant(variant) => {
            // A method the enum declares is called *with* the variant as its
            // receiver; `.value`, `.name`, and anything a payload happens to
            // hold are not. Which of the two this is has to be decided by
            // looking in the method map — deciding it from the shape of what
            // `read_member` returned would call a variant whose payload is a
            // function with a receiver it never declared.
            //
            // `read_member` answers `value` and `name` before it consults
            // the method map, so a method spelled either way is already
            // unreachable as a method. Mirrored here rather than reordered:
            // the two paths have to agree about which name wins.
            let method = (!matches!(name, "value" | "name"))
                .then(|| {
                    variant
                        .enum_obj
                        .borrow()
                        .as_ref()
                        .and_then(|e| e.methods.get(name).cloned())
                })
                .flatten();
            match method {
                // Either engine's method, receiver prepended for a compiled
                // one exactly as an instance method's is.
                Some(m) => call_method_ref_multi(&m, receiver.clone(), args, span),
                None => {
                    let v = read_member(receiver, name, span.clone())?;
                    call_value_multi(v, args, span)
                }
            }
        }
        Value::File(handle) => {
            let mut positional = Vec::with_capacity(args.len());
            for a in args {
                match a {
                    EvaluatedArg::Positional(v) | EvaluatedArg::TrailingBlock(v) => {
                        positional.push(v.clone())
                    }
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
            call_value_multi(v, args, span)
        }
    }
}

/// `self.super(args)` — call the parent class's constructor on the current
/// instance.
pub(crate) fn super_call(
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
    let ctor = crate::eval::expr::construct::constructor_chain(&parent).ok_or_else(|| {
        RuntimeError::TypeError {
            message: format!("parent class `{}` has no constructor", parent.name),
            span: span.clone(),
        }
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
    bind_params(
        &scope,
        user_params(&ctor),
        ctor.user_param_keys(),
        &vs,
        &span,
    )?;
    let result = run_function_body(&ctor, &scope, span).map(|_| Value::Nil);
    Environment::release(scope);
    result
}

pub(crate) fn eval_super_args(
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

pub(crate) fn super_owner_class(env: &Rc<RefCell<Environment>>) -> Option<Rc<ClassObject>> {
    match env.borrow().get(SUPER_OWNER_BINDING) {
        Some(Value::Class(c)) => Some(c),
        _ => None,
    }
}

pub(crate) fn invoke_method_multi(
    receiver: &Value,
    name: &str,
    args: Vec<EvaluatedArg>,
    span: std::ops::Range<usize>,
) -> Result<Vec<Value>, RuntimeError> {
    let out = dispatch_member_call_multi(receiver, name, &args, span);
    crate::recycle::give_args(args);
    out
}
