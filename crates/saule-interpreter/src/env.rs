//! Lexically-scoped variable environment.
//!
//! Environments form a parent chain. Each block (function body, `if`/`while`
//! body, etc.) creates a child via [`Environment::with_parent`] and is
//! dropped when execution leaves the scope.

use crate::fxhash::FxHashMap as HashMap;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use crate::module::ModuleLoader;
use crate::value::{ClassObject, Value};

/// Lexical scope: a `HashMap` of locals plus an optional parent pointer.
///
/// The optional `module_dir` and `loader` fields are only populated on the
/// root scope created by [`Environment::with_prelude_and_context`]; child
/// scopes inherit them by walking the parent chain via [`module_dir`] and
/// [`loader`].
#[derive(Debug, Default)]
pub struct Environment {
    parent: Option<Rc<RefCell<Environment>>>,
    vars: HashMap<Rc<str>, Value>,
    /// Bindings a lambda captured, moved out of `vars` and behind a shared
    /// cell so the scope and the closure observe each other's writes — the
    /// behaviour the old whole-scope capture got for free by pointing at the
    /// scope itself. See [`capture_flat`](Self::capture_flat).
    ///
    /// A second map rather than an enum in `vars` so the change costs the
    /// common path nothing: reads still hit `vars` first with the same layout
    /// and the same single lookup they always had, and this map is empty in
    /// every scope that never had a lambda written in it — which is nearly
    /// all of them. Wrapping every binding in a `Direct | Cell` enum instead
    /// measured 3–8% slower across the benchmark suite, paid by all code to
    /// benefit almost none.
    ///
    /// A name is in `vars` or in `cells`, never both — except after a
    /// rebinding `local`, where `vars` wins and the stale cell is dropped.
    cells: HashMap<Rc<str>, Rc<RefCell<Value>>>,
    /// Set on a method-call scope so the class's statics are reachable by
    /// bare name inside the body. Consulted by [`get`](Self::get) *after*
    /// `vars` and *before* `parent`, which reproduces the precedence of
    /// eagerly copying every static into `vars` — parameters and locals
    /// shadow statics, statics shadow the enclosing closure — without
    /// paying a map insert per static per call.
    statics_owner: Option<Rc<ClassObject>>,
    module_dir: Option<PathBuf>,
    loader: Option<Rc<RefCell<ModuleLoader>>>,
}

/// How many spent scopes to keep alive for reuse.
///
/// Scopes are handed back as the evaluator unwinds, so the pool only ever
/// needs to cover the deepest point of a call chain plus a little slack; a
/// bound keeps a program that briefly nests deeply from holding onto the
/// memory for the rest of the run.
const POOL_CAPACITY: usize = 128;

thread_local! {
    static POOL: RefCell<Vec<Rc<RefCell<Environment>>>> =
        const { RefCell::new(Vec::new()) };
}

