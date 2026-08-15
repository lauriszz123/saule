//! Name resolution + a handful of small structural checks bundled into a
//! single AST walker so we don't traverse the tree N times.
//!
//! Emits:
//!
//! * [`SemanticError::UndefinedName`] — a bare ident that isn't in any
//!   lexical scope, isn't a top-level declaration of this module, isn't
//!   an imported name, and isn't in the prelude.
//! * [`SemanticError::AssignToUndeclared`] — an assignment whose LHS
//!   ident isn't in scope (the runtime previously caught these).
//! * [`SemanticError::SelfOutsideClass`] — `self` referenced outside any
//!   method body.
//! * [`SemanticError::SuperOutsideClass`] — `super.x` or `super(...)` used
//!   outside a method.
//! * [`SemanticError::SuperCallOutsideInit`] — `self.super(...)` outside
//!   `init`.
//! * [`SemanticError::MultipleVariadicParams`] /
//!   [`SemanticError::VariadicNotLast`] — declaration-time variadic shape.
//! * [`SemanticError::PositionalAfterNamed`] — argument-list ordering.
//! * [`SemanticError::ForInArity`] — `for v1, v2, v3 in iter` is invalid.
//!
//! It also **publishes what it learned**: see [`crate::binding`] and
//! [`crate::analyze_with_bindings`]. Deciding whether a name is defined
//! means working out exactly where it is defined, and that answer is what
//! turns `env.get("total")` into `R[1]`.
//!
//! ## Scoping
//!
//! Two nested stacks, not one:
//!
//! * a stack of **function** scopes ([`FuncState`]), because crossing a
//!   function boundary changes what a name *is* — a local becomes an
//!   upvalue;
//! * within each, a stack of **block** scopes, so a `local` declared in a
//!   then-branch doesn't leak to the else-branch (Lua-style block scoping).
//!
//! Each block records `name -> slot`, and leaving a block returns its slots
//! to the pool, so sibling blocks share registers.
//!
//! `funcs[0]` is the module. Its *top-level* declarations are module slots —
//! visible file-wide, readable by importers, and reached from inner
//! functions directly rather than through a capture chain. A `local`
//! declared inside a block at top level is not: it is an ordinary local of
//! the module body, and treating it otherwise is precisely the leak block
//! scoping exists to prevent.
//!
//! A `local` binds *after* its initializer is walked, so `local x = x + 1`
//! reports an undefined name rather than resolving to the half-built `x`.
//! The one exception is a lambda initializer, which binds *before*: that is
//! what lets a local function call itself
//! (`local fact = fn(n) … fact(n-1) … end`). Keeping the exception to that
//! shape is deliberate — it is the only one where the name genuinely cannot
//! be in scope yet at the point it is used.
//!
//! Functions, methods, and lambdas push a frame and reset the
//! enclosing-class / `in_init` flags appropriately. Module-level
//! declarations are pre-collected before the walk so forward references
//! to top-level `fn` / `class` / etc. resolve cleanly.
//!
//! ## Wildcard imports
//!
//! `import * from "..."` introduces names this crate can't enumerate on
//! its own — it has no module loader. The embedder resolves them and
//! hands the result over as [`crate::ModuleSeed::wildcard_names`]:
//!
//! * `Some(names)` — every wildcard target was enumerated. The names go
//!   into the module scope and the [`UndefinedName`] /
//!   [`AssignToUndeclared`] checks stay fully active, so a typo still
//!   gets reported in a file that globs a module.
//! * `None` — at least one target couldn't be enumerated (or the
//!   embedder doesn't resolve imports at all). Those two checks then
//!   become advisory for any module containing a wildcard import: we
//!   still walk the AST for the other diagnostics, but ident lookups
//!   that would otherwise fail are silently accepted.

mod decls;
mod exprs;

mod scope;

pub(crate) use scope::*;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use saule_ast::{Expr, Module, NodeId, Spanned, Stmt};

