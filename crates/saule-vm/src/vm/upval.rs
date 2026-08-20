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

use std::cell::RefCell;
use std::rc::Rc;

use super::Vm;

impl Vm {

    // ---- upvalues ------------------------------------------------------

    pub(crate) fn capture_upvalue(&mut self, index: u32) -> Rc<RefCell<Upvalue>> {
        match self
            .open_upvals
            .binary_search_by_key(&Some(index), |u| u.borrow().stack_index())
        {
            Ok(i) => Rc::clone(&self.open_upvals[i]),
            Err(i) => {
                let cell = Rc::new(RefCell::new(Upvalue::Open(index)));
                self.open_upvals.insert(i, Rc::clone(&cell));
                cell
            }
        }
    }

    /// Close every open upvalue pointing at a register >= `from`. The value
    /// **moves** out of the register into the cell.
    pub(crate) fn close_upvalues(&mut self, from: u32) {
        while let Some(last) = self.open_upvals.last() {
            let Some(idx) = last.borrow().stack_index() else {
                self.open_upvals.pop();
                continue;
            };
            if idx < from {
                break;
            }
            let cell = self.open_upvals.pop().expect("checked");
            let v = std::mem::replace(&mut self.stack[idx as usize], Value::Nil);
            *cell.borrow_mut() = Upvalue::Closed(v);
        }
    }

}
