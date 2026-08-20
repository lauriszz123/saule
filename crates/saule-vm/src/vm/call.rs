//! Calls and frames: getting into one, getting out, and finding the method.
//!
//! A frame is a base index into one contiguous register file, so entering a
//! call is a bump and a push rather than an allocation (§6.1). Tail calls
//! reuse the frame they are in; natives never get one at all.

use std::rc::Rc;

use saule_interpreter::value::VmFunctionRef;
use saule_interpreter::{RuntimeError, Value};

use crate::chunk::{Chunk, Proto};
use crate::op::{Instruction, Op};

use super::ops::operand_err;
use super::{ALL_RESULTS, Closure, Frame, Vm};

impl Vm {

    /// Resolve a vtable slot against the class of the instance in `recv`.
    pub(crate) fn vtable_lookup(
        &self,
        recv: usize,
        slot: usize,
        proto: &Proto,
        here: u32,
    ) -> Result<(usize, u32), RuntimeError> {
        let Value::Instance(inst) = &self.stack[recv] else {
            return Err(operand_err(&self.stack[recv], "instance", proto, here));
        };
        let class = Rc::clone(&inst.borrow().class);
        let idx = self
            .shared.class_of
            .get(&(Rc::as_ptr(&class) as usize))
            .copied()
            .ok_or_else(|| RuntimeError::TypeError {
                message: format!(
                    "internal: `{}` was not built from this chunk",
                    class.name
                ),
                span: proto.span_at(here),
            })?;
        // The prefix invariant at work: a slot resolved against a parent's
        // vtable indexes the subclass's override, because a subclass's
        // vtable extends its parent's rather than reordering it (§8.3).
        let cp = &self.shared.chunks[0].classes[idx as usize];
        cp.vtable
            .get(slot)
            .copied()
            .filter(|t| *t != u32::MAX)
            .map(|t| (cp.module, t))
            .ok_or_else(|| RuntimeError::TypeError {
                message: format!(
                    "internal: `{}` has no method in vtable slot {slot}",
                    class.name
                ),
                span: proto.span_at(here),
            })
    }

    /// Resolve an interface method slot against the receiver's class.
    ///
    /// The itable was built once, when the class was laid out, so this is a
    /// map probe and two indexed loads — not a name lookup. §8.4 adds a
    /// one-entry inline cache on top; call sites on interface receivers are
    /// overwhelmingly monomorphic, so that collapses the probe to a pointer
    /// compare. That is Phase 5, with a benchmark.
    pub(crate) fn itable_lookup(
        &self,
        recv: usize,
        iface: u32,
        slot: usize,
        proto: &Proto,
        here: u32,
    ) -> Result<(usize, u32), RuntimeError> {
        let Value::Instance(inst) = &self.stack[recv] else {
            return Err(operand_err(&self.stack[recv], "instance", proto, here));
        };
        let class = Rc::clone(&inst.borrow().class);
        let idx = self
            .shared.class_of
            .get(&(Rc::as_ptr(&class) as usize))
            .copied()
            .ok_or_else(|| RuntimeError::TypeError {
                message: format!("internal: `{}` was not built from this chunk", class.name),
                span: proto.span_at(here),
            })?;
        let cp = &self.shared.chunks[0].classes[idx as usize];
        let vslot = cp
            .itables
            .get(&iface)
            .and_then(|t| t.get(slot))
            .copied()
            .ok_or_else(|| RuntimeError::TypeError {
                message: format!(
                    "`{}` does not implement the interface this call requires",
                    class.name
                ),
                span: proto.span_at(here),
            })?;
        cp.vtable
            .get(vslot as usize)
            .copied()
            .filter(|t| *t != u32::MAX)
            .map(|t| (cp.module, t))
            .ok_or_else(|| RuntimeError::TypeError {
                message: format!("internal: `{}` has no method in slot {vslot}", class.name),
                span: proto.span_at(here),
            })
    }


    // ---- calls ---------------------------------------------------------

    /// Returns `true` when a new bytecode frame was pushed and the caller
    /// must re-enter the dispatch loop.
    pub(crate) fn dispatch_call(
        &mut self,
        callee_abs: usize,
        n_args: usize,
        n_ret: u8,
        span: std::ops::Range<usize>,
        pc_after: usize,
    ) -> Result<bool, RuntimeError> {
        let callee = self.stack[callee_abs].clone();
        match callee {
            Value::VmFunction(handle) => {
                self.frames.last_mut().expect("frame").pc = pc_after as u32;
                self.push_frame(
                    handle,
                    (callee_abs + 1) as u32,
                    n_args,
                    callee_abs as u32,
                    n_ret,
                    span,
                )?;
                Ok(true)
            }
            other => {
                self.call_native(&other, callee_abs, n_args, n_ret, span)?;
                Ok(false)
            }
        }
    }

