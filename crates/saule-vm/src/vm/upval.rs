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

use saule_runtime::Value;

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

    // ---- leaving the register file ---------------------------------------
    //
    // An open upvalue is a register *index*, and an index only means
    // something in one register file. Runtime code reached from the
    // dispatch loop — a native, an operator overload, a `toString`, a
    // getter — can call a Saule function back, and that call runs on a
    // *different* register file (a fresh `Vm` over the same shared state).
    // A closure it runs, reading an upvalue still open over *this* file,
    // would read or write the other file's register at that index: the
    // caller never sees the write, and the callback's own temporaries are
    // clobbered.
    //
    // So the VM keeps one invariant: **every open upvalue points into the
    // register file that is running.** Before control leaves for code that
    // may call back, [`park_upvalues`](Self::park_upvalues) copies each open
    // value into its cell and closes it, so whoever runs next reads and
    // writes the cell. When control comes back,
    // [`unpark_upvalues`](Self::unpark_upvalues) writes each cell back into
    // its register and reopens it.
    //
    // Registers are *copied* rather than moved out, because a native's
    // arguments are a borrow of this very register file and a parked
    // local may be one of them. The copy is stale until unparking
    // overwrites it, and nothing reads it in between: this file's frames
    // do not run until control comes back.
    //
    // A coroutine is the same situation stretched across a `yield`: a
    // suspended coroutine's register file is not running, so its upvalues
    // are parked for as long as it is suspended.

    /// Run `f` — a call out to runtime code that may re-enter the VM — with
    /// this register file's open upvalues parked.
    ///
    /// Costs one load and a branch when nothing is open, which is every
    /// program that never captures a local. Restored whether `f` succeeds
    /// or fails: a `catch` in this file's frames resumes in them.
    #[inline(always)]
    pub(crate) fn foreign<R>(&mut self, f: impl FnOnce(&mut Vm) -> R) -> R {
        if self.open_upvals.is_empty() {
            return f(self);
        }
        self.park_upvalues();
        let r = f(self);
        self.unpark_upvalues();
        r
    }

    /// Close every open upvalue into its cell, remembering where each one
    /// was so [`unpark_upvalues`](Self::unpark_upvalues) can reopen it.
    #[inline(never)]
    pub(crate) fn park_upvalues(&mut self) {
        debug_assert!(self.parked.is_empty(), "register file parked twice");
        for cell in &self.open_upvals {
            let mut cell = cell.borrow_mut();
            let idx = cell
                .stack_index()
                .expect("the open list holds only open upvalues");
            *cell = Upvalue::Closed(self.stack[idx as usize].clone());
            self.parked.push(idx);
        }
    }

    /// Undo [`park_upvalues`](Self::park_upvalues): each cell's current value
    /// — possibly written while parked — goes back into its register, and
    /// the upvalue is open again.
    #[inline(never)]
    pub(crate) fn unpark_upvalues(&mut self) {
        debug_assert_eq!(self.parked.len(), self.open_upvals.len());
        for (cell, idx) in self.open_upvals.iter().zip(self.parked.drain(..)) {
            let mut cell = cell.borrow_mut();
            let Upvalue::Closed(v) = std::mem::replace(&mut *cell, Upvalue::Open(idx)) else {
                unreachable!("nothing reopens a parked upvalue but its own register file");
            };
            self.stack[idx as usize] = v;
        }
    }

    /// Close every upvalue this register file still has open, because the
    /// file is being abandoned with frames on it — an error escaped a
    /// re-entrant call. A closure that captured one of those registers and
    /// escaped keeps the value; left open, it would read whichever register
    /// file next ran at that index.
    pub(crate) fn abandon(&mut self) {
        self.close_upvalues(0);
        self.frames.clear();
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