use crate::binding::{Binding, Bindings, FunctionInfo, UpvalRef};
use crate::error::SemanticError;
use crate::prelude;
use crate::to_source_span;

/// One function's worth of scope: the block frames inside it, its slot
/// counter, and the upvalues it turned out to capture.
///
/// Level 0 is the module itself, which is a function only in the sense that
/// it has a body. Its bindings resolve to [`Binding::Module`], not to
/// registers, and it is never captured as an upvalue — an inner function
/// reaching a top-level name reads a module slot directly (`GETMOD`), which
/// is both cheaper and simpler than threading it through a capture chain.
#[derive(Default)]
struct FuncState {
    /// Block scopes, innermost last. Each is `name -> slot`.
    blocks: Vec<Vec<(Rc<str>, u16)>>,
    /// Next free slot in this frame.
    next_slot: u16,
    /// High-water mark — the frame size.
    n_slots: u16,
    n_params: u16,
    upvals: Vec<UpvalRef>,
    upval_names: Vec<Rc<str>>,
    /// name -> upvalue index, so a name captured twice gets one entry.
    upval_index: HashMap<Rc<str>, u16>,
    /// Statics of the enclosing class, visible by bare name inside its
    /// methods. Held apart from `blocks` because they are not slots.
    statics: HashSet<Rc<str>>,
    class: Option<Rc<str>>,
    /// Set when this function, or anything nested in it, mentions `self`.
    captures_self: bool,
    /// The node that introduced this function, for the function table.
    node: NodeId,
}

struct Resolver {
    /// Function scope stack; `funcs[0]` is the module.
    funcs: Vec<FuncState>,
    /// Module-level names in declaration order — index is the module slot.
    module_slots: Vec<Rc<str>>,
    module_index: HashMap<Rc<str>, u16>,
    /// Class context for `self` / `super` validity. `None` at module scope.
    in_class: Option<String>,
    /// Are we currently inside the `init` constructor body of a class?
    in_init: bool,
    /// Walking inside any method body (including `init`). `self` is legal,
    /// `super.x` is legal (when a parent exists; we don't check that here),
    /// `self.super(...)` is only legal when also `in_init`.
    in_method: bool,
    /// True when the module contains `import * from "..."` *and* the
    /// embedder couldn't tell us what those imports bind. Suppresses
    /// undefined-name diagnostics, since any unknown ident might have
    /// come in through the glob.
    has_opaque_wildcard_import: bool,
    errors: Vec<SemanticError>,
    /// Collected only when someone asked for it — `analyze_with_seed` runs
    /// on every language-server keystroke and should not build a table
    /// nobody reads.
    bindings: Option<Bindings>,
}

pub(crate) fn check(
    module: &Module,
    wildcard_names: Option<&HashSet<String>>,
    errors: &mut Vec<SemanticError>,
    collect: bool,
) -> Option<Bindings> {
    let mut module_slots = collect_module_scope(module);
    if let Some(names) = wildcard_names {
        // Deterministic order: a `HashSet` iterates arbitrarily, and module
        // slot numbers must not depend on hash seed.
        let mut extra: Vec<&String> = names.iter().collect();
        extra.sort();
        for n in extra {
            if !module_slots.iter().any(|s| s.as_ref() == n.as_str()) {
                module_slots.push(Rc::from(n.as_str()));
            }
        }
    }
    let module_index = module_slots
        .iter()
        .enumerate()
        .map(|(i, n)| (Rc::clone(n), i as u16))
        .collect();

    let mut r = Resolver {
        funcs: vec![FuncState {
            blocks: vec![Vec::new()],
            ..FuncState::default()
        }],
        module_slots,
        module_index,
        in_class: None,
        in_init: false,
        in_method: false,
        has_opaque_wildcard_import: wildcard_names.is_none() && module_has_wildcard_import(module),
        errors: Vec::new(),
        bindings: collect.then(Bindings::default),
    };

    for s in &module.stmts {
        r.stmt(s);
    }

    errors.append(&mut r.errors);
    r.bindings.map(|mut b| {
        b.module_slots = r.module_slots;
        b
    })
}

