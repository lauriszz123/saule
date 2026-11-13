//! Call frames and the runtime closure (`VM_DESIGN.md` §5.1, §6.1).

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use saule_runtime::value::{CallSession, VmFunction, VmFunctionRef};
use saule_runtime::{RuntimeError, Value};

use crate::chunk::Proto;

use super::upval::Upvalue;
use super::{Vm, VmShared};

/// A bytecode function plus the upvalues it captured.
///
/// Implements [`VmFunction`] so it can sit in a register as an ordinary
/// [`Value`](saule_runtime::Value) without `saule-interpreter` having to
/// know this type exists — which is what keeps the dependency arrow pointing
/// one way (§22.1).
#[derive(Debug)]
pub struct Closure {
    pub proto: Rc<Proto>,
    /// The module this function was compiled in.
    ///
    /// Constants, protos, jump tables and cast types are all indexed **per
    /// chunk**, so they have to follow the frame rather than the VM: a
    /// closure built in one module and called from another must still read
    /// its own module's constant pool. The class, enum and interface tables
    /// are the exception — every chunk of a program shares those `Rc`s, so
    /// reading them through whichever chunk is running gives the same
    /// answer by construction.
    pub chunk: Rc<crate::chunk::Chunk>,
    pub upvals: Vec<Rc<RefCell<Upvalue>>>,
    /// The engine state this closure runs against — the chunk, module slots,
    /// statics, classes and enums (§5.1). Needed because a closure that
    /// escapes into the tree-walker (a sort comparator, a `toString`) must
    /// be able to run *itself*, and a proto plus upvalues is not enough to
    /// execute anything.
    ///
    /// **Weak on purpose.** A closure very commonly lives in a module slot,
    /// and a module slot lives in [`VmShared`] — a strong pointer here would
    /// close that cycle and leak the whole program's state, which is the
    /// same shape as the capture leak `closure_semantics.rs` guards against.
    /// The running [`Vm`] holds the strong reference, and every closure is
    /// reachable only while some `Vm` is on the stack, so the upgrade
    /// succeeds exactly when calling makes sense.
    pub shared: Weak<VmShared>,
}

impl Closure {
    /// An upvalue-free closure, not bound to any engine state.
    ///
    /// For hand-assembled chunks and tests. A closure built this way cannot
    /// be *called back into* from the tree-walker — use
    /// [`Closure::bound`] for anything a running VM creates.
    pub fn new(proto: Rc<Proto>, chunk: Rc<crate::chunk::Chunk>) -> Closure {
        Closure {
            proto,
            chunk,
            upvals: Vec::new(),
            shared: Weak::new(),
        }
    }

    /// A closure that can run itself, over `shared`.
    pub fn bound(
        proto: Rc<Proto>,
        chunk: Rc<crate::chunk::Chunk>,
        upvals: Vec<Rc<RefCell<Upvalue>>>,
        shared: &Rc<VmShared>,
    ) -> Closure {
        Closure {
            proto,
            chunk,
            upvals,
            shared: Rc::downgrade(shared),
        }
    }

    /// Recover a `&Closure` from the erased handle a register holds.
    ///
    /// Every `Value::VmFunction` this crate creates wraps a `Closure`, so a
    /// failure here means a chunk was handed a foreign `VmFunction` impl.
    ///
    /// **This looks more expensive than it is; it was measured, leave it
    /// alone.** Two indirect calls — `as_any` through `VmFunction`'s vtable,
    /// then `type_id` through `Any`'s — plus a 128-bit compare, on every
    /// dynamic `CALL`, every `TAILCALL` and every upvalue read. Replacing the
    /// whole thing with an unchecked pointer cast (as a throwaway, to price
    /// it) is worth **2% of `closure` and nothing measurable anywhere else**,
    /// because both calls are perfectly predicted: the target is the same
    /// every time.
    ///
    /// Buying that 2% soundly costs more than it is worth. `Closure` is the
    /// only implementor of `VmFunction` in the workspace, but the trait is
    /// public, so skipping the check means either an `unsafe` contract on it
    /// — one that cannot even be *stated* in `saule-interpreter`, which
    /// cannot name `Closure` — or a per-call-site cache holding closures
    /// alive to keep a pointer compare honest. Neither is a 2% shape.
    ///
    /// What *was* worth taking sat next door, in `dispatch_call`: see the
    /// comment there.
    pub fn from_handle(handle: &Rc<VmFunctionRef>) -> Option<&Closure> {
        handle.as_any().downcast_ref::<Closure>()
    }
}

