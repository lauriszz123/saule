//! Calls and frames: getting into one, getting out, and finding the method.
//!
//! A frame is a base index into one contiguous register file, so entering a
//! call is a bump and a push rather than an allocation (§6.1). Tail calls
//! reuse the frame they are in; natives never get one at all.

use std::rc::Rc;

use saule_interpreter::value::VmFunctionRef;
use saule_interpreter::{RuntimeError, Value};

use crate::chunk::{Chunk, InlineCache, Proto};
use crate::op::{Instruction, Op};

use super::ops::operand_err;
use super::{ALL_RESULTS, Closure, Frame, Vm};

/// Where an error raised on the call path should point, *before* anyone has
/// paid to work that out.
///
/// [`Proto::span_at`] is a binary search over the line table, and every call
/// opcode was running one to build a span for an error it almost never
/// raises — `fib(30)` performed 2.7M searches and discarded 2.7M spans.
/// Carrying `span_at`'s two inputs instead defers the search to the error
/// branch, where it is free.
pub(crate) enum Site<'a> {
    /// A bytecode call site: the running proto and the pc of the call.
    Code(&'a Proto, u32),
    /// A span the caller already holds — the entry points reached from
    /// outside any proto, where there is no line table to search.
    Known(std::ops::Range<usize>),
}

impl Site<'_> {
    /// Materialise the span. Reached only while building a `RuntimeError`,
    /// which is why the search it may run costs nothing on the hot path.
    #[cold]
    #[inline(never)]
    pub(crate) fn span(&self) -> std::ops::Range<usize> {
        match self {
            Site::Code(proto, pc) => proto.span_at(*pc),
            Site::Known(span) => span.clone(),
        }
    }
}

impl Vm {