impl Resolver {
    // ── scope machinery ────────────────────────────────────────────────────

    fn push_scope(&mut self) {
        self.func().blocks.push(Vec::new());
    }

    fn pop_scope(&mut self) {
        // Slots freed by leaving a block are reused by the next one, which
        // is the stack discipline of `VM_DESIGN.md` §18.
        let f = self.func();
        if let Some(block) = f.blocks.pop() {
            f.next_slot = f.next_slot.saturating_sub(block.len() as u16);
        }
    }

    /// Open a function scope. `node` keys its entry in the function table.
    fn push_function(&mut self, node: NodeId) {
        let class = self.funcs.last().and_then(|f| f.class.clone());
        self.funcs.push(FuncState {
            blocks: vec![Vec::new()],
            class,
            node,
            ..FuncState::default()
        });
    }

    fn pop_function(&mut self) {
        let f = self.funcs.pop().expect("function scope");
        if let Some(b) = self.bindings.as_mut()
            && !f.node.is_none()
        {
            b.functions.insert(
                f.node,
                FunctionInfo {
                    upvals: f.upvals,
                    upval_names: f.upval_names,
                    n_slots: f.n_slots,
                    n_params: f.n_params,
                    captures_self: f.captures_self,
                },
            );
        }
    }

    fn func(&mut self) -> &mut FuncState {
        self.funcs.last_mut().expect("function scope")
    }

    fn declare(&mut self, name: &str) {
        let name: Rc<str> = Rc::from(name);
        let level = self.funcs.len() - 1;
        // A *top-level* declaration is a module slot: visible file-wide,
        // reachable by importers, and pre-collected so forward references
        // resolve. A declaration in a nested block at top level is not — it
        // is an ordinary local of the module body, and letting it into the
        // module scope is exactly the `if x then local y = 1 end` leak that
        // block scoping exists to prevent.
        if level == 0 && self.funcs[0].blocks.len() == 1 {
            if !self.module_index.contains_key(&name) {
                let slot = self.module_slots.len() as u16;
                self.module_slots.push(Rc::clone(&name));
                self.module_index.insert(name, slot);
            }
            return;
        }
        let f = self.func();
        let slot = f.next_slot;
        f.next_slot += 1;
        f.n_slots = f.n_slots.max(f.next_slot);
        f.blocks
            .last_mut()
            .expect("block scope")
            .push((name, slot));
    }

    fn declare_param(&mut self, name: &str) {
        self.declare(name);
        let f = self.func();
        f.n_params += 1;
    }

    fn declare_static(&mut self, name: &str) {
        let name: Rc<str> = Rc::from(name);
        self.func().statics.insert(name);
    }

    /// Find `name` among the block scopes of function level `level`.
    fn local_in(&self, level: usize, name: &str) -> Option<u16> {
        self.funcs[level]
            .blocks
            .iter()
            .rev()
            .find_map(|b| b.iter().rev().find(|(n, _)| n.as_ref() == name))
            .map(|(_, s)| *s)
    }

    /// Make `name` reachable from function `level` as an upvalue, adding a
    /// link to every function between it and the one that owns the variable.
    ///
    /// This is the exact free-variable analysis: a name becomes an upvalue
    /// only because the body actually mentioned it, and the chain records
    /// precisely which frame it came from.
    fn capture(&mut self, level: usize, name: &str) -> Option<u16> {
        if level == 0 {
            return None;
        }
        // Note the chain *does* reach level 0's block scopes. A top-level
        // `local` is a module slot and is found by name later, but one
        // declared inside a block at top level is an ordinary local of the
        // module body, and a closure written next to it captures it like any
        // other.
        let parent = level - 1;
        let source = match self.local_in(parent, name) {
            Some(slot) => UpvalRef::ParentLocal { slot },
            None => UpvalRef::ParentUpvalue {
                index: self.capture(parent, name)?,
            },
        };
        Some(self.add_upvalue(level, name, source))
    }

