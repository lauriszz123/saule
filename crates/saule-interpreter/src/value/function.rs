//! Function values: native functions, native closures, and user-defined
//! [`FunctionObject`]s.

use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;
use std::sync::Arc;

use saule_ast::{Expr, Param, Spanned, Stmt};

use crate::env::Environment;

use super::Value;
use super::class::ClassObject;

/// Rust-implemented function exposed to Saule.
///
/// The function returns `Result<Value, String>` where the error string is
/// surfaced as a `RuntimeError::TypeError` at the call site.
pub struct NativeFn {
    pub name: &'static str,
    pub func: fn(&[Value]) -> Result<Value, String>,
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
    /// Declared parameter names, in order. When non-empty, the call site
    /// accepts named arguments and reorders them into positional slots
    /// before invoking `func`. Empty for closures that only take positional
    /// arguments (the default for stdlib closures).
    pub param_names: Vec<String>,
}

impl fmt::Debug for NativeClosure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<native closure {}>", self.name)
    }
}

/// A user-defined function carrying its body and lexical closure.
#[derive(Debug)]
pub struct FunctionObject {
    /// `Some(name)` for named declarations, `None` for lambdas.
    pub name: Option<String>,
    pub params: Vec<Param>,
    /// `params`' names pre-interned, one per entry and in the same order.
    ///
    /// Binding an argument used to clone the parameter's name into a fresh
    /// `String` — one heap allocation per parameter on *every* call, which
    /// measured at roughly a third of the cost of passing an argument.
    /// Interning once at definition turns that into a refcount bump.
    pub param_keys: Vec<Rc<str>>,
    pub body: FunctionBody,
    /// The environment that was in scope when the function was created.
    /// Captured by reference so inner functions see live bindings.
    pub closure: Rc<RefCell<Environment>>,
    /// `Some(class)` when this function is a method (static or instance) of
    /// a class. Set after class construction via [`set_owner_class`]. The
    /// call sites consult this to re-inject the owning class's statics into
    /// the call scope, so a static method can reach sibling statics by their
    /// bare names even when invoked as a plain `Value::Function`.
    pub owner_class: RefCell<Option<std::rc::Weak<ClassObject>>>,
    /// `Some` when this function was defined inside an imported module —
    /// carries that module's `NamedSource` so a runtime error fired while
    /// executing the body can be rendered with the correct source snippet.
    /// `None` for functions defined in the entry file (the CLI attaches
    /// the entry file's source to top-level errors already).
    pub source: Option<Rc<miette::NamedSource<String>>>,
    /// `Some(name)` when this is a self-recursive local closure — the shape
    /// `local fact = fn(n) … fact(n-1) … end`. Set after construction via
    /// [`set_self_name`](Self::set_self_name).
    ///
    /// The recursive call resolves through a binding the *call scope* makes to
    /// this very function, not through a captured variable. Capturing it would
    /// close a cycle — the binding's cell would hold the function and the
    /// function's captured scope would hold the cell — which is precisely the
    /// leak flat capture exists to prevent. Binding per call costs one map
    /// insert on a scope that is released when the call returns.
    pub self_name: RefCell<Option<Rc<str>>>,
}

impl FunctionObject {
    /// Intern a parameter list's names for [`FunctionObject::param_keys`].
    ///
    /// Done once when the function value is created, so the per-call binding
    /// never allocates.
    pub fn intern_params(params: &[Param]) -> Vec<Rc<str>> {
        params.iter().map(|p| Rc::from(p.name.as_str())).collect()
    }

    /// The parameter keys a caller binds, with a leading `self` skipped the
    /// same way [`crate::eval::expr::user_params`] skips it, so the two stay
    /// index-aligned.
    pub fn user_param_keys(&self) -> &[Rc<str>] {
        if self.params.first().map(|p| p.name == "self") == Some(true) {
            &self.param_keys[1..]
        } else {
            &self.param_keys
        }
    }

    /// Attach this function to its owning class. No-op when called more than
    /// once; the first owner wins.
    pub fn set_owner_class(&self, class: &Rc<ClassObject>) {
        let mut slot = self.owner_class.borrow_mut();
        if slot.is_none() {
            *slot = Some(Rc::downgrade(class));
        }
    }

    /// Mark this function as reachable under `name` from inside its own body.
    /// See [`self_name`](Self::self_name).
    pub fn set_self_name(&self, name: impl Into<Rc<str>>) {
        *self.self_name.borrow_mut() = Some(name.into());
    }

    /// Resolve the owning class, if any. Returns `None` once the class has
    /// been dropped (which shouldn't happen in practice because the class
    /// outlives its methods).
    pub fn resolved_owner(&self) -> Option<Rc<ClassObject>> {
        self.owner_class.borrow().as_ref().and_then(|w| w.upgrade())
    }
}

/// Function bodies come in two shapes:
///   * a block of statements (named `fn` decls, block-body lambdas),
///   * a single expression (arrow-style lambdas like `(x) => x + 1`).
///
/// Shared by `Arc` so building a [`FunctionObject`] from a lambda's AST is
/// a refcount bump rather than a deep copy of the body — a lambda inside a
/// loop is re-evaluated on every iteration. See [`saule_ast::LambdaBody`],
/// which uses the same representation so the two share one allocation.
#[derive(Debug, Clone)]
pub enum FunctionBody {
    Block(Arc<[Spanned<Stmt>]>),
    Expr(Arc<Spanned<Expr>>),
}
