//! Assignment statements (`a = …`, `a, b = …`, `obj.field = …`,
//! `tbl[index] = …`).

use std::cell::RefCell;
use std::rc::Rc;

use saule_ast::{Expr, Spanned};

use crate::env::Environment;
use crate::error::RuntimeError;
use crate::value::{ClassObject, Value};

use super::super::{Flow, expr};

pub(super) fn exec_assign(
    target: &Spanned<Expr>,
    value: &Spanned<Expr>,
    env: &Rc<RefCell<Environment>>,
) -> Result<Flow, RuntimeError> {
    let v = expr::eval(value, env)?;
    assign_target(target, v, env)
}

pub(super) fn assign_target(
    target: &Spanned<Expr>,
    v: Value,
    env: &Rc<RefCell<Environment>>,
) -> Result<Flow, RuntimeError> {
    match &target.value {
        Expr::Ident(name) => {
            if env.borrow_mut().assign(name, v) {
                Ok(Flow::nil())
            } else {
                // Caught earlier by `saule_semantic::analyze` as
                // `AssignToUndeclared`. Only reachable via the low-level
                // `run()` entry point on a module that wasn't checked first.
                Err(RuntimeError::TypeError {
                    message: format!(
                        "internal: assignment to undeclared `{name}` reached \
                         evaluation — `saule_semantic::analyze` was not run on \
                         this module"
                    ),
                    span: target.span.clone(),
                })
            }
        }
        Expr::Member { obj, name } => {
            let receiver = expr::eval(obj, env)?;
            assign_member(&receiver, name, v, target.span.clone())
        }
        Expr::Index { obj, index } => {
            let receiver = expr::eval(obj, env)?;
            let index_value = expr::eval(index, env)?;
            assign_index(&receiver, index_value, v, target.span.clone())
        }
        _ => Err(RuntimeError::InvalidAssignTarget {
            span: target.span.clone(),
        }),
    }
}

fn assign_member(
    receiver: &Value,
    name: &str,
    value: Value,
    span: std::ops::Range<usize>,
) -> Result<Flow, RuntimeError> {
    match receiver {
        Value::Instance(inst) => {
            inst.borrow_mut().fields.insert(name.to_string(), value);
            Ok(Flow::nil())
        }
        Value::Class(class) => {
            // Walk the chain — `Child.staticField = …` should update the
            // declaring class so the change is visible to every sibling.
            if set_static_in_chain(class, name, value.clone()) {
                Ok(Flow::nil())
            } else {
                // Define a fresh static on the most-derived class.
                class
                    .static_fields
                    .borrow_mut()
                    .insert(name.to_string(), value);
                Ok(Flow::nil())
            }
        }
        other => Err(RuntimeError::TypeError {
            message: format!(
                "cannot assign field `{name}` on value of type `{}` — only instances and classes can have fields assigned",
                other.type_name()
            ),
            span,
        }),
    }
}

fn set_static_in_chain(class: &Rc<ClassObject>, name: &str, value: Value) -> bool {
    if class.static_fields.borrow().contains_key(name) {
        class
            .static_fields
            .borrow_mut()
            .insert(name.to_string(), value);
        return true;
    }
    if let Some(parent) = &class.parent {
        return set_static_in_chain(parent, name, value);
    }
    false
}

fn assign_index(
    receiver: &Value,
    index: Value,
    value: Value,
    span: std::ops::Range<usize>,
) -> Result<Flow, RuntimeError> {
    match receiver {
        Value::Table(items) => {
            items
                .borrow_mut()
                .set(&index, value)
                .map_err(|message| RuntimeError::TypeError {
                    message,
                    span: span.clone(),
                })?;
            Ok(Flow::nil())
        }
        other => Err(RuntimeError::TypeError {
            message: format!(
                "cannot assign through `[index]` on a `{}` — only tables support indexed assignment",
                other.type_name()
            ),
            span,
        }),
    }
}
