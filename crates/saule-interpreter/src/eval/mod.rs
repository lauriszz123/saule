//! Statement & expression evaluation.
//!
//! Organized into:
//!   * [`expr`] — pure expression evaluation
//!   * [`stmt`] — statement execution with control-flow propagation
//!   * [`ops`]  — operator helpers (arithmetic / comparison / equality)
//!
//! The [`Flow`] enum is how `break` / `continue` / `return` propagate up
//! through nested blocks without using panics or special error variants.

use crate::value::Value;

pub mod coerce;
pub mod expr;
pub mod index_hooks;
pub mod ops;
pub mod stmt;

/// Maximum nesting depth for Saule function calls.
///
/// The interpreter walks the AST recursively, so one Saule call costs several
/// native frames (`call_function_multi` → `run_function_body_multi` →
/// `exec_block` → `exec` → `eval` → …). Unbounded Saule recursion therefore
/// exhausts the *native* stack, and a native stack overflow is not a Rust
/// panic — it is `SIGSEGV`/`SIGABRT`, which cannot be caught, prints no span,
/// and gives `try ... catch` no chance to run. In the language server it kills
/// the whole session.
///
/// This limit is deliberately well under what the default 8 MiB main-thread
/// stack can take (measured: the abort lands around 20k frames in a debug
/// build, which is the tighter of the two since debug frames are larger), so
/// the error fires with room to spare and the diagnostic machinery — itself
/// recursive over the error chain — still has stack to run on.
///
/// Override with `SAULE_MAX_DEPTH` when a legitimately deep workload needs it
/// *and* the stack has been grown to match.
pub const MAX_EVAL_DEPTH: u32 = 10_000;

thread_local! {
    static EVAL_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    static DEPTH_LIMIT: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// The active depth limit: `SAULE_MAX_DEPTH` if set and parseable, else
/// [`MAX_EVAL_DEPTH`]. Read from the environment once per thread.
pub fn depth_limit() -> u32 {
    DEPTH_LIMIT.with(|c| {
        let cached = c.get();
        if cached != 0 {
            return cached;
        }
        let limit = std::env::var("SAULE_MAX_DEPTH")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(MAX_EVAL_DEPTH);
        c.set(limit);
        limit
    })
}

/// Enter one call level against the shared recursion counter, returning a
/// guard that releases it on drop.
///
/// Public because `saule-vm` needs the *same* counter. A re-entrant VM call
/// — a native invoking a bytecode comparator, an operator overload — is one
/// native stack frame per level, and the VM's own `max_frames` counts frames
/// inside a single `Vm`, so it cannot see that nesting at all. Sharing this
/// counter is also what bounds a program bouncing between the two engines
/// once rather than twice.
pub fn enter_call_depth(
    span: &std::ops::Range<usize>,
) -> Result<DepthGuard, crate::error::RuntimeError> {
    DepthGuard::enter(span)
}

/// RAII depth counter. Decrements on every exit path — including the `?`
/// early-returns that are how errors leave the evaluator — so the count can
/// never drift upward across a caught `throw`.
pub struct DepthGuard;

impl DepthGuard {
    /// Enter one call level, or fail if that would exceed the limit.
    ///
    /// On failure the depth is *not* incremented, so the error propagates
    /// out through frames that each decrement exactly once.
    pub(crate) fn enter(span: &std::ops::Range<usize>) -> Result<Self, crate::error::RuntimeError> {
        let limit = depth_limit();
        let depth = EVAL_DEPTH.with(|d| {
            let next = d.get() + 1;
            if next > limit {
                return None;
            }
            d.set(next);
            Some(next)
        });
        match depth {
            Some(_) => Ok(DepthGuard),
            None => Err(crate::error::RuntimeError::StackOverflow {
                limit,
                span: span.clone(),
            }),
        }
    }
}

impl Drop for DepthGuard {
    fn drop(&mut self) {
        EVAL_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

/// Reset the recursion counter. Called when starting a fresh top-level run so
/// a previous program's aborted stack can't leak into the next one — the REPL
/// and the language server both reuse a thread.
pub fn reset_depth() {
    EVAL_DEPTH.with(|d| d.set(0));
}

/// Result of executing a single statement (or a block of statements).
///
/// `Normal(v)` means execution should continue; `v` is the value of the
/// last expression-statement (used at the REPL and in tests).
#[derive(Debug, Clone)]
pub enum Flow {
    Normal(Value),
    Break,
    Continue,
    Return(Vec<Value>),
}

impl Flow {
    /// Convenience for the common "nothing interesting happened" case.
    pub fn nil() -> Self {
        Flow::Normal(Value::Nil)
    }

    /// Returns the inner value if this is a `Normal` outcome, else `Nil`.
    pub fn into_value(self) -> Value {
        match self {
            Flow::Normal(v) => v,
            Flow::Return(values) => values.into_iter().next().unwrap_or(Value::Nil),
            _ => Value::Nil,
        }
    }
}