    /// Call a native, a native closure, or fail with the same message shape
    /// the tree-walker produces. Arguments are passed as a **borrow of the
    /// register file** — no `Vec`, no per-argument clone (§13).
    pub(crate) fn call_native(
        &mut self,
        callee: &Value,
        dst: usize,
        n_args: usize,
        n_ret: u8,
        span: std::ops::Range<usize>,
    ) -> Result<(), RuntimeError> {
        let args_from = dst + 1;
        match callee {
            Value::Native(nf) => {
                let v = (nf.func)(&self.stack[args_from..args_from + n_args]).map_err(|m| {
                    RuntimeError::TypeError { message: m, span: span.clone() }
                })?;
                self.store_results(dst, std::slice::from_ref(&v), n_ret);
                Ok(())
            }
            Value::NativeClosure(nc) => {
                let vs = (nc.func)(&self.stack[args_from..args_from + n_args]).map_err(|m| {
                    RuntimeError::TypeError { message: m, span: span.clone() }
                })?;
                self.store_results(dst, &vs, n_ret);
                Ok(())
            }
            other => Err(RuntimeError::TypeError {
                message: format!("attempt to call a `{}`", other.type_name()),
                span,
            }),
        }
    }

    /// **Replace** the running frame with a call to `handle` (§6.4).
    ///
    /// The frame being replaced is gone: its upvalues are closed and its
    /// registers are overwritten by the callee's arguments. What survives is
    /// `ret_to` and `n_ret` — the callee returns to whoever called the frame
    /// it replaced, so a tail chain costs **one** frame however long it runs,
    /// and multi-return keeps working through it for free.
    ///
    /// No depth check, deliberately: not consuming depth is the entire point.
    /// The tree-walker's trampoline is the same bargain — one `DepthGuard`
    /// held across the whole chain.
    pub(crate) fn enter_tail_frame(
        &mut self,
        handle: Rc<VmFunctionRef>,
        args_from: usize,
        n_args: usize,
        span: std::ops::Range<usize>,
    ) -> Result<(), RuntimeError> {
        let proto = Rc::clone(
            &Closure::from_handle(&handle)
                .ok_or_else(|| RuntimeError::TypeError {
                    message: "internal: frame handle is not a bytecode closure".into(),
                    span: span.clone(),
                })?
                .proto,
        );

        let frame = self.frames.last().expect("frame");
        let base = frame.base as usize;
        let (ret_to, n_ret) = (frame.ret_to, frame.n_ret);

        // A closure built in this frame must stop pointing at registers the
        // callee is about to overwrite. A tail call ends the frame just as
        // surely as a return does, so this is `pop_frame`'s rule, not an
        // extra precaution.
        self.close_upvalues(base as u32);

        // Arguments move down to `base`, which is where the callee's `R[0]`
        // has to be. `base <= args_from` always — the window is allocated
        // above the frame's locals — so an ascending move cannot clobber an
        // argument it has not read yet.
        debug_assert!(base <= args_from);
        if base != args_from {
            for i in 0..n_args {
                self.stack[base + i] =
                    std::mem::replace(&mut self.stack[args_from + i], Value::Nil);
            }
        }

        let top = base + proto.max_regs as usize;
        self.ensure_stack(top);
        // Same rule as `push_frame`: missing parameters read as nil, and
        // registers past `n_params` are temporaries the callee writes before
        // it reads. Here it matters more than there — the frame is *dirty*,
        // holding the previous call's values rather than fresh stack.
        for i in n_args..proto.n_params as usize {
            self.stack[base + i] = Value::Nil;
        }
        let n_args = n_args.min(u8::MAX as usize) as u8;
        let pc = proto.entry_for(n_args);
        *self.frames.last_mut().expect("frame") = Frame {
            func: handle,
            base: base as u32,
            ret_to,
            n_ret,
            pc,
            top: top as u32,
            n_args,
        };
        Ok(())
    }

