//! Call frames and the runtime closure (`VM_DESIGN.md` §5.1, §6.1).

use std::cell::RefCell;
use std::rc::Rc;

use saule_interpreter::value::{VmFunction, VmFunctionRef};

use crate::chunk::Proto;

use super::upval::Upvalue;

/// A bytecode function plus the upvalues it captured.
///
/// Implements [`VmFunction`] so it can sit in a register as an ordinary
/// [`Value`](saule_interpreter::Value) without `saule-interpreter` having to
/// know this type exists — which is what keeps the dependency arrow pointing
/// one way (§22.1).
#[derive(Debug)]
pub struct Closure {
    pub proto: Rc<Proto>,
    pub upvals: Vec<Rc<RefCell<Upvalue>>>,
}

impl Closure {
    pub fn new(proto: Rc<Proto>) -> Closure {
        Closure { proto, upvals: Vec::new() }
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
}

/// One activation record. `Environment` disappears entirely: a call is a
/// `Vec` push plus a bounds check that the register file is long enough.
#[derive(Debug)]
pub struct Frame {
    /// The running closure, held erased. The concrete [`Closure`] is
    /// recovered with [`Closure::from_handle`]; the downcast is a vtable
    /// pointer compare and happens once per frame entry, not per
    /// instruction.
    pub func: Rc<VmFunctionRef>,
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
}

/// `n_ret` sentinel: the caller wants every result the callee produces.
pub const ALL_RESULTS: u8 = 255;