    fn add_upvalue(&mut self, level: usize, name: &str, source: UpvalRef) -> u16 {
        let key: Rc<str> = Rc::from(name);
        if let Some(i) = self.funcs[level].upval_index.get(&key) {
            return *i;
        }
        let f = &mut self.funcs[level];
        let index = f.upvals.len() as u16;
        f.upvals.push(source);
        f.upval_names.push(Rc::clone(&key));
        f.upval_index.insert(key, index);
        index
    }

    /// Resolve `name`, adding capture links as a side effect.
    fn resolve_binding(&mut self, name: &str) -> Option<Binding> {
        let level = self.funcs.len() - 1;

        if let Some(slot) = self.local_in(level, name) {
            return Some(Binding::Local { slot });
        }
        if let Some(index) = self.capture(level, name) {
            return Some(Binding::Upvalue { index });
        }
        // A static of the enclosing class, visible by bare name. Checked
        // after locals so a parameter of the same name still shadows it.
        for f in self.funcs[1..].iter().rev() {
            if f.statics.contains(name) {
                return Some(Binding::ClassStatic {
                    class: f.class.clone().unwrap_or_else(|| Rc::from("")),
                    name: Rc::from(name),
                });
            }
        }
        if let Some(slot) = self.module_index.get(name) {
            return Some(Binding::Module { slot: *slot });
        }
        if prelude::contains(name) {
            return Some(Binding::Prelude {
                name: Rc::from(name),
            });
        }
        None
    }

    /// Resolve `name` and record the answer against `id`.
    ///
    /// Returns whether it resolved, which is what the `UndefinedName` and
    /// `AssignToUndeclared` diagnostics are asking. The wildcard fallback is
    /// applied here so both the diagnostic and the table agree about it.
    fn resolve_at(&mut self, id: NodeId, name: &str) -> bool {
        match self.resolve_binding(name) {
            Some(b) => {
                self.record(id, b);
                true
            }
            None if self.has_opaque_wildcard_import => {
                self.record(id, Binding::WildcardImport);
                true
            }
            None => false,
        }
    }

    fn record(&mut self, id: NodeId, b: Binding) {
        if id.is_none() {
            return;
        }
        if let Some(t) = self.bindings.as_mut() {
            t.names.insert(id, b);
        }
    }

    fn resolved(&mut self, name: &str) -> bool {
        self.resolve_at(NodeId::NONE, name)
    }

    /// Record which class's methods we are inside, so a bare-name static
    /// read can say which class it belongs to.
    fn set_current_class(&mut self, name: &str) {
        self.func().class = Some(Rc::from(name));
    }

    /// Note that `self` was mentioned.
    ///
    /// Marked on **every** enclosing function, not just the innermost. A
    /// lambda two levels inside a method reaches `self` through the one
    /// between them, so that middle closure has to hold it too — the same
    /// reason an upvalue is threaded rather than grabbed directly.
    fn note_self(&mut self) {
        for f in self.funcs[1..].iter_mut() {
            f.captures_self = true;
        }
    }

    // ── Statements ─────────────────────────────────────────────────────────

