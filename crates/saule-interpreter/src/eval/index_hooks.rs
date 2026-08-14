//! `OpIndex` / `OpNewIndex` dispatch — Saule's `__index` / `__newindex`.
//!
//! ## How these differ from Lua's
//!
//! Lua's metamethods are **miss handlers**: they fire only when a raw lookup
//! finds nothing, which is what lets `__index` double as the inheritance
//! mechanism. Saule already has real fields, methods and inheritance, so
//! these hooks have a narrower job — dynamic members, for proxies, records
//! and config objects — and a simpler rule: `obj[key]` on a class has no
//! stored key space to miss in, so the method *is* the lookup and runs on
//! every access.
//!
//! `obj.name` is deliberately **not** routed here. Field and method names on
//! a class are resolved statically, and sending the misses to a hook would
//! mean giving up "unknown member" diagnostics for the whole class — the one
//! diagnostic worth keeping. Use `obj[key]` for dynamic access; a fixed
//! surface is declared as ordinary methods.
//!
//! ## Re-entrancy
//!
//! A hook body that touches `self[k]` re-enters its own hook. Lua answers
//! this with `rawget` / `rawset`; the depth cap below is the version of that
//! which needs no extra surface syntax, and it turns a hang into a
//! diagnostic naming the class.

use std::cell::Cell;

use crate::error::RuntimeError;
use crate::eval::expr::{EvaluatedArg, invoke_method_multi};
use crate::value::Value;

thread_local! {
    static DEPTH: Cell<usize> = const { Cell::new(0) };
}

/// How deep `index` / `newIndex` may nest before it is treated as runaway
/// recursion. Comfortably above any honest proxy-of-a-proxy chain.
const MAX_DEPTH: usize = 32;

fn class_name(v: &Value) -> String {
    match v {
        Value::Instance(inst) => inst.borrow().class.name.clone(),
        other => other.type_name().to_string(),
    }
}

/// Run `f` one hook-level deeper, refusing past [`MAX_DEPTH`].
///
/// The decrement runs on the error path too, so a program that recovers does
/// not carry a stale count into the rest of the run.
fn guarded<T>(
    receiver: &Value,
    method: &str,
    span: &std::ops::Range<usize>,
    f: impl FnOnce() -> Result<T, RuntimeError>,
) -> Result<T, RuntimeError> {
    if DEPTH.with(|d| d.get()) >= MAX_DEPTH {
        return Err(RuntimeError::TypeError {
            message: format!(
                "`{}.{method}()` recursed more than {MAX_DEPTH} levels deep — a hook that \
                 indexes `self` re-enters itself; reach the backing store directly instead",
                class_name(receiver)
            ),
            span: span.clone(),
        });
    }
    DEPTH.with(|d| d.set(d.get() + 1));
    let out = f();
    DEPTH.with(|d| d.set(d.get() - 1));
    out
}

/// `receiver[key]` → `receiver.index(key)`.
pub fn call_index(
    receiver: &Value,
    key: Value,
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    let method = saule_ast::ops::OP_INDEX.method;
    guarded(receiver, method, &span, || {
        let values = invoke_method_multi(
            receiver,
            method,
            vec![EvaluatedArg::Positional(key)],
            span.clone(),
        )?;
        Ok(values.into_iter().next().unwrap_or(Value::Nil))
    })
}

/// `receiver[key] = value` → `receiver.newIndex(key, value)`.
///
/// The method's own return value is discarded: the contract declares
/// `-> nil`, and an assignment is a statement, so there is nothing for a
/// result to become.
pub fn call_new_index(
    receiver: &Value,
    key: Value,
    value: Value,
    span: std::ops::Range<usize>,
) -> Result<(), RuntimeError> {
    let method = saule_ast::ops::OP_NEW_INDEX.method;
    guarded(receiver, method, &span, || {
        invoke_method_multi(
            receiver,
            method,
            vec![
                EvaluatedArg::Positional(key),
                EvaluatedArg::Positional(value),
            ],
            span.clone(),
        )?;
        Ok(())
    })
}