    /// [`vtable_lookup`](Self::vtable_lookup) behind a per-call-site
    /// monomorphic inline cache (§8.5).
    ///
    /// The uncached probe is `Rc::as_ptr`, a hash, a map probe and two
    /// indexed loads, and it was 9.3% of `bintree` and 7.7% of `interp`.
    /// A call site almost always sees one receiver class, so remembering
    /// the last one turns all of that into a pointer compare.
    ///
    /// **Keyed by `pc`.** One call site is one instruction, so the program
    /// counter identifies it without an operand — `CALLM` is `ABC` with no
    /// room for a cache index, and a cache index would have been a chunk
    /// ABI change for something that is pure runtime scratch.
    ///
    /// Sound without invalidation: Saule has no metatables and no runtime
    /// class mutation, so `(class, slot) -> (module, proto)` is permanently
    /// valid once observed. Holding the `Rc` rather than a raw pointer is
    /// what keeps the pointer compare honest — see [`InlineCache`].
    pub(crate) fn vtable_lookup_cached(
        &self,
        recv: usize,
        slot: usize,
        proto: &Proto,
        here: u32,
    ) -> Result<(usize, u32), RuntimeError> {
        let site = here as usize;
        {
            let Value::Instance(inst) = &self.stack[recv] else {
                return Err(operand_err(&self.stack[recv], "instance", proto, here));
            };
            let inst = inst.borrow();
            if let Some(InlineCache::Mono { class, module, target }) =
                proto.caches.borrow().get(site)
                && Rc::ptr_eq(class, &inst.class)
            {
                return Ok((*module as usize, *target));
            }
        }

        // Miss: the full probe, then remember it. A polymorphic site simply
        // keeps missing and pays one pointer compare for the privilege.
        let (module, target) = self.vtable_lookup(recv, slot, proto, here)?;
        if let Value::Instance(inst) = &self.stack[recv]
            && let Ok(module16) = u16::try_from(module)
        {
            let class = Rc::clone(&inst.borrow().class);
            let mut caches = proto.caches.borrow_mut();
            if caches.len() < proto.code.len() {
                caches.resize(proto.code.len(), InlineCache::Empty);
            }
            caches[site] = InlineCache::Mono { class, module: module16, target };
        }
        Ok((module, target))
    }

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
        // The identity is the pointer, so the probe needs no strong count of
        // its own — cloning the `Rc` here cost a bump and a drop on every
        // dynamic dispatch. The borrow ends with the statement; the error
        // path re-borrows, because it is the path that is allowed to be slow.
        let key = Rc::as_ptr(&inst.borrow().class) as usize;
        let idx = self
            .shared.class_of
            .get(&key)
            .copied()
            .ok_or_else(|| RuntimeError::TypeError {
                message: format!(
                    "internal: `{}` was not built from this chunk",
                    inst.borrow().class.name
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
                    inst.borrow().class.name
                ),
                span: proto.span_at(here),
            })
    }

    /// [`itable_lookup`](Self::itable_lookup) behind the same per-call-site
    /// cache `CALLM` uses (§8.4).
    ///
    /// Interface dispatch has *two* probes to skip rather than one — the
    /// `class_of` map and then the itable — so a hit is worth strictly more
    /// here than on a vtable call.
    pub(crate) fn itable_lookup_cached(
        &self,
        recv: usize,
        iface: u32,
        slot: usize,
        proto: &Proto,
        here: u32,
    ) -> Result<(usize, u32), RuntimeError> {
        let site = here as usize;
        {
            let Value::Instance(inst) = &self.stack[recv] else {
                return Err(operand_err(&self.stack[recv], "instance", proto, here));
            };
            let inst = inst.borrow();
            if let Some(InlineCache::Mono { class, module, target }) =
                proto.caches.borrow().get(site)
                && Rc::ptr_eq(class, &inst.class)
            {
                return Ok((*module as usize, *target));
            }
        }
        let (module, target) = self.itable_lookup(recv, iface, slot, proto, here)?;
        if let Value::Instance(inst) = &self.stack[recv]
            && let Ok(module16) = u16::try_from(module)
        {
            let class = Rc::clone(&inst.borrow().class);
            let mut caches = proto.caches.borrow_mut();
            if caches.len() < proto.code.len() {
                caches.resize(proto.code.len(), InlineCache::Empty);
            }
            caches[site] = InlineCache::Mono { class, module: module16, target };
        }
        Ok((module, target))
    }

    /// Resolve an interface method slot against the receiver's class.
    ///
    /// The itable was built once, when the class was laid out, so this is a
    /// map probe and two indexed loads — not a name lookup.
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
        let key = Rc::as_ptr(&inst.borrow().class) as usize;
        let idx = self
            .shared.class_of
            .get(&key)
            .copied()
            .ok_or_else(|| RuntimeError::TypeError {
                message: format!(
                    "internal: `{}` was not built from this chunk",
                    inst.borrow().class.name
                ),
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
                    inst.borrow().class.name
                ),
                span: proto.span_at(here),
            })?;
        cp.vtable
            .get(vslot as usize)
            .copied()
            .filter(|t| *t != u32::MAX)
            .map(|t| (cp.module, t))
            .ok_or_else(|| RuntimeError::TypeError {
                message: format!(
                    "internal: `{}` has no method in slot {vslot}",
                    inst.borrow().class.name
                ),
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
        site: &Site<'_>,
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
                    site,
                )?;
                Ok(true)
            }
            other => {
                self.call_native(&other, callee_abs, n_args, n_ret, site)?;
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
        site: &Site<'_>,
    ) -> Result<(), RuntimeError> {
        let args_from = dst + 1;
        match callee {
            Value::Native(nf) => {
                let v = (nf.func)(&self.stack[args_from..args_from + n_args])
                    .map_err(|m| RuntimeError::TypeError { message: m, span: site.span() })?;
                self.store_results(dst, std::slice::from_ref(&v), n_ret);
                Ok(())
            }
            Value::NativeClosure(nc) => {
                let vs = (nc.func)(&self.stack[args_from..args_from + n_args])
                    .map_err(|m| RuntimeError::TypeError { message: m, span: site.span() })?;
                self.store_results(dst, &vs, n_ret);
                Ok(())
            }
            other => Err(RuntimeError::TypeError {
                message: format!("attempt to call a `{}`", other.type_name()),
                span: site.span(),
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
        site: &Site<'_>,
    ) -> Result<(), RuntimeError> {
        let cl = Closure::from_handle(&handle).ok_or_else(|| RuntimeError::TypeError {
            message: "internal: frame handle is not a bytecode closure".into(),
            span: site.span(),
        })?;
        let proto = Rc::clone(&cl.proto);
        let chunk = Rc::clone(&cl.chunk);
        let pdata = &*proto;

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

        let top = base + pdata.max_regs as usize;
        self.claim_registers(top);
        // Same rule as `push_frame`: missing parameters read as nil, and
        // registers past `n_params` are temporaries the callee writes before
        // it reads. Here it matters more than there — the frame is *dirty*,
        // holding the previous call's values rather than fresh stack.
        for i in n_args..pdata.n_params as usize {
            self.stack[base + i] = Value::Nil;
        }
        let n_args = n_args.min(u8::MAX as usize) as u8;
        let pc = pdata.entry_for(n_args);
        *self.frames.last_mut().expect("frame") = Frame {
            func: handle,
            proto,
            chunk,
            base: base as u32,
            ret_to,
            n_ret,
            pc,
            top: top as u32,
            n_args,
        };
        Ok(())
    }

    /// Inlined into its one real body below: the split exists so a resolved
    /// caller can skip the downcast, not to add a call to the path that
    /// still needs it. Dynamic `CALL` — a lambda through a local — comes
    /// through here.
    ///
    /// `inline`, not `inline(always)`: forcing the whole body into the two
    /// call sites cost `closure` 12% — the dispatch loop is already large
    /// enough that the extra code hurts more than the saved frame helps.
    #[inline]
    pub(crate) fn push_frame(
        &mut self,
        func: Rc<VmFunctionRef>,
        base: u32,
        n_args: usize,
        ret_to: u32,
        n_ret: u8,
        site: &Site<'_>,
    ) -> Result<(), RuntimeError> {
        if self.frames.len() >= self.shared.max_frames {
            return Err(RuntimeError::StackOverflow {
                limit: self.shared.max_frames as u32,
                span: site.span(),
            });
        }
        let cl = Closure::from_handle(&func).ok_or_else(|| RuntimeError::TypeError {
            message: "internal: frame handle is not a bytecode closure".into(),
            span: site.span(),
        })?;
        let (proto, chunk) = (Rc::clone(&cl.proto), Rc::clone(&cl.chunk));
        self.push_frame_resolved(func, proto, chunk, base, n_args, ret_to, n_ret, site)
    }

    /// [`push_frame`](Self::push_frame) for a caller that already holds the
    /// proto and chunk, so the handle does not have to be downcast to
    /// recover them.
    ///
    /// Every statically-resolved call site is in that position — it looked
    /// both up to find the callee in the first place.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn push_frame_resolved(
        &mut self,
        func: Rc<VmFunctionRef>,
        proto: Rc<Proto>,
        chunk: Rc<Chunk>,
        base: u32,
        n_args: usize,
        ret_to: u32,
        n_ret: u8,
        site: &Site<'_>,
    ) -> Result<(), RuntimeError> {
        if self.frames.len() >= self.shared.max_frames {
            return Err(RuntimeError::StackOverflow {
                limit: self.shared.max_frames as u32,
                span: site.span(),
            });
        }
        let pdata = &*proto;
        let top = base as usize + pdata.max_regs as usize;
        self.claim_registers(top);
        // Missing parameters read as nil. Registers past `n_params` are the
        // compiler's temporaries and are always written before read, so they
        // are left alone rather than memset per call.
        for i in n_args..pdata.n_params as usize {
            self.stack[base as usize + i] = Value::Nil;
        }
        let pc = pdata.entry_for(n_args.min(u8::MAX as usize) as u8);
        self.frames.push(Frame {
            func,
            proto,
            chunk,
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
    /// caller. Returns `true` when the outermost frame returned, in which
    /// case the values are the program's result and are left in
    /// [`Vm::results`](super::Vm).
    /// Infallible, and typed that way on purpose: `RuntimeError` is 64
    /// bytes, so a `Result` return made every `RET` construct and test a
    /// 64-byte value to carry an error this function cannot produce.
    pub(crate) fn pop_frame(&mut self, src: usize, count: usize) -> bool {
        let frame = self.frames.pop().expect("frame");
        self.close_upvalues(frame.base);

        if self.frames.is_empty() {
            // Into the VM's own buffer, not a fresh `Vec`: this runs once
            // per re-entrant invocation, which on `Table.sort` is once per
            // comparison. See [`Vm::results`].
            self.results.clear();
            self.results.reserve(count);
            for i in 0..count {
                let v = std::mem::replace(&mut self.stack[src + i], Value::Nil);
                self.results.push(v);
            }
            return true;
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
        false
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
    ///
    /// Every call opcode that takes one runs this, and it used to re-decode
    /// the word and compare the opcode to prove what the verifier already
    /// proved: `expect_extra` in `verify_proto` rejects any chunk where an
    /// opcode that needs an `EXTRAARG` is not followed by one. So this reads
    /// the payload and moves on — the check is kept as a `debug_assert`, so
    /// the test suite still fails loudly if that invariant is ever broken.
    #[inline(always)]
    pub(crate) fn extra_arg(&self, code: *const Instruction, pc: &mut usize) -> u32 {
        // SAFETY: the word exists because it was verified to exist — a
        // proto can never end on an opcode that needs an `EXTRAARG`.
        let ins = unsafe { *code.add(*pc) };
        debug_assert_eq!(ins.op(), Some(Op::EXTRAARG), "verify_proto guarantees an EXTRAARG here");
        *pc += 1;
        ins.ax()
    }

    /// Resolve `(module, proto)` to its cached closure and enter it.
    ///
    /// Every statically-resolved call — `CALLK`, `CALLSTAT`, `CALLM`,
    /// `CALLI` and the `super` forms — repeated the same five lines, and
    /// two of them cost more than they read: the proto was cloned only to
    /// be handed to `closure_for`, which drops it on every cache hit, and
    /// `push_frame` then recovered the same proto and chunk from the handle
    /// by downcast. Resolving once and moving the results into the frame
    /// removes two refcount pairs and a downcast from every call.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn enter_static(
        &mut self,
        tm: usize,
        target: u32,
        base: u32,
        n_args: usize,
        ret_to: u32,
        n_ret: u8,
        site: &Site<'_>,
    ) -> Result<(), RuntimeError> {
        let chunk = Rc::clone(&self.shared.chunks[tm]);
        let proto = Rc::clone(chunk.proto(target));
        let handle = self.closure_for(&chunk, target);
        self.push_frame_resolved(handle, proto, chunk, base, n_args, ret_to, n_ret, site)
    }

    /// A cached, upvalue-free closure for a statically-resolved callee, so
    /// `CALLK` allocates nothing per call.
    pub(crate) fn closure_for(&self, chunk: &Rc<Chunk>, idx: u32) -> Rc<VmFunctionRef> {
        let m = chunk.module_index;
        Rc::clone(self.shared.closure_cache[m][idx as usize].get_or_init(|| {
            // Only on the first visit to this call site. Cloning the proto
            // here rather than in the caller keeps the hit path — which is
            // every call after the first — down to two indexed loads and a
            // refcount bump.
            VmFunctionRef::new(Closure::bound(
                Rc::clone(chunk.proto(idx)),
                Rc::clone(chunk),
                Vec::new(),
                &self.shared,
            ))
        }))
    }

}
