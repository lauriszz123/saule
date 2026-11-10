//! Coroutines, held opaquely.
//!
//! A coroutine is a function that can stop part-way — `yield` — and later be
//! resumed from where it stopped, with its locals and its whole call stack
//! intact. Running one means running bytecode, which only `saule-vm` can do,
//! so this crate defines the trait and the VM implements it: the same
//! arrangement as [`VmFunction`](super::VmFunction), and for the same reason
//! (the dependency runs from the VM to the runtime, never back).
//!
//! Two kinds of object stand behind [`Value::Coroutine`](super::Value):
//!
//! * the **coroutine** itself — what `Coroutine.create` returns, and what the
//!   program resumes, asks the status of, and reads the error from;
//! * its **yielder** — the handle the coroutine's body is called with, and
//!   the only thing that can `yield` it.
//!
//! They are separate objects so that the body does not hold its own
//! coroutine. A body that did would keep a strong reference to the
//! coroutine in its own first register for as long as it was suspended, and
//! no abandoned coroutine would ever be freed. The yielder refers to its
//! coroutine weakly.
//!
//! Every method of both is dispatched through [`VmCoroutine::call_method`],
//! so what a coroutine answers to is decided in one place, in the VM.

use std::fmt;
use std::rc::Rc;

use super::Value;
use crate::error::RuntimeError;

/// A coroutine or a yielder, implemented by the VM.
pub trait VmCoroutine: fmt::Debug {
    /// Downcast hook — the VM recovers its own types through this.
    fn as_any(&self) -> &dyn std::any::Any;

    /// The name the program knows this object's type by: `Coroutine` or
    /// `Yielder`. It is what an `as` test compares against, and, lowercased,
    /// what [`Value::type_name`](super::Value::type_name) reports.
    fn class_name(&self) -> &'static str;

    /// `value.type_name()` for this object.
    fn type_name(&self) -> &'static str;

    /// Run the method `name` on this object.
    ///
    /// Reached from the runtime's dynamic method dispatch, which is to say
    /// from a method call the VM did not handle itself. The one call the VM
    /// *does* handle itself is a yielder's `yield` made by the coroutine's
    /// own code — that one has to leave the dispatch loop, which nothing
    /// outside the loop can make it do. A `yield` that arrives here was made
    /// from somewhere else, and is reported as the error it is.
    fn call_method(
        &self,
        handle: &Rc<CoroutineRef>,
        name: &str,
        args: &[Value],
        span: std::ops::Range<usize>,
    ) -> Result<Vec<Value>, RuntimeError>;
}

/// A [`VmCoroutine`] behind a **thin** pointer, for the reason
/// [`VmFunctionRef`](super::VmFunctionRef) is one: a fat `Rc<dyn Trait>` in
/// [`Value`] would widen every value in the language from 16 bytes to 24.
pub struct CoroutineRef(Box<dyn VmCoroutine>);

impl CoroutineRef {
    pub fn new(c: impl VmCoroutine + 'static) -> Rc<CoroutineRef> {
        Rc::new(CoroutineRef(Box::new(c)))
    }

    /// Wrap an implementation without allocating the `Rc` — for
    /// `Rc::new_cyclic`, where the caller allocates it.
    pub fn wrap(c: impl VmCoroutine + 'static) -> CoroutineRef {
        CoroutineRef(Box::new(c))
    }

    /// Run a method on the object behind `handle`. See
    /// [`VmCoroutine::call_method`].
    pub fn call_method(
        self: &Rc<Self>,
        name: &str,
        args: &[Value],
        span: std::ops::Range<usize>,
    ) -> Result<Vec<Value>, RuntimeError> {
        self.0.call_method(self, name, args, span)
    }
}

impl std::ops::Deref for CoroutineRef {
    type Target = dyn VmCoroutine + 'static;

    fn deref(&self) -> &(dyn VmCoroutine + 'static) {
        &*self.0
    }
}

impl fmt::Debug for CoroutineRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}
