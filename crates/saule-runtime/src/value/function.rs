//! Function values: native functions, native closures, and user-defined
//! functions compiled to bytecode.

use std::fmt;

use super::Value;

/// A function that has been compiled to bytecode, held opaquely.
///
/// `saule-vm` depends on `saule-runtime` and never the reverse, so [`Value`]
/// cannot name the VM's `Closure` type directly; it names this trait, which
/// the VM implements.
///
/// The runtime carries such a value around, reports it as a `function`, and
/// — through [`Self::call`] — **invokes** it. That last part is what makes a
/// native like `Table.sort` able to run a comparator the VM compiled, and
/// what lets a class answer `toString`. It does not weaken the dependency
/// rule: this crate names only the trait, and the VM implements it.
pub trait VmFunction: fmt::Debug {
    /// `Some(name)` for a named declaration, `None` for a lambda.
    fn vm_name(&self) -> Option<&str>;
    /// Downcast hook — the VM recovers its own closure type through this.
    fn as_any(&self) -> &dyn std::any::Any;

    /// Run this function over `args` and return everything it returned.
    ///
    /// `handle` is the very `Rc` this trait object lives inside. Passing it
    /// back in looks redundant, but the VM's frames hold an
    /// `Rc<VmFunctionRef>` and `&self` cannot be turned back into one — so
    /// without it every callback would have to rebuild an equivalent
    /// closure, which on a sort comparator is an allocation per comparison.
    /// Go through [`VmFunctionRef::invoke`] rather than calling this
    /// directly; it is what guarantees the two agree.
    fn call(
        &self,
        handle: &std::rc::Rc<VmFunctionRef>,
        args: &[Value],
        span: std::ops::Range<usize>,
    ) -> Result<Vec<Value>, crate::error::RuntimeError>;

    /// [`call`](Self::call) for a caller that wants one value.
    ///
    /// Almost every callback is this shape — a sort comparator, an operator
    /// overload, a `toString` — and going through `call` means a `Vec` is
    /// allocated and freed to carry a single `Value`. On `Table.sort` that
    /// is a malloc and a free *per comparison*, which measured as a quarter
    /// of the comparator's cost.
    ///
    /// A new, not-yet-started coroutine whose body is this function —
    /// `Coroutine.create(f)`.
    ///
    /// `None` from an implementor that cannot suspend a running function,
    /// which is the default. The VM can: a coroutine is a register file of
    /// its own that keeps its frames between resumes.
    fn coroutine(
        &self,
        _handle: &std::rc::Rc<VmFunctionRef>,
    ) -> Option<std::rc::Rc<super::CoroutineRef>> {
        None
    }

    /// The default implementation is the honest one for any implementor
    /// that has no cheaper route; the VM overrides it with a path that
    /// returns out of the register file directly.
    fn call_first(
        &self,
        handle: &std::rc::Rc<VmFunctionRef>,
        args: &[Value],
        span: std::ops::Range<usize>,
    ) -> Result<Value, crate::error::RuntimeError> {
        Ok(self
            .call(handle, args, span)?
            .into_iter()
            .next()
            .unwrap_or(Value::Nil))
    }

    /// Open a [`CallSession`] for a caller about to make *many* calls to
    /// this one function.
    ///
    /// `None` by default, and from any implementor whose per-call cost is
    /// already the whole cost. The VM's is not: a callback runs on a
    /// register file of its own — the dispatch loop holds `&mut Vm` across
    /// the native that is calling back, so the callee cannot have the
    /// caller's — and setting one up is per *call* what it could be per
    /// *sort*. `Table.sort` on 200k elements asks its comparator about 3.3
    /// million times.
    fn call_session(&self, _handle: &std::rc::Rc<VmFunctionRef>) -> Option<Box<dyn CallSession>> {
        None
    }
}

/// Many calls to one function, with whatever a call needs set up once
/// instead of per call.
///
/// Opened through [`crate::call::call_value_session`], which answers `None`
/// for a callee with nothing to amortise — the caller then falls back to
/// [`crate::call::call_value_first`] per call and both paths stay honest.
///
/// A session that fails stays failed: the engine state it was holding is
/// abandoned with the error, because a callback that did not return cleanly
/// may have left a frame or an open upvalue behind. Callers stop at the
/// first error anyway, which is what makes that the simple rule rather than
/// a limitation.
pub trait CallSession {
    /// Call the function this session was opened for, taking its first
    /// result — [`VmFunction::call_first`]'s shape, and for the same reason.
    fn call_first(
        &mut self,
        args: &[Value],
        span: std::ops::Range<usize>,
    ) -> Result<Value, crate::error::RuntimeError>;

    /// What the function is, when it is nothing but one comparison of its
    /// two arguments: `(a, b) => a < b` and its like. `None` for any other.
    ///
    /// A native that would call such a function a million times to learn
    /// what it can work out itself (`Table.sort`) asks this first.
    fn compares(&self) -> Option<Comparison> {
        None
    }
}