impl Environment {
    /// Empty global scope.
    pub fn new() -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(Self::default()))
    }

    /// Child scope of `parent`.
    ///
    /// Takes a scope from the recycle pool when one is free, so the common
    /// case costs a pointer pop rather than an `Rc` allocation plus the
    /// bucket array `vars` grows on its first insert. See [`release`].
    pub fn with_parent(parent: Rc<RefCell<Self>>) -> Rc<RefCell<Self>> {
        if let Some(env) = POOL.with(|p| p.borrow_mut().pop()) {
            env.borrow_mut().parent = Some(parent);
            return env;
        }
        Rc::new(RefCell::new(Self {
            parent: Some(parent),
            vars: HashMap::default(),
            cells: HashMap::default(),
            statics_owner: None,
            module_dir: None,
            loader: None,
        }))
    }

    /// Offer a finished scope back to the recycle pool.
    ///
    /// A scope is reusable only when nothing else can still observe it —
    /// `strong_count == 1` means no closure captured it and no child scope
    /// outlived it, so its identity is dead and the next
    /// [`with_parent`](Self::with_parent) may hand the same allocation out
    /// again. Anything captured is simply dropped as before, which is what
    /// keeps per-call and per-iteration capture semantics intact.
    ///
    /// The bindings are cleared but `vars`'s capacity is kept — that
    /// retained bucket array is most of the point, since a scope holding a
    /// handful of parameters otherwise pays a fresh table allocation on
    /// every single call.
    pub fn release(scope: Rc<RefCell<Self>>) {
        if Rc::strong_count(&scope) != 1 {
            return;
        }
        {
            let mut b = scope.borrow_mut();
            b.vars.clear();
            if !b.cells.is_empty() {
                b.cells.clear();
            }
            b.statics_owner = None;
            b.module_dir = None;
            b.loader = None;
            // Dropped last, and outside the pool's borrow: releasing the
            // parent link can cascade into dropping a whole closure chain.
            b.parent = None;
        }
        POOL.with(|p| {
            let mut pool = p.borrow_mut();
            if pool.len() < POOL_CAPACITY {
                pool.push(scope);
            }
        });
    }

    /// Hand back a loop body's scope for the next iteration.
    ///
    /// A loop body gets a scope of its own per iteration so a closure built in
    /// one pass keeps the values that pass saw. Almost no body builds a
    /// closure, and for those the scope is dead the moment the iteration ends
    /// — building a fresh one every time round was the single largest cost in
    /// a tight loop.
    ///
    /// This is [`release`](Self::release) plus [`with_parent`](Self::with_parent)
    /// collapsed into one step: when the scope is uncaptured its parent link
    /// is already the right one, so clearing the bindings is the whole job and
    /// the pool need not be touched at all. When something *did* capture it,
    /// it is left alone and a new scope starts the next iteration, which is
    /// what keeps per-iteration capture semantics intact.
    pub fn recycle(scope: Rc<RefCell<Self>>, parent: &Rc<RefCell<Self>>) -> Rc<RefCell<Self>> {
        if Rc::strong_count(&scope) == 1 {
            let mut b = scope.borrow_mut();
            b.vars.clear();
            // A closure built in the previous iteration still holds the cells
            // it captured; dropping this scope's handle on them is what gives
            // the next iteration a fresh binding.
            if !b.cells.is_empty() {
                b.cells.clear();
            }
            drop(b);
            return scope;
        }
        Self::with_parent(parent.clone())
    }

    /// Global scope pre-populated with the standard built-ins.
    pub fn with_prelude() -> Rc<RefCell<Self>> {
        let env = Self::new();
        crate::stdlib::install_std(&env);
        env
    }

    /// Like [`Environment::with_prelude`], but also stamps the root scope
    /// with the importing file's directory and a shared module loader so
    /// `import "..."` statements can resolve relative paths and dedupe
    /// already-loaded modules.
    pub fn with_prelude_and_context(
        module_dir: Option<PathBuf>,
        loader: Option<Rc<RefCell<ModuleLoader>>>,
    ) -> Rc<RefCell<Self>> {
        let env = Self::with_prelude();
        {
            let mut b = env.borrow_mut();
            b.module_dir = module_dir;
            b.loader = loader;
        }
        env
    }

    /// Define (or shadow) a local in this scope.
    ///
    /// The key is an `Rc<str>` so the hot paths can hand over a name they
    /// already hold — a function's interned parameter names, or the `self`
    /// key — and pay a refcount bump instead of a fresh allocation. Cold
    /// paths passing a `String` or `&str` still convert, which is what they
    /// did before.
    pub fn define(&mut self, name: impl Into<Rc<str>>, value: Value) {
        let key = name.into();
        // A fresh `local` is a fresh binding, so it drops any cell a previous
        // binding of the same name was captured through rather than writing
        // into it. That is what gives a loop body's closures one captured
        // variable per iteration.
        if !self.cells.is_empty() {
            self.cells.remove(&key);
        }
        self.vars.insert(key, value);
    }

    /// Make `class`'s statics visible by bare name in this scope. See
    /// [`statics_owner`](Self::statics_owner).
    pub fn set_statics_owner(&mut self, class: Rc<ClassObject>) {
        self.statics_owner = Some(class);
    }

    /// Look up a name, walking parent scopes until found.
    pub fn get(&self, name: &str) -> Option<Value> {
        if let Some(v) = self.vars.get(name) {
            return Some(v.clone());
        }
        if !self.cells.is_empty()
            && let Some(cell) = self.cells.get(name)
        {
            return Some(cell.borrow().clone());
        }
        if let Some(class) = &self.statics_owner
            && let Some(v) = class_static(class, name)
        {
            return Some(v);
        }
        match &self.parent {
            Some(parent) => parent.borrow().get(name),
            None => None,
        }
    }

    /// Assign to an existing variable in this or an ancestor scope.
    /// Returns `false` if no such binding exists.
    pub fn assign(&mut self, name: &str, value: Value) -> bool {
        if let Some(slot) = self.vars.get_mut(name) {
            *slot = value;
            return true;
        }
        if !self.cells.is_empty()
            && let Some(cell) = self.cells.get(name)
        {
            *cell.borrow_mut() = value;
            return true;
        }
        // Bare-name write to one of the owning class's statics. This has to
        // reach the class itself: `static local` members are invisible from
        // outside the class, so a bare-name write is the *only* way to
        // mutate one — routing it to a scope-local copy would make them
        // silently immutable.
        match self
            .statics_owner
            .as_ref()
            .and_then(|c| resolve_static_write(c, name))
        {
            Some(StaticTarget::Field(owner)) => {
                owner
                    .static_fields
                    .borrow_mut()
                    .insert(name.to_string(), value);
                return true;
            }
            // Assigning to a static *method*'s name. `static_methods` is
            // immutable, so the binding can only be shadowed for the rest
            // of this scope, which is what the old eager injection did.
            Some(StaticTarget::Method) => {
                self.vars.insert(Rc::from(name), value);
                return true;
            }
            None => {}
        }
        if let Some(parent) = &self.parent {
            return parent.borrow_mut().assign(name, value);
        }
        false
    }

    /// Walk the parent chain to find the nearest set `module_dir`.
    pub fn module_dir(&self) -> Option<PathBuf> {
        if self.module_dir.is_some() {
            return self.module_dir.clone();
        }
        self.parent.as_ref().and_then(|p| p.borrow().module_dir())
    }

    /// Walk the parent chain to find the nearest attached module loader.
    pub fn loader(&self) -> Option<Rc<RefCell<ModuleLoader>>> {
        if self.loader.is_some() {
            return self.loader.clone();
        }
        self.parent.as_ref().and_then(|p| p.borrow().loader())
    }

    /// Build the scope a lambda closes over: one flat frame holding just the
    /// names its body mentions, parented straight to the module root.
    ///
    /// A lambda used to capture the defining scope itself. That is a cycle as
    /// soon as the lambda is stored back into it — `local f = fn() … end`, the
    /// ordinary way to write a helper — because the scope holds the function
    /// and the function holds the scope, both strongly. The scope, everything
    /// bound in it, and its whole parent chain then leaked on every execution.
    ///
    /// Capturing names instead of the frame breaks the cycle: nothing the
    /// closure holds points back at the scope that created it. Captured
    /// bindings move into [`cells`](Self::cells) and are shared, so the
    /// closure and the original scope still read and write one location — the
    /// live-binding semantics closures had before, kept deliberately:
    ///
    /// ```saule
    /// fn counter(stop: integer) -> fn() -> integer?
    ///   local i: integer = 0
    ///   return fn()          -- mutates the `i` above, and outlives it
    ///     i = i + 1
    ///     return i
    ///   end
    /// end
    /// ```
    ///
    /// `names` over-approximates: see [`crate::capture`] for why that is the
    /// safe direction and how shadowing survives it.
    pub fn capture_flat(env: &Rc<RefCell<Self>>, names: &[Rc<str>]) -> Rc<RefCell<Self>> {
        let root = Self::root_of(env);
        // A lambda at module scope has nothing to flatten — the root is what
        // it would capture anyway, and the root outlives the program, so
        // holding it strongly leaks nothing.
        if Rc::ptr_eq(env, &root) {
            return root;
        }

        let mut flat = Self {
            parent: Some(root),
            vars: HashMap::default(),
            cells: HashMap::default(),
            statics_owner: Self::nearest_statics_owner(env),
            module_dir: None,
            loader: None,
        };
        for name in names {
            if let Some(cell) = Self::promote_in_chain(env, name) {
                flat.cells.insert(name.clone(), cell);
            }
        }
        Rc::new(RefCell::new(flat))
    }

    /// Forget a captured binding.
    ///
    /// Used for exactly one thing: a self-recursive local closure captures its
    /// own name like any other, and that closes a cycle (cell → function →
    /// this scope → cell). Dropping it here and letting the call scope bind
    /// the name to the function directly keeps the recursion working without
    /// the cycle — see
    /// [`FunctionObject::self_name`](crate::value::FunctionObject::self_name).
    pub fn drop_capture(&mut self, name: &str) {
        self.cells.remove(name);
    }

    /// The bottom of a scope chain: the module scope carrying top-level
    /// declarations, the prelude, `module_dir`, and the loader.
    fn root_of(env: &Rc<RefCell<Self>>) -> Rc<RefCell<Self>> {
        let mut cur = env.clone();
        loop {
            let parent = cur.borrow().parent.clone();
            match parent {
                Some(p) => cur = p,
                None => return cur,
            }
        }
    }

    /// Find `name` between `env` and the root (exclusive) and make it
    /// shareable.
    ///
    /// The root is skipped on purpose. Its bindings are the module's
    /// top-level names, which the flat scope reaches through its parent link
    /// at call time. Capturing them here instead would freeze a forward
    /// reference — `fn a() b() end` written before `b` exists — at the moment
    /// the lambda was built, when the binding is not there to promote.
    fn promote_in_chain(env: &Rc<RefCell<Self>>, name: &str) -> Option<Rc<RefCell<Value>>> {
        let mut cur = env.clone();
        loop {
            // Taken before the lookup so reaching the root ends the walk
            // without ever consulting its bindings.
            let parent = cur.borrow().parent.clone()?;
            if let Some(cell) = cur.borrow_mut().promote(name) {
                return Some(cell);
            }
            cur = parent;
        }
    }

    /// Move `name` out of [`vars`](Self::vars) and behind a shared cell,
    /// returning the cell to share.
    ///
    /// Idempotent: a binding captured by two lambdas hands both the same
    /// cell, so all three views of the variable stay in agreement.
    fn promote(&mut self, name: &str) -> Option<Rc<RefCell<Value>>> {
        if let Some(cell) = self.cells.get(name) {
            return Some(cell.clone());
        }
        let (key, value) = self.vars.remove_entry(name)?;
        let cell = Rc::new(RefCell::new(value));
        self.cells.insert(key, cell.clone());
        Some(cell)
    }

    /// The nearest enclosing method scope's class, so a lambda written inside
    /// a method keeps bare-name access to that class's statics.
    fn nearest_statics_owner(env: &Rc<RefCell<Self>>) -> Option<Rc<ClassObject>> {
        let mut cur = env.clone();
        loop {
            let owner = cur.borrow().statics_owner.clone();
            if owner.is_some() {
                return owner;
            }
            let parent = cur.borrow().parent.clone();
            cur = parent?;
        }
    }
}

