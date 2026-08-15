//! Assignment statements (`a = …`, `a op= …`, `a, b = …`, `obj.field = …`,
//! `tbl[index] = …`).

use std::cell::RefCell;
use std::rc::Rc;

use saule_ast::{BinOp, Expr, Spanned};

use crate::env::Environment;
use crate::error::RuntimeError;
use crate::value::{ClassObject, Value};

use super::super::expr::members::{read_index, read_member};
use super::super::{Flow, expr, ops};

pub(super) fn exec_assign(
    target: &Spanned<Expr>,
    value: &Spanned<Expr>,
    env: &Rc<RefCell<Environment>>,
) -> Result<Flow, RuntimeError> {
    let v = expr::eval(value, env)?;
    assign_target(target, v, env)
}

/// `target op= value` — read the target, combine, write back.
///
/// Each arm resolves the target's *place* once and then reuses it for both
/// the read and the write. That is the whole reason compound assignment is
/// not desugared to `target = target op value` in the parser: `t[i()] += 1`
/// must call `i()` once, and `make().total += 1` must update the object
/// `make()` returned rather than a second, freshly built one.
///
/// The combine itself goes through [`ops::binary`], so a class that
/// implements `OpAdd` gets `+=` for free with exactly the semantics its
/// `add` method defines.
pub(super) fn exec_compound_assign(
    target: &Spanned<Expr>,
    op: BinOp,
    value: &Spanned<Expr>,
    env: &Rc<RefCell<Environment>>,
) -> Result<Flow, RuntimeError> {
    let span = target.span.start..value.span.end;

    match &target.value {
        Expr::Ident(name) => {
            let current = env
                .borrow()
                .get(name)
                .ok_or_else(|| RuntimeError::TypeError {
                    message: format!(
                        "internal: compound assignment to undeclared `{name}` reached \
                         evaluation — `saule_semantic::analyze` was not run on \
                         this module"
                    ),
                    span: target.span.clone(),
                })?;
            let rhs = expr::eval(value, env)?;
            let combined = ops::binary(op, current, rhs, span)?;
            assign_target(target, combined, env)
        }
        Expr::Member { obj, name } => {
            let receiver = expr::eval(obj, env)?;
            let current = read_member(&receiver, name, target.span.clone())?;
            let rhs = expr::eval(value, env)?;
            let combined = ops::binary(op, current, rhs, span)?;
            assign_member(&receiver, name, combined, target.span.clone())
        }
        Expr::Index { obj, index } => {
            let receiver = expr::eval(obj, env)?;
            let index_value = expr::eval(index, env)?;
            let current = read_index(&receiver, index_value.clone(), target.span.clone())?;
            let rhs = expr::eval(value, env)?;
            let combined = ops::binary(op, current, rhs, span)?;
            assign_index(&receiver, index_value, combined, target.span.clone())
        }
        _ => Err(RuntimeError::InvalidAssignTarget {
            span: target.span.clone(),
        }),
    }
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
            // Instances have a fixed shape now, so there is no slot to
            // conjure for a name the class never declared. The typechecker
            // already rejects that (`tests/ui/unknown_field.sau`); this only
            // fires for a caller that skipped it via the raw `run()` entry
            // point, where silently creating an invisible field would be a
            // worse answer than saying so.
            if inst.borrow_mut().set_field(name, value) {
                Ok(Flow::nil())
            } else {
                let class = inst.borrow().class.name.clone();
                let known = inst.borrow().class.layout.names().join("`, `");
                Err(RuntimeError::TypeError {
                    message: if known.is_empty() {
                        format!("class `{class}` declares no instance field `{name}`")
                    } else {
                        format!(
                            "class `{class}` declares no instance field `{name}` — \
                             it has `{known}`"
                        )
                    },
                    span,
                })
            }
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
        // Lua-style table write: `t.foo = v` is sugar for `t["foo"] = v`.
        Value::Table(items) => {
            let key = Value::Str(Rc::new(name.to_string()));
            items
                .borrow_mut()
                .set(&key, value)
                .map_err(|message| RuntimeError::TypeError { message, span })?;
            Ok(Flow::nil())
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
    match class.declaring_static_field(name) {
        Some(owner) => {
            owner
                .static_fields
                .borrow_mut()
                .insert(name.to_string(), value);
            true
        }
        None => false,
    }
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
        // `obj[key] = v` on a class instance is `OpNewIndex` — Saule's
        // `__newindex`. As with `OpIndex`, it runs on every write rather
        // than only on a miss: an instance has no key space to miss in.
        Value::Instance(_)
            if super::super::expr::members::has_index_overload(
                receiver,
                saule_ast::ops::OP_NEW_INDEX.method,
            ) =>
        {
            crate::eval::index_hooks::call_new_index(receiver, index, value, span)?;
            Ok(Flow::nil())
        }
        other => Err(RuntimeError::TypeError {
            message: format!(
                "cannot assign through `[index]` on a `{}` — only tables and classes \
                 implementing `OpNewIndex` support indexed assignment",
                other.type_name()
            ),
            span,
        }),
    }
}