impl VmFunction for Closure {
    fn vm_name(&self) -> Option<&str> {
        self.proto.name.as_deref()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    /// Run this closure from outside the VM.
    ///
    /// The re-entrancy path: a native invoking its callable argument, an
    /// operator overload, a `toString`. A **fresh** [`Vm`] runs over the
    /// same [`VmShared`], so the callee sees the same module slots, statics
    /// and classes as its caller while getting a register file of its own —
    /// which is what dissolves the `&mut self` the dispatch loop holds
    /// across a `CALLNAT`.
    fn call(
        &self,
        handle: &Rc<VmFunctionRef>,
        args: &[Value],
        span: std::ops::Range<usize>,
    ) -> Result<Vec<Value>, RuntimeError> {
        let Some(shared) = self.shared.upgrade() else {
            return Err(engine_gone(&self.proto, span));
        };
        // Recycled where possible: this is per *comparison* on a sort, and
        // a fresh `Vm` is two heap allocations.
        let mut vm = shared.take_vm();
        let out = vm.invoke(handle, args, span);
        if out.is_ok() {
            shared.give_vm(vm);
        } else {
            vm.abandon();
        }
        out
    }

    /// `Coroutine.create(f)`: a coroutine whose body is this closure.
    fn coroutine(
        &self,
        handle: &Rc<VmFunctionRef>,
    ) -> Option<Rc<saule_runtime::value::CoroutineRef>> {
        Some(super::coroutine::Coroutine::create(
            Rc::clone(handle),
            self.shared.clone(),
        ))
    }

    /// The single-result form, and the one the callbacks actually use.
    ///
    /// Identical to [`call`](Self::call) except that the result never
    /// becomes a `Vec`: it comes straight out of the VM's own results
    /// buffer, which the re-entry pool then hands to the next call. On
    /// `Table.sort` that removes a malloc and a free per comparison.
    fn call_first(
        &self,
        handle: &Rc<VmFunctionRef>,
        args: &[Value],
        span: std::ops::Range<usize>,
    ) -> Result<Value, RuntimeError> {
        let Some(shared) = self.shared.upgrade() else {
            return Err(engine_gone(&self.proto, span));
        };
        let mut vm = shared.take_vm();
        let out = vm.invoke_first(handle, args, span);
        if out.is_ok() {
            shared.give_vm(vm);
        } else {
            // Dropped rather than pooled, but a closure that captured one of
            // its registers may have escaped — close those first.
            vm.abandon();
        }
        out
    }

    /// Hold the register file open across a run of calls.
    ///
    /// [`call_first`](Self::call_first) above does three things per call
    /// that do not depend on the call: it upgrades the `Weak<VmShared>`,
    /// takes a `Vm` out of the re-entry pool, and hands it back — and
    /// handing it back resets every register the callback touched. Per
    /// `Table.sort` that is nothing; per *comparison*, on a sort that asks
    /// 3.3 million times, it is most of what a two-instruction comparator
    /// costs.
    fn call_session(&self, handle: &Rc<VmFunctionRef>) -> Option<Box<dyn CallSession>> {
        let shared = self.shared.upgrade()?;
        // One guard for the session, because its calls are sequential: it is
        // the same bound on the same resource, taken once.
        let depth = saule_runtime::enter_call_depth(&(0..0)).ok()?;
        let vm = shared.take_vm();
        Some(Box::new(Calls {
            shared,
            handle: Rc::clone(handle),
            proto: Rc::clone(&self.proto),
            chunk: Rc::clone(&self.chunk),
            vm: Some(vm),
            _depth: depth,
        }))
    }
}

/// One re-entry `Vm`, held for as long as a native keeps calling back.
///
/// The register file is reset when the session ends rather than after each
/// call. In between, the last call's registers still refer to its values —
/// which is why the reset exists at all — but they are released when the
/// native that opened the session returns, instead of outliving the whole
/// program the way a pooled-but-unreset `Vm` would.
struct Calls {
    shared: Rc<VmShared>,
    handle: Rc<VmFunctionRef>,
    /// The callee's proto and chunk, recovered from the handle once instead
    /// of by a downcast per call.
    proto: Rc<Proto>,
    chunk: Rc<crate::chunk::Chunk>,
    /// `None` once a call has failed: the engine state went with the error.
    vm: Option<Box<Vm>>,
    /// Held, not read: the session occupies one level of call depth for as
    /// long as it is open.
    _depth: saule_runtime::DepthGuard,
}

impl CallSession for Calls {
    fn call_first(
        &mut self,
        args: &[Value],
        span: std::ops::Range<usize>,
    ) -> Result<Value, RuntimeError> {
        let Some(vm) = self.vm.as_mut() else {
            return Err(RuntimeError::Internal {
                message: "internal: a call session is used after one of its calls failed".into(),
                span,
            });
        };
        match vm.invoke_first_resolved(&self.handle, &self.proto, &self.chunk, args, span) {
            Ok(v) => Ok(v),
            Err(e) => {
                // As in `call_first`: a closure that captured one of the
                // callee's registers may have escaped, so close before the
                // file is dropped. Not pooled — an unwound `Vm` still holds
                // the frames the error left.
                let mut vm = self.vm.take().expect("checked just above");
                vm.abandon();
                Err(e)
            }
        }
    }
}

impl Drop for Calls {
    fn drop(&mut self) {
        if let Some(vm) = self.vm.take() {
            self.shared.give_vm(vm);
        }
    }
}

/// A pointer a [`Frame`] carries to something it does not own.
///
/// `ManuallyDrop<Rc<T>>` rather than `&T` or `*const T`, which is the same
/// choice the dispatch loop makes one layer down and for the same reason —
/// see that module's header. It is the same pointer in a register and the
/// same type to codegen as an `Rc<T>`, so it keeps whatever the owned form
/// bought LLVM, while costing nothing to create and nothing to drop.
pub type Held<T> = std::mem::ManuallyDrop<Rc<T>>;

/// A function value kept past the end of the program that compiled it — an
/// embedder holding on to one, say — and then called.
#[cold]
#[inline(never)]
fn engine_gone(proto: &Proto, span: std::ops::Range<usize>) -> RuntimeError {
    RuntimeError::fault(
        format!(
            "cannot call `{}`: the program that defined it has finished running",
            proto.label()
        ),
        "called here",
        span,
    )
    .with_help("a Saule function can only be called while the program that defined it is running")
}

/// Copy the pointer out of an `Rc` without touching its reference count.
///
/// # Safety
///
/// The pointee must outlive the returned [`Held`]. Nothing about the value
/// enforces that: it neither holds a count nor carries a lifetime, and
/// dereferencing one whose pointee has been freed is use-after-free. Every
/// caller here satisfies it the same way — see [`Frame::proto`].
pub(crate) unsafe fn hold<T>(rc: &Rc<T>) -> Held<T> {
    // SAFETY: the bits are copied, not the ownership. The original `Rc` keeps
    // its count and the copy is never dropped, so no count is touched and
    // none is lost.
    std::mem::ManuallyDrop::new(unsafe { std::ptr::read(rc) })
}

/// One activation record. `Environment` disappears entirely: a call is a
/// `Vec` push plus a bounds check that the register file is long enough.
#[derive(Debug)]
pub struct Frame {
    /// The running closure, held erased. The concrete [`Closure`] is
    /// recovered with [`Closure::from_handle`].
    ///
    /// "Once per frame entry, not per instruction" undersold what that
    /// costs: a frame is entered on every call *and* every return, so
    /// `fib(28)` performed the downcast about two million times. The two
    /// things the dispatch loop actually wants from the closure are cached
    /// beside it, and `func` is now read only by the three opcodes that
    /// touch upvalues.
    ///
    /// The one of the three a frame may have to own. `proto` and `chunk`
    /// below are kept alive by the engine whatever the frame does; a closure
    /// taken from a register is not. Its last strong reference can be the
    /// very register or upvalue cell that its own body overwrites — `f = nil`
    /// inside a lambda that captured itself is enough — and the frame is then
    /// all that stands between `GETUPVAL` and a freed upvalue array. So such
    /// a frame owns it, and a frame whose closure something else is already
    /// keeping alive only holds it — see [`owns_func`](Self::owns_func).
    pub func: Held<VmFunctionRef>,
    /// Whether this frame holds a strong count on [`func`](Self::func), and
    /// so must release it when the frame goes.
    ///
    /// `true` for a closure that came from a register: the reason above for
    /// owning one is real, and it is the only one that can apply. `false`
    /// for a statically-resolved callee — `CALLK`, `CALLSTAT` and their tail
    /// forms — whose closure lives in `VmShared::closure_cache`: a cell
    /// written once and never cleared, in the same engine state that owns
    /// every `Proto` and `Chunk` a frame already holds without counting. A
    /// static closure is never a register's to drop, so it cannot be freed
    /// under the frame. Counting it cost an increment on every call and a
    /// decrement on every return, on the same cell for a recursive
    /// function: 11% of `fib`. Also `false` for a call made through a
    /// [`CallSession`], which owns the handle for longer than any call it
    /// makes.
    ///
    /// Next to the other byte-wide fields, in what was padding: the frame is
    /// no larger for it.
    pub owns_func: bool,
    /// The running proto — `func`'s, resolved once when the frame is built.
    ///
    /// Carried as a [`Held`] rather than an `Rc`: entering and leaving a
    /// frame happens on every call *and* every return, so an owned `Proto`
    /// and `Chunk` cost two refcount pairs per round trip for a guarantee
    /// that was already in hand. Every `Rc<Proto>` the VM runs comes from
    /// `chunk.proto(idx)`, so its `Chunk` holds it for the chunk's whole
    /// life; every `Rc<Chunk>` comes from `VmShared::chunks`, which outlives
    /// every frame opened over it. The dispatch loop already reads both out
    /// of the frame this way and documents the invariant at length; this
    /// stops the *writes* paying for it too.
    pub proto: Held<Proto>,
    /// The chunk `proto` was compiled in. Constants, protos, jump tables and
    /// cast types are per chunk, so they follow the frame rather than the VM.
    /// Held, not owned, for the reason given on `proto`.
    pub chunk: Held<crate::chunk::Chunk>,
    /// `R[0]` is `stack[base]`.
    pub base: u32,
    /// Absolute register in the *caller* where this call's results go.
    pub ret_to: u32,
    /// How many results the caller wants. [`ALL_RESULTS`] means "all of
    /// them, and set `top`".
    pub n_ret: u8,
    /// Saved program counter, valid while this frame is not the active one.
    pub pc: u32,
    /// Stack top — meaningful only at variadic call/return points, where a
    /// `B`/`C` operand of 0 means "however many there turned out to be".
    pub top: u32,
    /// How many arguments this call actually supplied.
    ///
    /// Only `VARARG` reads it, but it has to be recorded on every frame:
    /// the callee is what gathers its surplus arguments, and it cannot ask
    /// the caller after the fact.
    pub n_args: u8,
}

impl Drop for Frame {
    /// Release the closure handle if this frame was given one to own.
    ///
    /// Every way a frame ends comes through here — a return's `pop`, a tail
    /// call writing a new frame over it, an unwind, an abandoned register
    /// file, a finished coroutine's frame list — so the rule is kept in one
    /// place rather than at each of them.
    fn drop(&mut self) {
        if self.owns_func {
            // SAFETY: an owned handle was moved in when the frame was built,
            // and this is the only place it is released — `Drop` runs once.
            unsafe { std::mem::ManuallyDrop::drop(&mut self.func) }
        }
    }
}

/// `n_ret` sentinel: the caller wants every result the callee produces.
pub const ALL_RESULTS: u8 = 255;
