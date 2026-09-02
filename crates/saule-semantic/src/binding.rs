//! The published binding table (`VM_DESIGN.md` §21.1 item 0.6).
//!
//! The resolver already knows, for every identifier, whether it names a
//! local, an enclosing function's variable, a module-level declaration, a
//! class static, or the prelude — that is exactly what
//! [`SemanticError::UndefinedName`](crate::SemanticError::UndefinedName) is
//! deciding. It just throws the answer away afterwards.
//!
//! Recovering it is what turns `env.borrow().get("total")` — a `RefCell`
//! borrow, a string hash, a bucket probe, and on a miss the same again one
//! scope up — into `R[1]`.
//!
//! ## Why upvalues are computed here and not in the interpreter
//!
//! `crates/saule-interpreter/src/capture.rs` answers a related question by
//! over-approximating: "every identifier this lambda's body mentions". Its
//! own documentation is candid that it **bails out entirely** on a nested
//! declaration and falls back to capturing the whole enclosing scope, which
//! is where a class of leaks comes from.
//!
//! The resolver is the only pass that already carries the scope stack needed
//! to answer it exactly, so the exact answer is computed here — once, at
//! analysis time — and both engines can read it.

use std::collections::HashMap;
use std::rc::Rc;

use saule_ast::NodeId;

/// What an identifier refers to.
///
/// The variants are ordered the way resolution consults them: the current
/// function's own slots, then an enclosing function's, then module scope,
/// then the prelude.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Binding {
    /// A slot in the current function's frame — a register.
    ///
    /// Note there is no block depth here: block scoping is a *compile-time*
    /// notion, and by the time a name is resolved the slot is already
    /// absolute within the frame. Two blocks that cannot be live at once may
    /// share a slot.
    Local { slot: u16 },
    /// A variable of an enclosing function, reached through this closure's
    /// upvalue list. See [`FunctionInfo::upvals`].
    Upvalue { index: u16 },
    /// A top-level binding of this module — flat, unhashed (`GETMOD`).
    Module { slot: u16 },
    /// A static member of the enclosing class, referenced by its bare name
    /// from inside one of that class's methods.
    ///
    /// Not resolved to a slot here: statics live on the class, and which
    /// class in the chain *declares* the name is a question this pass cannot
    /// answer (it has no parent-class information at this point). The
    /// compiler resolves it against the class layout.
    ClassStatic { class: Rc<str>, name: Rc<str> },
    /// A prelude name — `print`, `Math`, `Os`, and friends.
    Prelude { name: Rc<str> },
    /// `self`.
    SelfRef,
    /// The name resolved to nothing the resolver can see, but the module has
    /// an unenumerable `import * from "..."` so it is not reported as an
    /// error.
    ///
    /// Distinguished from simply being absent from the table: absent means
    /// "not recorded", this means "recorded, and known to be unknowable".
    /// The compiler must fall back to a dynamic lookup rather than assume.
    WildcardImport,
}

/// Where one upvalue comes from at the moment its closure is built.
///
/// The direct source for `saule_vm::chunk::UpvalDesc` — this is the exact
/// version of the "flat capture" the interpreter approximates today.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpvalRef {
    /// A slot of the immediately enclosing function's frame.
    ParentLocal { slot: u16 },
    /// An upvalue of the immediately enclosing closure — the link in the
    /// chain that lets a name cross more than one function boundary.
    ParentUpvalue { index: u16 },
}

/// Per-function facts the compiler needs before it can lay out a frame.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FunctionInfo {
    /// Captured variables, in upvalue-index order. **This is the precise
    /// free-variable set**: exactly the enclosing bindings the body
    /// references, no over-approximation and no whole-scope fallback.
    pub upvals: Vec<UpvalRef>,
    /// Names of those upvalues, positionally — diagnostics and disassembly.
    pub upval_names: Vec<Rc<str>>,
    /// Slots this function's locals occupy, high-water mark. Parameters take
    /// the first `n_params` of them.
    pub n_slots: u16,
    pub n_params: u16,
    /// Whether this function's body — or anything nested inside it —
    /// mentions `self`.
    ///
    /// `self` is not an identifier, so it never appears in `upvals`, but a
    /// lambda written inside a method still has to reach the enclosing
    /// `self`. Tracked separately rather than being synthesised as a name so
    /// the upvalue list stays exactly the set of *bindings* the body refers
    /// to.
    pub captures_self: bool,
}

/// Identifier node -> what it refers to.
pub type ResolveTable = HashMap<NodeId, Binding>;

/// Function-defining node -> its frame facts.
///
/// Keyed by the `NodeId` of the node that *introduces* the function: the
/// `Spanned<Expr>` of a lambda, the `Spanned<Decl>` of an `fn`, the
/// `Spanned<ClassMember>` of a method.
pub type FunctionTable = HashMap<NodeId, FunctionInfo>;

/// Everything the resolver learned, published together.
#[derive(Debug, Clone, Default)]
pub struct Bindings {
    pub names: ResolveTable,
    pub functions: FunctionTable,
    /// Module-level slot assignment, in declaration order. The compiler
    /// sizes `Chunk::module_slots` from this.
    pub module_slots: Vec<Rc<str>>,
}

impl Bindings {
    pub fn get(&self, id: NodeId) -> Option<&Binding> {
        self.names.get(&id)
    }

    pub fn function(&self, id: NodeId) -> Option<&FunctionInfo> {
        self.functions.get(&id)
    }
}
