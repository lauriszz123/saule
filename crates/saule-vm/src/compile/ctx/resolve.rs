//! Name resolution at compile time: what is this name, and where does it live?
//!
//! The resolver in `saule-semantic` already decided *what* every name is;
//! these read that answer back and turn it into something the emitter can
//! use — a module slot, a static slot, a proto index, a prelude value that
//! can be folded straight into the constant pool.

use std::ops::Range;

use saule_ast::NodeId;
use saule_interpreter::Value;
use saule_semantic::Binding;

use crate::compile::CompileError;

use super::Compiler;

impl Compiler<'_> {

    /// A stdlib *value* member — `Math.pi`, `Os.sep`, `IoMode.Write`.
    ///
    /// The same compile-time resolution `CALLNAT` does for `String.len`,
    /// applied to members that hold a value rather than a function. The
    /// prelude is fixed before a program runs, so the read becomes a
    /// `LOADK` and nothing looks anything up at run time.
    ///
    /// Only value-shaped results are returned. A `Table` or a `File` is a
    /// mutable handle and freezing one into the constant pool would make
    /// every read share a snapshot the tree-walker does not; those fall
    /// back instead.
    pub fn prelude_member(&self, id: NodeId, recv: &str, name: &str) -> Option<Value> {
        let v = match self.static_value(id, recv)? {
            Value::Class(cls) => cls.lookup_static_field(name)?,
            Value::Enum(e) => Value::EnumVariant(std::rc::Rc::clone(e.variants.get(name)?)),
            _ => return None,
        };
        matches!(
            v,
            Value::Int(_)
                | Value::Float(_)
                | Value::Str(_)
                | Value::Bool(_)
                | Value::EnumVariant(_)
        )
        .then_some(v)
    }

    /// The runtime value a prelude name is bound to, for `CALLNAT`.
    ///
    /// Resolved at compile time: the prelude is fixed before a program runs,
    /// so `print` can be a constant in the chunk rather than a name looked up
    /// on every call.
    pub fn prelude_value(&self, name: &str) -> Option<Value> {
        let mut slot = self.prelude.borrow_mut();
        if slot.is_none() {
            saule_interpreter::init();
            *slot = Some(saule_interpreter::Environment::with_prelude());
        }
        let env = slot.as_ref().expect("just installed");
        let v = env.borrow().get(name);
        v
    }

    /// The value a bare name denotes at **compile** time, if any.
    ///
    /// Two sources, and they behave identically once resolved: the prelude,
    /// and a name an `import` bound to a native package's export. Both are
    /// fixed before a program runs, which is what makes folding them sound.
    ///
    /// Declines when the module shadows the name — a top-level
    /// `local String = {…}` must not resolve to the stdlib's, which is a bug
    /// this compiler has shipped once already.
    pub fn static_value(&self, id: NodeId, name: &str) -> Option<Value> {
        if !self.not_shadowed(name) {
            return None;
        }
        if let Some(v) = self.native_imports.get(name) {
            return Some(v.clone());
        }
        if matches!(self.binding(id), Some(Binding::Prelude { .. })) {
            return self.prelude_value(name);
        }
        None
    }


    /// Whether a `CALLK` to top-level `fn` `name` is sound *at this point*.
    ///
    /// Inside any function body the answer is always yes: the body cannot
    /// run before the module body has finished defining every top-level
    /// `fn`, which is exactly why `fn_protos` is pre-collected and why
    /// `fn a() b() end` above `fn b()` is ordinary Saule.
    ///
    /// The module body itself is the one place that reasoning fails, and it
    /// failed silently. `local r = later(5)` above `fn later` errors under
    /// the tree-walker — the resolver never binds `later`, because at that
    /// point it does not exist — and returned `105` under the VM, because
    /// `fn_protos` knows the proto regardless of position. Right exit
    /// status, invented value.
    ///
    /// Returning `false` makes the call site refuse, which hands the whole
    /// module to the tree-walker. That is the right trade twice over: the
    /// program is one the language rejects, so nothing correct is lost, and
    /// the tree-walker is the engine that *defines* the diagnostic, so the
    /// two agree by construction rather than by matching message text.
    /// Whether every top-level `fn`/`class`/`enum` is declared by now.
    ///
    /// Once it is, a call the module body makes cannot reach a *callable*
    /// that does not exist yet, and the conservative guard switches off.
    ///
    /// Module-level `local`s are not counted here — see `module_type_decls`
    /// — so a callee reaching a `local` declared further down is the one
    /// shape this does not cover. It is narrow (the callee must be invoked
    /// from the module body *and* read a value declared below that call),
    /// and the direct case is caught precisely by the read guard in
    /// `ident_to`, which needs no approximation at all.
    pub fn module_callables_declared(&self) -> bool {
        self.module_type_decls
            .iter()
            .all(|n| self.module_decls_seen.contains(n))
    }

    pub fn callk_resolvable(&self, name: &str) -> bool {
        !self.enclosing.is_empty() || self.module_decls_seen.contains(name)
    }

    /// Whether calling `name` from the **module body** could reach a
    /// top-level declaration the body has not run yet.
    ///
    /// A breadth-first closure over [`Self::module_refs`]. Inside a function
    /// body it is vacuously false: by the time one runs, the module body has
    /// finished and every top-level name exists.
    ///
    /// Deliberately one-sided. It over-approximates — a name mentioned on a
    /// branch that never executes still counts — and over-approximating here
    /// costs a fallback, while under-approximating costs a wrong answer on a
    /// program the tree-walker rejects outright.
    pub fn reaches_undeclared(&self, name: &str) -> bool {
        if !self.enclosing.is_empty() {
            return false;
        }
        let mut queue = vec![name.to_string()];
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        while let Some(cur) = queue.pop() {
            if !seen.insert(cur.clone()) {
                continue;
            }
            let Some(refs) = self.module_refs.get(&cur) else {
                continue;
            };
            for r in refs {
                if self.module_type_decls.contains(r) && !self.module_decls_seen.contains(r) {
                    return true;
                }
                queue.push(r.clone());
            }
        }
        false
    }

    /// A `CALLK` target: the proto index packed with its module.
    ///
    /// A proto index only means something inside its own chunk, and
    /// `self.super()` on a parent from another module is a `CALLK` across
    /// that boundary — without the module it loaded the *running* module's
    /// proto of the same number, which for a subclass's `init` was the
    /// subclass's own `init`. Unbounded recursion, and only because the two
    /// happened to be numbered alike.
    ///
    /// `EXTRAARG` carries 24 bits, split 8/16: 256 modules and 65 536 protos
    /// each, both refused cleanly rather than wrapped.
    pub fn call_target(
        &self,
        module: usize,
        proto: u32,
        span: &Range<usize>,
    ) -> Result<u32, CompileError> {
        if module > 0xFF {
            return Err(CompileError::unsupported("a program with over 256 modules", span.clone()));
        }
        if proto > 0xFFFF {
            return Err(CompileError::unsupported(
                "a module with over 65536 functions",
                span.clone(),
            ));
        }
        Ok(((module as u32) << 16) | proto)
    }

    /// [`Self::call_target`] for a proto of the module being compiled.
    pub fn own_call_target(&self, proto: u32, span: &Range<usize>) -> Result<u32, CompileError> {
        self.call_target(self.chunk.module_index, proto, span)
    }


    /// The static `field` as seen from inside class `class`, by name.
    ///
    /// `sindex` is flattened and each entry names its **declaring** class,
    /// so an inherited static resolves to the parent's cell rather than to a
    /// second, never-initialised one in the subclass — the bug §24.2 warns
    /// about, expressed as data.
    pub fn static_slot_of(&self, class: &str, field: &str) -> Option<crate::chunk::StaticSlot> {
        let idx = self
            .layouts
            .get(class)
            // The resolver leaves the class name empty for a static reached
            // from somewhere it could not attribute; fall back to the class
            // whose body we are compiling.
            .or(self.f.current_class)?;
        self.chunk.classes[idx as usize].sindex.get(field).copied()
    }

    /// The `static fn` `method` on class `class`, as `(class index, slot)`.
    ///
    /// The companion to [`Self::static_slot_of`] for the *method* half:
    /// `smindex` and `sindex` are separate tables, and a bare name inside a
    /// class body may be either.
    pub fn static_method_of(&self, class: &str, method: &str) -> Option<(u32, u16)> {
        let idx = self.layouts.get(class).or(self.f.current_class)?;
        let s = *self.chunk.classes[idx as usize].smindex.get(method)?;
        // `s.class`, not `idx`: an inherited `static fn` lives in the proto
        // vector of the class that declared it.
        Some((s.class, s.slot))
    }


    /// This module's slot for `name`, by name.
    ///
    /// For the places that have a name but no `NodeId` to ask the binding
    /// table with — a pipeline stage, for one, whose `PipeStage` holds a
    /// bare `String`.
    pub fn module_slot_of(&self, name: &str) -> Option<u16> {
        self.bindings
            .module_slots
            .iter()
            .position(|s| s.as_ref() == name)
            .and_then(|i| u16::try_from(i).ok())
    }

    /// A module slot as the **program** numbers it.
    ///
    /// The resolver numbers each module's slots from zero; the program lays
    /// those spaces end to end. Rebasing here rather than at run time is
    /// what lets an import be an ordinary `GETMOD` from the exporter's slot
    /// followed by a `SETMOD` into the importer's — two indices into one
    /// vector, no new opcode and no name lookup.
    pub fn mod_slot(&self, slot: u16, span: &Range<usize>) -> Result<u16, CompileError> {
        u16::try_from(self.module_slot_base + slot as usize).map_err(|_| {
            // `Bx` is 16 bits. A program with more than 65 536 top-level
            // bindings across all its modules is refused rather than
            // silently wrapped into another module's slot.
            CompileError::unsupported("a program with over 65536 top-level names", span.clone())
        })
    }

    // ---- what the front end proved -------------------------------------

    /// What `name` refers to at `id`, as `saule-semantic` resolved it.
    pub fn binding(&self, id: NodeId) -> Option<&Binding> {
        self.bindings.get(id)
    }

    /// The type `saule-typeck` proved for a node, if any. `None` is always
    /// safe: it selects a dynamic opcode rather than a wrong one.
    pub fn ty_name(&self, id: NodeId) -> Option<&str> {
        match self.types.get(&id)? {
            saule_ast::Type::Named(n) => Some(n.as_str()),
            _ => None,
        }
    }

}
