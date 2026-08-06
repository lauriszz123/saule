//! Free lists for the two buffers every function call allocates.
//!
//! A Saule call is multi-return, so a call's result travels as a `Vec<Value>`
//! and its arguments as a `Vec<EvaluatedArg>`. Both are born and die inside
//! one call — the argument vector dies once the parameters are bound, the
//! result vector once [`first_or_nil`](crate::eval::expr) has taken element
//! zero — and nearly every one of them holds a single element. Left alone
//! that is two `malloc`/`free` pairs per call, which profiling put among the
//! largest single costs in call-heavy code.
//!
//! Handing the spent buffer to a free list instead lets the next call reuse
//! the same allocation. Nothing here changes what a caller sees: a borrowed
//! buffer is always empty, and a returned one is cleared before it is filed.
//!
//! Both pools are thread-local, so an interpreter running on two threads
//! (the CLI runs the program on a worker) keeps two independent free lists
//! and needs no synchronization.

use std::cell::RefCell;

use crate::eval::expr::EvaluatedArg;
use crate::value::Value;

/// How many spent buffers of each kind to keep.
///
/// Call depth, not call count, sets how many are live at once, so a small
/// list covers the steady state. The cap is what stops one pathologically
/// deep call chain from pinning its buffers for the rest of the run.
const CAPACITY: usize = 64;

thread_local! {
    static VALUES: RefCell<Vec<Vec<Value>>> = const { RefCell::new(Vec::new()) };
    static ARGS: RefCell<Vec<Vec<EvaluatedArg>>> = const { RefCell::new(Vec::new()) };
}

/// An empty `Vec<Value>`, reusing a pooled allocation when one is free.
pub(crate) fn take_values() -> Vec<Value> {
    VALUES.with(|p| p.borrow_mut().pop()).unwrap_or_default()
}

/// A pooled `Vec<Value>` holding exactly `value` — the single-return shape
/// that almost every call takes.
pub(crate) fn values_of(value: Value) -> Vec<Value> {
    let mut out = take_values();
    out.push(value);
    out
}

/// File a finished `Vec<Value>` for reuse.
pub(crate) fn give_values(mut values: Vec<Value>) {
    // A vector that never allocated is not worth a slot, and clearing has to
    // happen before filing so the pooled buffer holds no `Value` alive.
    if values.capacity() == 0 {
        return;
    }
    values.clear();
    VALUES.with(|p| {
        let mut pool = p.borrow_mut();
        if pool.len() < CAPACITY {
            pool.push(values);
        }
    });
}

/// An empty `Vec<EvaluatedArg>`, reusing a pooled allocation when one is free.
pub(crate) fn take_args() -> Vec<EvaluatedArg> {
    ARGS.with(|p| p.borrow_mut().pop()).unwrap_or_default()
}

/// File a finished `Vec<EvaluatedArg>` for reuse.
pub(crate) fn give_args(mut args: Vec<EvaluatedArg>) {
    if args.capacity() == 0 {
        return;
    }
    args.clear();
    ARGS.with(|p| {
        let mut pool = p.borrow_mut();
        if pool.len() < CAPACITY {
            pool.push(args);
        }
    });
}
