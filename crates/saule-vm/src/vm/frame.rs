//! Call frames and the runtime closure (`VM_DESIGN.md` §5.1, §6.1).

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use saule_interpreter::value::{VmFunction, VmFunctionRef};
use saule_interpreter::{RuntimeError, Value};

use crate::chunk::Proto;

use super::upval::Upvalue;
use super::VmShared;

/// A bytecode function plus the upvalues it captured.
///
/// Implements [`VmFunction`] so it can sit in a register as an ordinary
/// [`Value`](saule_interpreter::Value) without `saule-interpreter` having to
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
        Closure { proto, chunk, upvals: Vec::new(), shared: Weak::new() }
    }

    /// A closure that can run itself, over `shared`.
    pub fn bound(
        proto: Rc<Proto>,
        chunk: Rc<crate::chunk::Chunk>,
        upvals: Vec<Rc<RefCell<Upvalue>>>,
        shared: &Rc<VmShared>,
    ) -> Closure {
        Closure { proto, chunk, upvals, shared: Rc::downgrade(shared) }
    }

    /// Recover a `&Closure` from the erased handle a register holds.
    ///
    /// Every `Value::VmFunction` this crate creates wraps a `Closure`, so a
    /// failure here means a chunk was handed a foreign `VmFunction` impl.
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
            return Err(RuntimeError::TypeError {
                message: format!(
                    "cannot call `{}`: the engine that compiled it is no longer running",
                    self.proto.label()
                ),
                span,
            });
        };
        // Recycled where possible: this is per *comparison* on a sort, and
        // a fresh `Vm` is two heap allocations.
        let mut vm = shared.take_vm();
        let out = vm.invoke(handle, args, span);
        if out.is_ok() {
            shared.give_vm(vm);
        }
        out
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
            return Err(RuntimeError::TypeError {
                message: format!(
                    "cannot call `{}`: the engine that compiled it is no longer running",
                    self.proto.label()
                ),
                span,
            });
        };
        let mut vm = shared.take_vm();
        let out = vm.invoke_first(handle, args, span);
        if out.is_ok() {
            shared.give_vm(vm);
        }
        out
    }
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
    pub func: Rc<VmFunctionRef>,
    /// The running proto — `func`'s, resolved once when the frame is built.
    pub proto: Rc<Proto>,
    /// The chunk `proto` was compiled in. Constants, protos, jump tables and
    /// cast types are per chunk, so they follow the frame rather than the VM.
    pub chunk: Rc<crate::chunk::Chunk>,
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

/// `n_ret` sentinel: the caller wants every result the callee produces.
pub const ALL_RESULTS: u8 = 255;
