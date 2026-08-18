//! `try ... catch e: T ... end` evaluation and runtime type matching for
//! the catch arm.

use std::cell::RefCell;
use std::rc::Rc;

use saule_ast::{Spanned, Stmt, Type};

use crate::env::Environment;
use crate::error::RuntimeError;
use crate::value::Value;

use super::super::Flow;
use super::{exec_block, thrown_slot};

/// Run a `try ... catch e: T ... end` block. The catch arm fires only when:
///   1. the body errored with a `RuntimeError::Thrown`, **and**
///   2. the thrown value's runtime type matches `catch_ty`.
///
/// Any other error — or a thrown value whose type doesn't match — is
/// re-propagated so an outer `try` (or the top-level driver) can see it.
pub(super) fn exec_try(
    body: &[Spanned<Stmt>],
    catch_var: &str,
    catch_ty: &Type,
    catch_body: &[Spanned<Stmt>],
    env: &Rc<RefCell<Environment>>,
) -> Result<Flow, RuntimeError> {
    let body_scope = Environment::with_parent(env.clone());
    // A tail call is a call that has not happened yet, and letting one escape
    // a `try` would run it with the handler already unwound — `try return
    // f() catch ...` would stop catching what `f` throws. Forcing it trades
    // the frame reuse for keeping `catch` meaningful, which is the right way
    // round: a tail call is an optimisation, a handler is semantics.
    //
    // **The forced call has to be folded back into the same `Result` the
    // body produced**, not made with a `?`. Propagating it directly ran the
    // callee outside the very handler that forced it, so `try return boom()
    // catch` reported `uncaught exception` — the exact failure the forcing
    // exists to prevent. Found by the VM, which keeps the handler live and
    // therefore caught it where the oracle did not.
    let outcome = match exec_block(body, &body_scope) {
        Ok(Flow::TailCall { callee, args, span }) => {
            crate::eval::expr::call_function_multi(&callee, &args, span).map(Flow::Return)
        }
        other => other,
    };
    match outcome {
        Ok(flow) => Ok(flow),
        Err(RuntimeError::Thrown { value, span }) => {
            let thrown = thrown_slot::take().unwrap_or(Value::Nil);
            if runtime_matches_type(&thrown, catch_ty) {
                let catch_scope = Environment::with_parent(env.clone());
                catch_scope
                    .borrow_mut()
                    .define(catch_var.to_string(), thrown);
                exec_block(catch_body, &catch_scope)
            } else {
                // Re-park and re-throw for an outer handler.
                thrown_slot::set(thrown);
                Err(RuntimeError::Thrown { value, span })
            }
        }
        Err(other) => Err(other),
    }
}

/// Best-effort runtime check that `value` satisfies the declared `catch_ty`.
/// Nullable, table-of, and function types are accepted structurally; classes
/// match by walking the parent chain; interfaces match by name lookup.
fn runtime_matches_type(value: &Value, ty: &Type) -> bool {
    match ty {
        Type::Nullable(inner) => matches!(value, Value::Nil) || runtime_matches_type(value, inner),
        Type::Tuple(_) => true, // multi-return shapes aren't introspectable here
        Type::Function { .. } => matches!(
            value,
            Value::Function(_) | Value::Native(_) | Value::NativeClosure(_)
        ),
        Type::Table { .. } => matches!(value, Value::Table(_)),
        Type::Named(name) => match name.as_str() {
            "any" => true,
            "nil" => matches!(value, Value::Nil),
            "boolean" => matches!(value, Value::Bool(_)),
            "integer" => matches!(value, Value::Int(_)),
            "float" => matches!(value, Value::Float(_)),
            "number" => matches!(value, Value::Int(_) | Value::Float(_)),
            "string" => matches!(value, Value::Str(_)),
            "table" => matches!(value, Value::Table(_)),
            // No `"function"` arm: the bare name is not a type. A callable
            // `catch` type is written `fn(...) -> T`, handled above.
            other => match value {
                Value::Instance(inst) => {
                    let inst_ref = inst.borrow();
                    let mut cur = Some(inst_ref.class.clone());
                    while let Some(c) = cur {
                        if c.name == other {
                            return true;
                        }
                        cur = c.parent.clone();
                    }
                    false
                }
                Value::Class(c) => c.name == other,
                Value::EnumVariant(v) => v.enum_name == other,
                Value::Enum(e) => e.name == other,
                _ => false,
            },
        },
    }
}
