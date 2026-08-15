//! Open/closed upvalues (`VM_DESIGN.md` §7).
//!
//! This is the exact version of what `saule-interpreter`'s `capture.rs` does
//! approximately. An **open** upvalue points into the live register stack, so
//! the closure and the enclosing frame observe each other's writes — the
//! live-binding semantics `Environment::capture_flat` protects. When the
//! enclosing scope exits, `CLOSEUP` **closes** every upvalue at or above a
//! register: the value moves out of the stack and into the cell.
//!
//! That is also what gives per-iteration capture for free (§7.2): closing at
//! the bottom of a loop body freezes that iteration's value, and the next
//! iteration reuses the register under a fresh open upvalue. No allocation
//! when nothing captures, and no `strong_count` probe.

use saule_interpreter::Value;

#[derive(Debug)]
pub enum Upvalue {
    /// Absolute index into the VM's register stack.
    Open(u32),
    Closed(Value),
}

impl Upvalue {
    pub fn stack_index(&self) -> Option<u32> {
        match self {
            Upvalue::Open(i) => Some(*i),
            Upvalue::Closed(_) => None,
        }
    }
}
