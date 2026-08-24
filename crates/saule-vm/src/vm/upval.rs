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

    /// The upvalue cell at `i` of the running closure.
    ///
    /// This is where the `Closure` downcast lives now. Three opcodes read
    /// upvalues and the other ninety-odd do not, so paying for it here costs
    /// those three a vtable compare and saves every call and every return
    /// one — see [`Frame::func`](super::Frame::func).
    pub(crate) fn upvalue(&self, i: usize) -> Rc<RefCell<Upvalue>> {
        let f = self.frames.last().expect("frame");
        let cl = super::Closure::from_handle(&f.func).expect("VM frame holds a Closure");
        Rc::clone(&cl.upvals[i])
    }

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
    ///
    /// Split in two so the common case is a load and a branch at the call
    /// site. Every `pop_frame` and every tail call runs this, while most
    /// programs capture nothing at all — `fib` and `oop` never build a
    /// closure — so the empty check is worth inlining and the loop is not.
    #[inline]
    pub(crate) fn close_upvalues(&mut self, from: u32) {
        if self.open_upvals.is_empty() {
            return;
        }
        self.close_upvalues_slow(from);
    }

    #[inline(never)]
    fn close_upvalues_slow(&mut self, from: u32) {
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
