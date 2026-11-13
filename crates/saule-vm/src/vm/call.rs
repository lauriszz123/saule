//! Calls and frames: getting into one, getting out, and finding the method.
//!
//! A frame is a base index into one contiguous register file, so entering a
//! call is a bump and a push rather than an allocation (§6.1). Tail calls
//! reuse the frame they are in; natives never get one at all.

use std::rc::Rc;

use saule_runtime::value::VmFunctionRef;
use saule_runtime::{RuntimeError, Value};

use crate::chunk::{Chunk, InlineCache, Proto};
use crate::op::{Instruction, Op};

use super::frame::{Held, hold};
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
    /// A call site almost always sees one receiver class, so remembering it
    /// turns all of that into a pointer compare — and a site that sees a
    /// few, in turn, remembers each of them rather than evicting one per
    /// call (see [`InlineCache`]).
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
        if let Some(hit) = self.cached_callee(recv, proto, here)? {
            return Ok(hit);
        }
        let (module, target) = self.vtable_lookup(recv, slot, proto, here)?;
        self.remember_callee(recv, proto, here, module, target);
        Ok((module, target))
    }

    /// The call-site cache's answer for the receiver in `recv`, if this site
    /// has seen its class.
    ///
    /// Shared by `CALLM` and `CALLIF`: both cache the same thing, the callee
    /// a receiver class resolves to, and differ only in the probe a miss
    /// falls back to.
    ///
    /// `inline(always)`, because a plain `inline` was not taken: this came
    /// out as a function of its own, so every *hit* — nearly every method
    /// call in a program — paid a call and a `Result<Option<_>, RuntimeError>`
    /// (72 bytes, `RuntimeError` being 64) written to memory and read back.
    /// That was `oop` +4.6%, and it is exactly the path the cache exists to
    /// make cheap.
    #[inline(always)]
    fn cached_callee(
        &self,
        recv: usize,
        proto: &Proto,
        here: u32,
    ) -> Result<Option<(usize, u32)>, RuntimeError> {
        let Value::Instance(inst) = &self.stack[recv] else {
            return Err(operand_err(&self.stack[recv], "instance", proto, here));
        };
        let inst = inst.borrow();
        Ok(proto
            .caches
            .borrow()
            .get(here as usize)
            .and_then(|c| c.probe(&inst.class))
            .map(|(module, target)| (module as usize, target)))
    }

    /// Record a miss: the receiver in `recv` resolves to `(module, target)`
    /// at this site — unless the site has seen so many classes that it has
    /// stopped learning, in which case nothing is cloned or written.
    ///
    /// Out of line and `cold` on purpose. A site misses a handful of times
    /// in its life and hits on every other call, and the code here — the
    /// widening to a polymorphic cache, with its `Box` — inlined into the
    /// `CALLM` path made the *hit* slower: `oop` +4.6%, `bintree` +2.7%,
    /// `vecops` +2.9%, programs whose sites never miss after the first call.
    #[cold]
    #[inline(never)]
    fn remember_callee(&self, recv: usize, proto: &Proto, here: u32, module: usize, target: u32) {
        let Value::Instance(inst) = &self.stack[recv] else {
            return;
        };
        let Ok(module16) = u16::try_from(module) else {
            return;
        };
        let site = here as usize;
        let mut caches = proto.caches.borrow_mut();
        if caches.get(site).is_some_and(|c| !c.learns()) {
            return;
        }
        if caches.len() < proto.code.len() {
            caches.resize(proto.code.len(), InlineCache::Empty);
        }
        caches[site].record(Rc::clone(&inst.borrow().class), module16, target);
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
        let idx =
            self.shared
                .class_of
                .get(&key)
                .copied()
                .ok_or_else(|| RuntimeError::Internal {
                    message: format!(
                        "internal: `{}` was not built from this chunk",
                        inst.borrow().class.name
                    ),
                    span: proto.span_at(here),
                })?;
        // The prefix invariant at work: a slot resolved against a parent's
        // vtable indexes the subclass's override, because a subclass's
        // vtable extends its parent's rather than reordering it (§8.3).
        let table = &self.shared.chunks[0].classes;
        let cp = &table[idx as usize];
        cp.vtable
            .get(slot)
            .copied()
            .filter(|t| *t != u32::MAX)
            // The module of the class that **declared** the body, which is
            // not `cp` when the slot was inherited: a `ProtoIdx` only means
            // something inside its own chunk (`ClassProto::vowner`).
            .zip(cp.vowner.get(slot).map(|o| table[*o as usize].module))
            .map(|(t, m)| (m, t))
            .ok_or_else(|| RuntimeError::Internal {
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
        if let Some(hit) = self.cached_callee(recv, proto, here)? {
            return Ok(hit);
        }
        let (module, target) = self.itable_lookup(recv, iface, slot, proto, here)?;
        self.remember_callee(recv, proto, here, module, target);
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
        let idx =
            self.shared
                .class_of
                .get(&key)
                .copied()
                .ok_or_else(|| RuntimeError::Internal {
                    message: format!(
                        "internal: `{}` was not built from this chunk",
                        inst.borrow().class.name
                    ),
                    span: proto.span_at(here),
                })?;
        let table = &self.shared.chunks[0].classes;
        let cp = &table[idx as usize];
        let vslot = cp
            .itables
            .get(&iface)
            .and_then(|t| t.get(slot))
            .copied()
            .ok_or_else(|| {
                let class = inst.borrow().class.name.clone();
                RuntimeError::fault(
                    format!("`{class}` does not implement the interface this call goes through"),
                    format!("this is {}", saule_runtime::error::a_type(&class)),
                    proto.span_at(here),
                )
                .with_help(format!(
                    "declare it on the class: `class {class} implements …`"
                ))
            })?;
        cp.vtable
            .get(vslot as usize)
            .copied()
            .filter(|t| *t != u32::MAX)
            // The declaring class's module — an itable lands on a vtable
            // slot, and an inherited one is the ancestor's proto index.
            .zip(
                cp.vowner
                    .get(vslot as usize)
                    .map(|o| table[*o as usize].module),
            )
            .map(|(t, m)| (m, t))
            .ok_or_else(|| RuntimeError::Internal {
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
        // The bytecode case reads the register **in place** and clones only
        // the handle. It used to `Value::clone` the whole register first, and
        // `Value`'s clone sends every heap variant to an `#[inline(never)]`
        // `clone_heap` — so a dynamic call paid an out-of-line function call
        // to bump one refcount, on the one path where the tag is already
        // known to be `VmFunction`. The native arm still clones, and has to:
        // `call_native` takes `&Value` and `&mut self`, so it cannot borrow
        // the register it is passed.
        if let Value::VmFunction(h) = &self.stack[callee_abs] {
            // **Held, not counted**, when nothing can overwrite the callee's
            // register while it runs. It sits just below the callee's frame,
            // which no instruction of the callee can address, and results
            // are written to it only after the frame is gone. The one way in
            // is an open upvalue over that register, so with any open the
            // handle is counted as before. A dynamic call paid a reference
            // count bump and drop on the closure for nothing.
            let (func, owns_func) = if self.open_upvals.is_empty() {
                // SAFETY: as above — `stack[callee_abs]` keeps the closure
                // alive for longer than the frame that holds this copy, and
                // `owns_func: false` means the frame never drops it.
                (unsafe { std::ptr::read(h) }, false)
            } else {
                (Rc::clone(h), true)
            };
            self.frames.last_mut().expect("frame").pc = pc_after as u32;
            self.push_frame_owned(
                func,
                owns_func,
                (callee_abs + 1) as u32,
                n_args,
                callee_abs as u32,
                n_ret,
                site,
            )?;
            return Ok(true);
        }
        let other = self.stack[callee_abs].clone();
        self.call_native(&other, callee_abs, n_args, n_ret, site)?;
        Ok(false)
    }

    /// The `CALLNAT` arm's body, out of line.
    ///
    /// Not a tidiness split: keeping the arm itself down to one call is worth
    /// measurable time on programs that make no native calls at all, because
    /// what it displaces in the dispatch loop costs them. See the arm.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn call_nat(
        &mut self,
        chunk: &Chunk,
        k: u32,
        ins: Instruction,
        base: usize,
        a: usize,
        proto: &Proto,
        here: u32,
    ) -> Result<(), RuntimeError> {
        let n_args = self.arg_count(ins.b(), base + a + 1);
        let n_ret = if ins.c() == 0 {
            ALL_RESULTS
        } else {
            ins.c() - 1
        };
        self.call_native(
            &chunk.constants[k as usize],
            base + a,
            n_args,
            n_ret,
            &Site::Code(proto, here),
        )
    }

    /// The error a native's `Err(message)` becomes.
    ///
    /// Most natives fail because they were handed something they cannot work
    /// with. `error()` is different: the program raised it on purpose, and may
    /// have asked — with a level, as in Lua — for the blame to land on a
    /// caller's line rather than its own. The callers' call sites are only
    /// known here, in their saved `pc`s.
    #[cold]
    #[inline(never)]
    pub(crate) fn native_failure(
        &self,
        message: String,
        function: &str,
        site: &Site<'_>,
    ) -> RuntimeError {
        let Some(level) = saule_runtime::stdlib::core::take_raised_level() else {
            return RuntimeError::native(message, Some(function), site.span());
        };
        let span = site.span();
        let origin = span.start;
        let here = RuntimeError::Raised {
            message: message.clone(),
            span,
            label: "raised here".to_string(),
            origin,
        };
        if level <= 1 {
            return here;
        }
        // Natives push no frame, so the top frame is the function that
        // called `error`: level 1. Level `n` is `n - 1` frames below it, and
        // a level deeper than the stack blames the outermost call there is.
        let depth = self.frames.len();
        let (Some(top), Some(blamed)) = (
            self.frames.last(),
            self.frames.get(
                depth
                    .saturating_sub(level as usize)
                    .min(depth.saturating_sub(1)),
            ),
        ) else {
            return here;
        };
        // The snippet is drawn from the file the top frame is in, so a
        // caller in another module can't be pointed at from here; its line
        // is still in the stack trace below.
        if Rc::ptr_eq(&top.chunk, &blamed.chunk) && !std::ptr::eq(top, blamed) {
            return RuntimeError::Raised {
                message,
                span: blamed.proto.span_at(blamed.pc.saturating_sub(1)),
                label: format!("raised at level {level}: blamed on this call"),
                origin,
            };
        }
        here
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
        // Every native may call a Saule function back — `Table.sort` does —
        // so the call goes through `foreign`, which parks this file's open
        // upvalues for its duration. The results are stored *after* it
        // returns, once the registers they may land in are this file's again.
        match callee {
            Value::Native(nf) => {
                let v = self
                    .foreign(|vm| (nf.func)(&vm.stack[args_from..args_from + n_args]))
                    .map_err(|m| self.native_failure(m, nf.name, site))?;
                // **Moved**, not cloned. `store_results` takes a slice, so
                // the one value a `Native` returns went in by reference and
                // came back out through `cloned()` — a refcount pair per
                // call, and the original dropped an instruction later. This
                // is the single commonest shape in the stdlib.
                self.store_one(dst, v, n_ret);
                Ok(())
            }
            Value::NativeClosure(nc) => {
                let vs = self
                    .foreign(|vm| (nc.func)(&vm.stack[args_from..args_from + n_args]))
                    .map_err(|m| self.native_failure(m, nc.name, site))?;
                self.store_results(dst, &vs, n_ret);
                Ok(())
            }
            // `Image(…)` on a native package's class. A Saule class is built
            // by `NEW`, which needs a layout the compiler can prove; a native
            // class has none, so its construction compiles to a plain call of
            // the class value, which runs the constructor it carries.
            Value::Class(class) => {
                match class.lookup_static_field(saule_runtime::value::foreign::NATIVE_CONSTRUCTOR) {
                    Some(ctor) => self.call_native(&ctor, dst, n_args, n_ret, site),
                    None => Err(RuntimeError::not_callable(callee.type_name(), site.span())),
                }
            }
            // The tree-walker's wording, from its own constructor: `CALL`
            // and `call_value_multi` compile the same source, so a value
            // that is not callable has to say so identically.
            other => Err(RuntimeError::not_callable(other.type_name(), site.span())),
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
        handle: Held<VmFunctionRef>,
        owns_func: bool,
        args_from: usize,
        n_args: usize,
        site: &Site<'_>,
    ) -> Result<(), RuntimeError> {
        let cl = Closure::from_handle(&handle).ok_or_else(|| RuntimeError::Internal {
            message: "internal: frame handle is not a bytecode closure".into(),
            span: site.span(),
        })?;
        // SAFETY: as in `push_frame` — the chunk owns the proto and `shared`
        // owns the chunk, so neither can go away while this frame runs. The
        // frame being replaced held them the same way, so overwriting it
        // leaks no count either.
        let (proto, chunk) = unsafe { (hold(&cl.proto), hold(&cl.chunk)) };
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
            owns_func,
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
        self.push_frame_owned(func, true, base, n_args, ret_to, n_ret, site)
    }

    /// [`push_frame`](Self::push_frame), saying whether the frame owns the
    /// reference count `func` carries. With `owns_func: false` the frame
    /// never drops it, so `func` must be a bitwise copy of a handle that
    /// outlives the frame.
    #[inline]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn push_frame_owned(
        &mut self,
        func: Rc<VmFunctionRef>,
        owns_func: bool,
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
        let (proto, chunk) = {
            let cl = Closure::from_handle(&func).ok_or_else(|| RuntimeError::Internal {
                message: "internal: frame handle is not a bytecode closure".into(),
                span: site.span(),
            })?;
            // SAFETY: the closure's `Chunk` holds this `Proto`, and
            // `shared.chunks` holds that `Chunk`, for longer than any frame
            // over them — see `Frame::proto`. Taken in a block of its own so
            // the borrow of `func` ends before it is moved into the frame.
            unsafe { (hold(&cl.proto), hold(&cl.chunk)) }
        };
        self.push_frame_resolved(
            std::mem::ManuallyDrop::new(func),
            owns_func,
            proto,
            chunk,
            base,
            n_args,
            ret_to,
            n_ret,
            site,
        )
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
        func: Held<VmFunctionRef>,
        owns_func: bool,
        proto: Held<Proto>,
        chunk: Held<Chunk>,
        base: u32,
        n_args: usize,
        ret_to: u32,
        n_ret: u8,
        site: &Site<'_>,
    ) -> Result<(), RuntimeError> {
        if self.push_frame_raw(func, owns_func, proto, chunk, base, n_args, ret_to, n_ret) {
            return Err(self.stack_overflow(site));
        }
        Ok(())
    }

    /// The error a call past `max_frames` raises.
    #[cold]
    #[inline(never)]
    pub(crate) fn stack_overflow(&self, site: &Site<'_>) -> RuntimeError {
        RuntimeError::StackOverflow {
            limit: self.shared.max_frames as u32,
            span: site.span(),
        }
    }

    /// [`push_frame_resolved`](Self::push_frame_resolved), answering `true`
    /// for a stack overflow instead of building the error.
    ///
    /// `RuntimeError` is 64 bytes, so a `Result` return made every call
    /// write and test a 64-byte value through memory to carry an error that
    /// almost never happens. The hot call arms take this and build the error
    /// themselves, out of line.
    #[allow(clippy::too_many_arguments)]
    #[inline(always)]
    pub(crate) fn push_frame_raw(
        &mut self,
        func: Held<VmFunctionRef>,
        owns_func: bool,
        proto: Held<Proto>,
        chunk: Held<Chunk>,
        base: u32,
        n_args: usize,
        ret_to: u32,
        n_ret: u8,
    ) -> bool {
        if self.frames.len() >= self.shared.max_frames {
            return true;
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
            owns_func,
            proto,
            chunk,
            base,
            ret_to,
            n_ret,
            pc,
            top: top as u32,
            n_args: n_args.min(u8::MAX as usize) as u8,
        });
        false
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

        // One value back to a caller that wants one: `RET1` into an
        // expression, which is nearly every return a program makes. The
        // general loop below paid a stack-growth check that cannot fire here
        // (`dst + 1 <= src + 1`, and `src` is a register of the frame just
        // popped), two bounds-checked indexes, and the full drop of the
        // destination's old value even when it was an integer. Both
        // registers are inside the file by the headroom invariant `reg`
        // documents, and `set_reg` skips the drop for a scalar.
        if count == 1 && frame.n_ret == 1 {
            let v = std::mem::replace(self.reg_mut(src), Value::Nil);
            self.set_reg(dst, v);
            return false;
        }

        let wanted = if frame.n_ret == ALL_RESULTS {
            count
        } else {
            frame.n_ret as usize
        };
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

    /// [`store_results`](Self::store_results) for a caller that owns exactly
    /// one value, so it can be moved into the register rather than cloned.
    ///
    /// Behaviourally identical to `store_results(dst, &[v], n_ret)`, down to
    /// what `top` becomes and what happens when the call site wants zero
    /// results (the value is dropped) or more than one (the rest are nil).
    pub(crate) fn store_one(&mut self, dst: usize, v: Value, n_ret: u8) {
        let wanted = if n_ret == ALL_RESULTS {
            1
        } else {
            n_ret as usize
        };
        self.ensure_stack(dst + wanted);
        if wanted > 0 {
            self.stack[dst] = v;
            for i in 1..wanted {
                self.stack[dst + i] = Value::Nil;
            }
        }
        if n_ret == ALL_RESULTS
            && let Some(f) = self.frames.last_mut()
        {
            f.top = (dst + 1) as u32;
        }
    }

    pub(crate) fn store_results(&mut self, dst: usize, vals: &[Value], n_ret: u8) {
        let wanted = if n_ret == ALL_RESULTS {
            vals.len()
        } else {
            n_ret as usize
        };
        self.ensure_stack(dst + wanted);
        for i in 0..wanted {
            self.stack[dst + i] = vals.get(i).cloned().unwrap_or(Value::Nil);
        }
        if n_ret == ALL_RESULTS
            && let Some(f) = self.frames.last_mut()
        {
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
        debug_assert_eq!(
            ins.op(),
            Some(Op::EXTRAARG),
            "verify_proto guarantees an EXTRAARG here"
        );
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
    #[inline(never)]
    pub(crate) fn enter_static(
        &mut self,
        tm: usize,
        target: u32,
        base: u32,
        n_args: usize,
        ret_to: u32,
        n_ret: u8,
    ) -> bool {
        // SAFETY: the cache owns all three for as long as the engine runs
        // and never clears a cell — see `Frame::owns_func` and
        // `Frame::proto`. Held rather than borrowed because
        // `push_frame_raw` wants `&mut self`.
        let t = self.static_target(tm, target);
        let (handle, proto, chunk) = unsafe { (hold(&t.closure), hold(&t.proto), hold(&t.chunk)) };
        self.push_frame_raw(handle, false, proto, chunk, base, n_args, ret_to, n_ret)
    }

    /// `CALLSELF`: enter the running frame's own function again. `true` is
    /// a stack overflow.
    #[inline(never)]
    pub(crate) fn enter_self(&mut self, pc: u32, base: u32, n_args: usize, n_ret: u8) -> bool {
        let f = self.frames.last_mut().expect("frame");
        f.pc = pc;
        // SAFETY: the running frame holds all three and outlives the
        // callee's frame, which ends first; the copies are held without a
        // count and never dropped.
        let (func, proto, chunk) = unsafe {
            (
                std::ptr::read(&f.func),
                std::ptr::read(&f.proto),
                std::ptr::read(&f.chunk),
            )
        };
        self.push_frame_raw(func, false, proto, chunk, base, n_args, base, n_ret)
    }

    /// `(module, proto)`'s [`StaticTarget`], built on first use.
    #[inline]
    pub(crate) fn static_target(&self, tm: usize, idx: u32) -> &super::StaticTarget {
        self.shared.closure_cache[tm][idx as usize].get_or_init(|| {
            let chunk = &self.shared.chunks[tm];
            super::StaticTarget {
                closure: VmFunctionRef::new(Closure::bound(
                    Rc::clone(chunk.proto(idx)),
                    Rc::clone(chunk),
                    Vec::new(),
                    &self.shared,
                )),
                proto: Rc::clone(chunk.proto(idx)),
                chunk: Rc::clone(chunk),
            }
        })
    }

    /// A cached, upvalue-free closure for a statically-resolved callee, so
    /// `CALLK` allocates nothing per call.
    ///
    /// Lent, not cloned: the cache owns it for the life of the engine, so a
    /// static call's frame holds it without a count (`Frame::owns_func`), and
    /// a caller that needs to keep one clones it itself.
    pub(crate) fn closure_for(&self, chunk: &Rc<Chunk>, idx: u32) -> &Rc<VmFunctionRef> {
        &self.static_target(chunk.module_index, idx).closure
    }
}