    pub(crate) fn push_frame(
        &mut self,
        func: Rc<VmFunctionRef>,
        base: u32,
        n_args: usize,
        ret_to: u32,
        n_ret: u8,
        span: std::ops::Range<usize>,
    ) -> Result<(), RuntimeError> {
        if self.frames.len() >= self.shared.max_frames {
            return Err(RuntimeError::StackOverflow {
                limit: self.shared.max_frames as u32,
                span,
            });
        }
        let proto = Rc::clone(
            &Closure::from_handle(&func)
                .ok_or_else(|| RuntimeError::TypeError {
                    message: "internal: frame handle is not a bytecode closure".into(),
                    span: span.clone(),
                })?
                .proto,
        );
        let top = base as usize + proto.max_regs as usize;
        self.ensure_stack(top);
        // Missing parameters read as nil. Registers past `n_params` are the
        // compiler's temporaries and are always written before read, so they
        // are left alone rather than memset per call.
        for i in n_args..proto.n_params as usize {
            self.stack[base as usize + i] = Value::Nil;
        }
        let pc = proto.entry_for(n_args.min(u8::MAX as usize) as u8);
        self.frames.push(Frame {
            func,
            base,
            ret_to,
            n_ret,
            pc,
            top: top as u32,
            n_args: n_args.min(u8::MAX as usize) as u8,
        });
        Ok(())
    }

    /// Pop the active frame, moving `count` results from `src` into the
    /// caller. Returns `Some` when the outermost frame returned, in which
    /// case the values are the program's result.
    pub(crate) fn pop_frame(&mut self, src: usize, count: usize) -> Result<Option<Vec<Value>>, RuntimeError> {
        let frame = self.frames.pop().expect("frame");
        self.close_upvalues(frame.base);

        if self.frames.is_empty() {
            let mut out = Vec::with_capacity(count);
            for i in 0..count {
                out.push(std::mem::replace(&mut self.stack[src + i], Value::Nil));
            }
            return Ok(Some(out));
        }

        let dst = frame.ret_to as usize;
        // `dst < src` always — the callee's base is the caller's `A + 1` at
        // the lowest — so a forward move never clobbers an unread source.
        debug_assert!(dst <= src);
        let wanted = if frame.n_ret == ALL_RESULTS { count } else { frame.n_ret as usize };
        self.ensure_stack(dst + wanted);
        for i in 0..wanted {
            self.stack[dst + i] = if i < count {
                std::mem::replace(&mut self.stack[src + i], Value::Nil)
            } else {
                Value::Nil
            };
        }
        if frame.n_ret == ALL_RESULTS {
            self.frames.last_mut().expect("frame").top = (dst + count) as u32;
        }
        Ok(None)
    }

    pub(crate) fn store_results(&mut self, dst: usize, vals: &[Value], n_ret: u8) {
        let wanted = if n_ret == ALL_RESULTS { vals.len() } else { n_ret as usize };
        self.ensure_stack(dst + wanted);
        for i in 0..wanted {
            self.stack[dst + i] = vals.get(i).cloned().unwrap_or(Value::Nil);
        }
        if n_ret == ALL_RESULTS && let Some(f) = self.frames.last_mut() {
            f.top = (dst + vals.len()) as u32;
        }
    }

    /// `B = nargs + 1`; `B = 0` means "arguments run from `from` to `top`".
    pub(crate) fn arg_count(&self, b: u8, from: usize) -> usize {
        if b == 0 {
            (self.frames.last().map_or(from, |f| f.top as usize)).saturating_sub(from)
        } else {
            (b - 1) as usize
        }
    }

    /// Read the `EXTRAARG` word following the current instruction.
    pub(crate) fn extra_arg(
        &self,
        code: &[Instruction],
        pc: &mut usize,
        proto: &Proto,
        here: u32,
    ) -> Result<u32, RuntimeError> {
        let ins = code.get(*pc).copied().unwrap_or(Instruction(0));
        if ins.op() != Some(Op::EXTRAARG) {
            return Err(RuntimeError::TypeError {
                message: "internal: instruction requires a following EXTRAARG".into(),
                span: proto.span_at(here),
            });
        }
        *pc += 1;
        Ok(ins.ax())
    }

    /// A cached, upvalue-free closure for a statically-resolved callee, so
    /// `CALLK` allocates nothing per call.
    pub(crate) fn closure_for(&mut self, chunk: &Rc<Chunk>, idx: u32, proto: Rc<Proto>) -> Rc<VmFunctionRef> {
        let m = chunk.module_index;
        // Hit path first, under a shared borrow: a `borrow_mut` here would
        // be taken on every call, not just the first.
        if let Some(Some(c)) = self.shared.closure_cache.borrow().get(m).and_then(|r| r.get(idx as usize)) {
            return Rc::clone(c);
        }
        let c = VmFunctionRef::new(Closure::bound(proto, Rc::clone(chunk), Vec::new(), &self.shared));
        if let Some(slot) = self
            .shared
            .closure_cache
            .borrow_mut()
            .get_mut(m)
            .and_then(|r| r.get_mut(idx as usize))
        {
            *slot = Some(Rc::clone(&c));
        }
        c
    }

}