/// Resolve a bare name against a class's statics, walking the inheritance
/// chain from the class itself up to the root.
///
/// The per-level order (methods before fields, nearest class first) mirrors
/// exactly what eager injection produced: it seeded each class root-first,
/// writing that class's fields and then its methods into the scope, so a
/// nearer class overwrote a farther one and — within one class — a method
/// overwrote a same-named field. Colliding names are almost certainly
/// rejected earlier by `saule_semantic`, but matching the old precedence
/// costs nothing and keeps this a pure optimization.
fn class_static(class: &Rc<ClassObject>, name: &str) -> Option<Value> {
    let mut cur = Some(class);
    while let Some(c) = cur {
        if let Some(m) = c.static_methods.get(name) {
            return Some(Value::Function(m.clone()));
        }
        if let Some(v) = c.static_fields.borrow().get(name) {
            return Some(v.clone());
        }
        cur = c.parent.as_ref();
    }
    None
}

/// What a bare-name assignment inside a method body resolves to.
enum StaticTarget {
    /// A static field, held by the class in the chain that declares it.
    Field(Rc<ClassObject>),
    /// A static method — a name that exists but has no writable slot.
    Method,
}

/// Write-side counterpart to [`class_static`], using the same chain order so
/// a name always reads and writes through the same member.
fn resolve_static_write(class: &Rc<ClassObject>, name: &str) -> Option<StaticTarget> {
    let mut cur = Some(class);
    while let Some(c) = cur {
        if c.static_methods.contains_key(name) {
            return Some(StaticTarget::Method);
        }
        if c.static_fields.borrow().contains_key(name) {
            return Some(StaticTarget::Field(c.clone()));
        }
        cur = c.parent.as_ref();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `(root, outer, inner)` — see [`chain`].
    type Scopes = (
        Rc<RefCell<Environment>>,
        Rc<RefCell<Environment>>,
        Rc<RefCell<Environment>>,
    );

    /// `root <- outer <- inner`, with `x` bound in `outer`.
    fn chain() -> Scopes {
        let root = Environment::new();
        let outer = Environment::with_parent(root.clone());
        outer.borrow_mut().define("x", Value::Int(1));
        let inner = Environment::with_parent(outer.clone());
        (root, outer, inner)
    }

    /// The property the whole change exists for: what a lambda closes over
    /// must not hold the scope it was written in, or the scope holding the
    /// lambda and the lambda holding the scope form a cycle that `Rc` can
    /// never break.
    #[test]
    fn capture_does_not_retain_the_defining_scope() {
        let (_root, outer, inner) = chain();
        let before = Rc::strong_count(&outer);

        let captured = Environment::capture_flat(&inner, &[Rc::from("x")]);

        assert_eq!(
            Rc::strong_count(&outer),
            before,
            "the captured scope took a reference to the defining scope"
        );
        assert_eq!(Rc::strong_count(&inner), 1, "inner scope was retained");
        assert_eq!(captured.borrow().get("x"), Some(Value::Int(1)));
    }

    /// Captured bindings are shared, not copied: the closure sees the
    /// scope's later writes and the scope sees the closure's.
    #[test]
    fn capture_shares_the_binding_both_ways() {
        let (_root, outer, inner) = chain();
        let captured = Environment::capture_flat(&inner, &[Rc::from("x")]);

        outer.borrow_mut().assign("x", Value::Int(2));
        assert_eq!(captured.borrow().get("x"), Some(Value::Int(2)));

        captured.borrow_mut().assign("x", Value::Int(3));
        assert_eq!(outer.borrow().get("x"), Some(Value::Int(3)));
    }

    /// Two lambdas over one variable land on the same cell.
    #[test]
    fn two_captures_of_one_binding_agree() {
        let (_root, _outer, inner) = chain();
        let a = Environment::capture_flat(&inner, &[Rc::from("x")]);
        let b = Environment::capture_flat(&inner, &[Rc::from("x")]);

        a.borrow_mut().assign("x", Value::Int(9));
        assert_eq!(b.borrow().get("x"), Some(Value::Int(9)));
    }

    /// Module-level names are left in the root and reached through the
    /// parent link, so a lambda still sees a top-level binding that did not
    /// exist when it was built (forward references between declarations).
    #[test]
    fn root_bindings_resolve_later_not_at_capture_time() {
        let (root, _outer, inner) = chain();
        let captured = Environment::capture_flat(&inner, &[Rc::from("later")]);
        assert_eq!(captured.borrow().get("later"), None);

        root.borrow_mut().define("later", Value::Int(7));
        assert_eq!(captured.borrow().get("later"), Some(Value::Int(7)));
    }

    /// A lambda written at module scope has nothing to flatten.
    #[test]
    fn capture_at_module_scope_is_the_root_itself() {
        let root = Environment::new();
        let captured = Environment::capture_flat(&root, &[Rc::from("x")]);
        assert!(Rc::ptr_eq(&captured, &root));
    }

    /// Naming something that is not bound anywhere captures nothing — the
    /// capture set is allowed to over-approximate.
    #[test]
    fn unknown_names_are_ignored() {
        let (_root, _outer, inner) = chain();
        let captured = Environment::capture_flat(&inner, &[Rc::from("nope")]);
        assert_eq!(captured.borrow().get("nope"), None);
    }

    /// A fresh `local` rebinds rather than writing through the cell an
    /// earlier binding of that name was captured through. This is what gives
    /// a loop body's closures one variable per iteration.
    #[test]
    fn redefining_breaks_the_old_capture() {
        let (_root, outer, inner) = chain();
        let captured = Environment::capture_flat(&inner, &[Rc::from("x")]);

        outer.borrow_mut().define("x", Value::Int(100));

        assert_eq!(captured.borrow().get("x"), Some(Value::Int(1)));
        assert_eq!(outer.borrow().get("x"), Some(Value::Int(100)));
    }
}