/// A function of two arguments that only compares them, as
/// [`CallSession::compares`] tells it: which of the two it wants first,
/// and of what. It answers for those and nothing else: given any other
/// values it fails, as a call of it would.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Comparison {
    /// `a < b`, integers.
    IntLess,
    /// `a > b`.
    IntGreater,
    /// `a <= b`.
    IntAtMost,
    /// `a >= b`.
    IntAtLeast,
    /// `a < b`, floats.
    FloatLess,
    /// `a > b`.
    FloatGreater,
}

/// A [`VmFunction`] held behind a **thin** pointer.
///
/// This box exists for one reason, and it is a load-bearing one. `Rc<dyn
/// Trait>` is a *fat* pointer: two words, data plus vtable. Putting one
/// directly in [`Value`] pushes the whole enum from 16 bytes to 24 — a 50%
/// inflation of every local, every table slot, every argument and every
/// register in the language, to carry a vtable pointer that is identical for
/// every value of the variant.
///
/// `VM_DESIGN.md` §4.1 measures `Value` at 16 bytes and argues *from that
/// number* that NaN-boxing is not worth doing. Silently invalidating the
/// premise while adding a variant would have been a poor trade.
///
/// Boxing moves the vtable pointer into the heap allocation, where it is paid
/// for once per closure instead of once per copy. The extra indirection costs
/// nothing measurable: the VM calls a closure through its own frame
/// machinery, never through this pointer.
pub struct VmFunctionRef(Box<dyn VmFunction>);

impl VmFunctionRef {
    pub fn new(f: impl VmFunction + 'static) -> std::rc::Rc<VmFunctionRef> {
        std::rc::Rc::new(VmFunctionRef(Box::new(f)))
    }

    /// Call the bytecode function this handle wraps.
    ///
    /// The entry point for everything outside the VM's own dispatch loop.
    /// Taking `self: &Rc<Self>` is what lets the implementation hand its own
    /// handle to a VM frame without rebuilding a closure — see
    /// [`VmFunction::call`].
    pub fn invoke(
        self: &std::rc::Rc<Self>,
        args: &[Value],
        span: std::ops::Range<usize>,
    ) -> Result<Vec<Value>, crate::error::RuntimeError> {
        self.0.call(self, args, span)
    }

    /// [`invoke`](Self::invoke) for the single-result callers.
    pub fn invoke_first(
        self: &std::rc::Rc<Self>,
        args: &[Value],
        span: std::ops::Range<usize>,
    ) -> Result<Value, crate::error::RuntimeError> {
        self.0.call_first(self, args, span)
    }
}

impl std::ops::Deref for VmFunctionRef {
    type Target = dyn VmFunction + 'static;

    fn deref(&self) -> &(dyn VmFunction + 'static) {
        &*self.0
    }
}

impl fmt::Debug for VmFunctionRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

/// Rust-implemented function exposed to Saule.
///
/// The function returns `Result<Value, String>` where the error string is
/// surfaced as a `RuntimeError::TypeError` at the call site.
pub struct NativeFn {
    pub name: &'static str,
    pub func: fn(&[Value]) -> Result<Value, String>,
    /// A shorter way in, for a native that only works something out from
    /// its arguments: `func`'s own answer where it has one, and `None`
    /// where `func` would fail, which sends the call to `func` for the
    /// failure. It is `func` itself with its error let go of (see
    /// [`NativeFn::pure`]), so the two cannot disagree.
    ///
    /// Only for a native that calls nothing back and changes nothing: the
    /// caller skips, for such a call, what it sets up in case a native
    /// runs Saule code, and a refused call runs twice.
    pub quick: Option<QuickFn>,
}

/// A native's shorter way in: see [`NativeFn::quick`].
pub type QuickFn = fn(&[Value]) -> Option<Value>;

impl NativeFn {
    /// A native with nothing but `func`.
    pub fn new(name: &'static str, func: fn(&[Value]) -> Result<Value, String>) -> Self {
        Self {
            name,
            func,
            quick: None,
        }
    }

    /// A native that only works something out from its arguments, with
    /// its shorter way in (see `quick`).
    pub fn pure(
        name: &'static str,
        func: fn(&[Value]) -> Result<Value, String>,
        quick: QuickFn,
    ) -> Self {
        Self {
            name,
            func,
            quick: Some(quick),
        }
    }
}

impl fmt::Debug for NativeFn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<native fn {}>", self.name)
    }
}

/// The body of a [`NativeClosure`]: positional arguments in, multiple
/// return values or an error message out.
pub type NativeClosureFn = Box<dyn Fn(&[Value]) -> Result<Vec<Value>, String>>;

/// Stateful Rust-implemented function. The closure may capture arbitrary
/// Rust state (e.g. an iterator's cursor) and may return multiple values.
pub struct NativeClosure {
    pub name: &'static str,
    pub func: NativeClosureFn,
    /// Declared parameter names, in order — descriptive only: a native is
    /// called with positional arguments. Empty for most stdlib closures.
    pub param_names: Vec<String>,
}

impl fmt::Debug for NativeClosure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<native closure {}>", self.name)
    }
}