    fn stmt(&mut self, stmt: &Spanned<Stmt>) {
        match &stmt.value {
            Stmt::Error => {}
            Stmt::Local { name, value, .. } => {
                // A lambda initializer sees the name being bound, so a local
                // function can call itself: `local fact = fn(n) … fact(n-1) …`.
                // Declaring first is scoped to that one shape deliberately —
                // doing it for every initializer would make `local x = x + 1`
                // resolve to the half-built `x` instead of reporting an
                // undefined (or shadowed-outer) name.
                let recursive =
                    matches!(value.as_ref().map(|v| &v.value), Some(Expr::Lambda { .. }));
                if recursive {
                    self.declare(name);
                }
                if let Some(v) = value {
                    self.expr(v);
                }
                if !recursive {
                    self.declare(name);
                }
            }
            Stmt::LocalMulti { names, values } => {
                for v in values {
                    self.expr(v);
                }
                for (n, _, _) in names {
                    self.declare(n);
                }
            }
            // Compound assignment resolves exactly like plain assignment:
            // an ident target still has to be declared, and `a op= b` reading
            // `a` before writing it makes that requirement stricter, not
            // looser.
            Stmt::Assign { target, value } | Stmt::CompoundAssign { target, value, .. } => {
                if let Expr::Ident(name) = &target.value
                    && !self.resolve_at(target.id, name)
                {
                    self.errors.push(SemanticError::AssignToUndeclared {
                        name: name.clone(),
                        span: to_source_span(target.span.clone()),
                    });
                } else {
                    // Targets other than plain idents go through `expr`
                    // for member/index resolution.
                    if !matches!(target.value, Expr::Ident(_)) {
                        self.expr(target);
                    }
                }
                self.expr(value);
            }
            Stmt::AssignMulti { targets, values } => {
                for v in values {
                    self.expr(v);
                }
                for t in targets {
                    if let Expr::Ident(name) = &t.value {
                        if !self.resolve_at(t.id, name) {
                            self.errors.push(SemanticError::AssignToUndeclared {
                                name: name.clone(),
                                span: to_source_span(t.span.clone()),
                            });
                        }
                    } else {
                        self.expr(t);
                    }
                }
            }
            Stmt::Expr(e) | Stmt::Throw(e) => self.expr(e),

            Stmt::If {
                cond,
                then_block,
                elseifs,
                else_block,
            } => {
                self.expr(cond);
                self.push_scope();
                self.block(then_block);
                self.pop_scope();
                for (c, b) in elseifs {
                    self.expr(c);
                    self.push_scope();
                    self.block(b);
                    self.pop_scope();
                }
                if let Some(b) = else_block {
                    self.push_scope();
                    self.block(b);
                    self.pop_scope();
                }
            }
            Stmt::While { cond, body } => {
                self.expr(cond);
                self.push_scope();
                self.block(body);
                self.pop_scope();
            }
            Stmt::Repeat { body, cond } => {
                // Lua-style: `until` cond sees locals declared in body, so
                // walk the cond *before* popping.
                self.push_scope();
                self.block(body);
                self.expr(cond);
                self.pop_scope();
            }
            Stmt::ForNumeric {
                var,
                from,
                to,
                step,
                body,
                ..
            } => {
                self.expr(from);
                self.expr(to);
                if let Some(s) = step {
                    self.expr(s);
                }
                self.push_scope();
                self.declare(var);
                self.block(body);
                self.pop_scope();
            }
            Stmt::ForIn { vars, iter, body } => {
                if vars.is_empty() || vars.len() > 2 {
                    self.errors.push(SemanticError::ForInArity {
                        found: vars.len(),
                        span: to_source_span(stmt.span.clone()),
                    });
                }
                self.expr(iter);
                self.push_scope();
                for (n, _) in vars {
                    self.declare(n);
                }
                self.block(body);
                self.pop_scope();
            }
            Stmt::Return(values) => {
                for v in values {
                    self.expr(v);
                }
            }
            Stmt::Try {
                body,
                catch_var,
                catch_body,
                ..
            } => {
                self.push_scope();
                self.block(body);
                self.pop_scope();
                self.push_scope();
                self.declare(catch_var);
                self.block(catch_body);
                self.pop_scope();
            }
            Stmt::Break | Stmt::Continue => {}

            Stmt::Decl(d) => self.decl(d),
        }
    }

    fn block(&mut self, body: &[Spanned<Stmt>]) {
        for s in body {
            self.stmt(s);
        }
    }

    // ── Declarations ───────────────────────────────────────────────────────
}
