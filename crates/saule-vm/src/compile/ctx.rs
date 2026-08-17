//! The compiler's own state: emission, constants, labels, and the scope map
//! that turns a name into a register.
//!
//! ## Who owns register numbers
//!
//! `saule-semantic` already assigns every local a slot (Phase 0.6), and it is
//! tempting to just use those. This compiler deliberately does not. The two
//! allocators would have to agree *exactly and forever* — including about
//! compiler-introduced registers the resolver never sees, like the three
//! control registers a numeric `for` needs — and a silent disagreement would
//! mean reading the wrong register, which produces a wrong answer rather
//! than a crash.
//!
//! So the split is:
//!
//! * **the resolver says _what_** — is this name a local, a module slot, an
//!   upvalue, a class static, the prelude; and for a closure, exactly which
//!   names it captures and in what order;
//! * **the compiler says _where_** — which register, via [`RegAlloc`].
//!
//! Module slots are the exception: those *are* the resolver's numbering,
//! because they are part of the module's interface to its importers, and
//! nothing else assigns them.

use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use saule_ast::NodeId;
use saule_interpreter::Value;
use saule_semantic::{Binding, Bindings};
use saule_typeck::TypeTable;

use crate::chunk::{Chunk, LineEntry, Proto, UpvalDesc};
use crate::compile::CompileError;
use crate::compile::regalloc::{Mark, RegAlloc};
use crate::op::{Instruction, Op};

/// One imported value: where it lives now, and which slot it must land in.
#[derive(Debug, Clone, Copy)]
pub struct ImportBinding {
    /// Slot in *this* module, as the resolver numbered it — rebased when
    /// emitted.
    pub local: u16,
    /// Slot in the exporting module, already program-global.
    pub from: u16,
}

/// How a callee is named, for [`Compiler::callee_params`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CalleeKey {
    /// A top-level `fn` of this module.
    Function(String),
    /// A method or `init` of a class, by class index and method name.
    Method(crate::chunk::ClassIdx, String),
}

/// A patch site: an emitted jump whose target is not known yet.
#[must_use = "an unpatched jump goes to the wrong place"]
#[derive(Debug, Clone, Copy)]
pub struct Label(usize);

/// One step of the capture walk. `stack.last()` is the function doing the
/// capturing; level 0 is the module body, whose block-scoped locals are
/// capturable like any other.
fn capture_in(stack: &mut [FuncCtx], level: usize, name: &str) -> Option<u16> {
    if level == 0 {
        return None;
    }
    let parent = level - 1;
    let desc = match stack[parent].lookup(name) {
        Some(slot) => {
            // The owning frame must close this register when its block ends,
            // or the next loop iteration would overwrite a value the closure
            // still points at (§7.2).
            stack[parent].regs.note_capture();
            UpvalDesc {
                from_parent_stack: true,
                index: slot as u8,
                name: Rc::from(name),
            }
        }
        None => UpvalDesc {
            from_parent_stack: false,
            index: capture_in(stack, parent, name)? as u8,
            name: Rc::from(name),
        },
    };
    Some(stack[level].upvalue(name, desc))
}

/// Whether a code array already ends in a return, so `pop_function` does not
/// append a second, unreachable one.
fn ends_in_return(code: &[Instruction]) -> bool {
    matches!(
        code.last().and_then(|i| i.op()),
        Some(Op::RET | Op::RET0 | Op::RET1)
    )
}

/// One function being compiled.
pub struct FuncCtx {
    pub name: Option<Rc<str>>,
    pub code: Vec<Instruction>,
    pub regs: RegAlloc,
    pub lines: Vec<LineEntry>,
    pub handlers: Vec<crate::chunk::Handler>,
    pub n_params: u8,
    /// Chunk proto indices this function's `CLOSURE` instructions refer to.
    ///
    /// `CLOSURE Bx` indexes the *enclosing proto's* nested list rather than
    /// the chunk directly, so that a proto is self-contained — which is what
    /// keeps a serialized chunk relocatable if the bytecode cache of §14
    /// ever lands.
    pub nested: Vec<u32>,
    /// Upvalue descriptors for this function, in index order.
    ///
    /// Rebuilt here rather than copied from `saule-semantic`: the resolver
    /// says *which names* a closure captures and in what order, but its slot
    /// numbers are its own. Registers are this compiler's to assign, so the
    /// descriptor's `index` has to come from the enclosing `FuncCtx`.
    pub upvals: Vec<UpvalDesc>,
    /// The class whose method this is, if any — what `self` denotes.
    pub current_class: Option<crate::chunk::ClassIdx>,
    /// Whether `self` is bound (register 0). False in a static method.
    pub in_method: bool,
    /// Per-arity entry points for defaulted parameters (§19), built by
    /// [`Compiler::param_entries`] before the body is compiled.
    pub entries: Vec<u32>,
    /// The variadic parameter's name, when this function has one.
    ///
    /// `VARARG` binds it to a table, but the front end types it as the
    /// *element* type — `...values: integer` makes `values` an `integer` as
    /// far as the `TypeTable` is concerned. So `for v in values` cannot be
    /// proved a table from the type alone, and this records what the
    /// compiler already knows.
    pub variadic_param: Option<Rc<str>>,
    /// Lexical scopes, innermost last: `name -> register`. The compiler's
    /// own map, authoritative for register numbers.
    scopes: Vec<Vec<(Rc<str>, u16)>>,
}

impl FuncCtx {
    pub fn new(name: Option<&str>) -> FuncCtx {
        FuncCtx {
            name: name.map(Rc::from),
            code: Vec::new(),
            regs: RegAlloc::new(),
            lines: Vec::new(),
            handlers: Vec::new(),
            n_params: 0,
            nested: Vec::new(),
            upvals: Vec::new(),
            current_class: None,
            in_method: false,
            entries: Vec::new(),
            variadic_param: None,
            scopes: vec![Vec::new()],
        }
    }

    /// Record a nested proto and return the index `CLOSURE` should carry.
    pub fn nested_index(&mut self, chunk_idx: u32) -> u16 {
        match self.nested.iter().position(|i| *i == chunk_idx) {
            Some(i) => i as u16,
            None => {
                self.nested.push(chunk_idx);
                (self.nested.len() - 1) as u16
            }
        }
    }

    pub fn declare(&mut self, name: &str, reg: u16) {
        self.scopes
            .last_mut()
            .expect("scope")
            .push((Rc::from(name), reg));
    }

    /// Innermost binding of `name`, or `None` if this function has no local
    /// by that name.
    pub fn lookup(&self, name: &str) -> Option<u16> {
        self.scopes
            .iter()
            .rev()
            .find_map(|s| s.iter().rev().find(|(n, _)| n.as_ref() == name))
            .map(|(_, r)| *r)
    }

    pub fn enter_scope(&mut self) {
        self.scopes.push(Vec::new());
        self.regs.enter_block();
    }

    /// Leave a scope. Returns the register to `CLOSEUP` at, if anything in
    /// the scope was captured.
    pub fn leave_scope(&mut self) -> Option<u16> {
        self.scopes.pop();
        self.regs.leave_block()
    }

    pub fn label_here(&self) -> usize {
        self.code.len()
    }

    /// Index of `name` in this function's upvalue list, adding it if new.
    fn upvalue(&mut self, name: &str, desc: UpvalDesc) -> u16 {
        match self.upvals.iter().position(|u| u.name.as_ref() == name) {
            Some(i) => i as u16,
            None => {
                self.upvals.push(desc);
                (self.upvals.len() - 1) as u16
            }
        }
    }

    pub fn upvalue_index(&self, name: &str) -> Option<u16> {
        self.upvals
            .iter()
            .position(|u| u.name.as_ref() == name)
            .map(|i| i as u16)
    }
}

/// Where `break` and `continue` jump to.
///
/// A stack, because loops nest and each `break` belongs to the innermost
/// one. `continue` targets differ per loop kind — a `while` re-tests its
/// condition, a numeric `for` runs its `FORLOOP` — so the target is recorded
/// rather than assumed.
#[derive(Default)]
pub struct LoopCtx {
    pub breaks: Vec<Label>,
    pub continues: Vec<Label>,
}

pub struct Compiler<'a> {
    pub chunk: Chunk,
    pub bindings: &'a Bindings,
    pub types: &'a TypeTable,
    pub f: FuncCtx,
    /// Enclosing functions, innermost last. A function body is compiled by
    /// pushing a fresh [`FuncCtx`], so nesting is a stack rather than a
    /// separate compiler instance.
    pub enclosing: Vec<FuncCtx>,
    /// Top-level `fn` name -> its index in [`Chunk::protos`].
    ///
    /// Filled by a pre-pass before any body is compiled, so a forward
    /// reference (`fn a() b() end` above `fn b()`) resolves — which is the
    /// same reason the resolver pre-collects module scope.
    pub fn_protos: HashMap<String, u32>,
    pub loops: Vec<LoopCtx>,
    /// Class name -> index, from Pass 1.
    pub layouts: crate::compile::layout::Layouts,
    /// Receiver names that appear on the **left** of an assignment.
    ///
    /// A stdlib value like `Math.pi` is resolved at compile time and frozen
    /// into the constant pool. That is only sound if nothing writes to it —
    /// and `Math.pi = 3.0` is accepted today, the typechecker does not
    /// reject it. So the compiler asks first, and declines to freeze a
    /// receiver this module assigns through.
    pub mutated_receivers: std::collections::HashSet<String>,
    /// Top-level names bound by a `local` — i.e. holding a *value*.
    ///
    /// A module-level `local` becomes a module **slot**, not a frame local,
    /// so `FuncCtx::lookup` cannot see it. Every "is this bare name really
    /// the class / enum / stdlib entity it looks like?" test used that
    /// lookup, and so answered yes for `local Foo = {...}` shadowing a class
    /// `Foo` — reading the class's static where the program meant its table.
    /// Nested locals are still `FuncCtx::lookup`'s job; this covers the one
    /// case it structurally cannot.
    pub shadowed_names: std::collections::HashSet<String>,
    /// Whether a program driver already resolved this module's imports.
    ///
    /// When it has, an `import` of a **type** emits nothing at all: the
    /// driver bound the name to a program-global `ClassIdx` before codegen
    /// started, so `Button(...)` is already a plain `NEW` and there is no
    /// runtime work left to do. When it has not — a single file compiled on
    /// its own — an `import` must still be refused, because the name has a
    /// module slot that nothing would ever write.
    pub imports_bound: bool,
    /// Where this module's slots start in the program's flat slot space.
    /// Added to every module-slot operand by [`Compiler::mod_slot`].
    pub module_slot_base: usize,
    /// Imported **values** to copy in before the module body runs.
    ///
    /// A type needs nothing here — it is a compile-time index. A `fn` or a
    /// module variable is a runtime value living in the exporting module's
    /// slot, and post-order guarantees that module has already run by the
    /// time this one starts.
    pub import_bindings: Vec<ImportBinding>,
    /// Names an `import` bound to a **native package's** exports.
    ///
    /// A native package is a bag of Rust-built values, not a Saule module:
    /// there is nothing to compile and nothing to run, and the export is
    /// fixed before the program starts. So it resolves at compile time,
    /// exactly like a prelude name — the same fold `print` and `Math.pi`
    /// already get.
    pub native_imports: HashMap<String, Value>,
    /// Declared parameters of every callee this module can name, for §19's
    /// compile-time argument binding.
    ///
    /// A `Proto` deliberately does not carry parameter *names* or defaults —
    /// those are compile-time facts and the runtime never needs them — so
    /// the call site reads them from here instead. Collected in one pre-pass
    /// before any body is compiled, because a method may call another
    /// declared further down the file.
    pub callee_params: HashMap<CalleeKey, Vec<saule_ast::Param>>,
    /// A prelude scope, consulted at *compile* time to turn `print` into the
    /// actual `NativeFn` value a `CALLNAT` constant points at.
    prelude: std::cell::RefCell<Option<std::rc::Rc<std::cell::RefCell<saule_interpreter::Environment>>>>,
}

impl<'a> Compiler<'a> {
    pub fn new(name: &str, source: &str, bindings: &'a Bindings, types: &'a TypeTable) -> Self {
        let mut chunk = Chunk::empty(name);
        chunk.source = Rc::new(miette::NamedSource::new(name, source.to_string()));
        chunk.module_slots = bindings.module_slots.len();
        Compiler {
            chunk,
            bindings,
            types,
            f: FuncCtx::new(Some("main")),
            enclosing: Vec::new(),
            fn_protos: HashMap::new(),
            loops: Vec::new(),
            layouts: Default::default(),
            mutated_receivers: Default::default(),
            shadowed_names: Default::default(),
            imports_bound: false,
            module_slot_base: 0,
            import_bindings: Vec::new(),
            native_imports: HashMap::new(),
            callee_params: HashMap::new(),
            prelude: std::cell::RefCell::new(None),
        }
    }

    /// Begin compiling a nested function body.
    pub fn push_function(&mut self, name: Option<&str>) {
        let outer = std::mem::replace(&mut self.f, FuncCtx::new(name));
        self.enclosing.push(outer);
    }

    /// Finish the current function body into a `Proto`, restoring the
    /// enclosing one.
    pub fn pop_function(&mut self, span: &Range<usize>) -> Proto {
        // Every proto must end in a return: falling off the end of the code
        // array is a VM error, and a Saule function that reaches its end
        // returns nothing.
        if !ends_in_return(&self.f.code) {
            self.emit(Instruction::abc(Op::RET0, 0, 0, 0), span);
        }
        let mut proto = Proto::new(
            self.f.name.as_deref(),
            self.f.n_params,
            self.f.regs.max_regs(),
            std::mem::take(&mut self.f.code),
        );
        proto.lines = std::mem::take(&mut self.f.lines);
        proto.handlers = std::mem::take(&mut self.f.handlers);
        proto.protos = std::mem::take(&mut self.f.nested);
        proto.upvals = std::mem::take(&mut self.f.upvals);
        proto.entries = std::mem::take(&mut self.f.entries);
        proto.source = Some(Rc::clone(&self.chunk.source));
        self.f = self.enclosing.pop().expect("push_function/pop_function pair");
        proto
    }

    /// Make `name` reachable from the function being compiled as an
    /// upvalue, returning its index.
    ///
    /// The Lua capture algorithm: walk out until the frame that owns the
    /// variable is found, then add a link to **every** function in between,
    /// so an inner closure reaches through the middle ones rather than past
    /// them (§7.1). The frame that owns it also has its current block marked
    /// captured, which is what makes `leave_scope` ask for a `CLOSEUP`.
    ///
    /// Returns `None` when no enclosing frame has the name, which for a
    /// resolved program means it is a module slot or the prelude.
    pub fn capture_upvalue(&mut self, name: &str) -> Option<u16> {
        // Treat the current function as the top of one stack so the walk is
        // uniform; restored before returning.
        self.enclosing
            .push(std::mem::replace(&mut self.f, FuncCtx::new(None)));
        let level = self.enclosing.len() - 1;
        let found = capture_in(&mut self.enclosing, level, name);
        self.f = self.enclosing.pop().expect("just pushed");
        found
    }

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

    // ---- emission ------------------------------------------------------

    /// Emit one instruction, recording the span it came from.
    ///
    /// The line table is built as we go and stays sorted by construction,
    /// because `pc` only ever increases. It is out of band — nothing in the
    /// instruction stream refers to it — so it costs nothing until an error
    /// needs a span (§12.3).
    pub fn emit(&mut self, ins: Instruction, span: &Range<usize>) {
        let pc = self.f.code.len() as u32;
        let entry = LineEntry {
            pc,
            span_start: span.start as u32,
            span_end: span.end as u32,
        };
        // Only record a change: consecutive instructions from one expression
        // share an entry, which is most of them.
        if self.f.lines.last().map(|l| (l.span_start, l.span_end)) != Some((entry.span_start, entry.span_end))
        {
            self.f.lines.push(entry);
        }
        self.f.code.push(ins);
    }

    /// Emit a jump whose target is patched later.
    pub fn emit_jump(&mut self, op: Op, a: u8, span: &Range<usize>) -> Label {
        let at = self.f.code.len();
        // A placeholder of 0 is harmless: `patch_to` overwrites the whole
        // word, and the verifier would catch an unpatched one.
        self.emit(Instruction::asbx(op, a, 0), span);
        Label(at)
    }

    /// Emit an `ABx` instruction whose `Bx` is a forward displacement,
    /// patched later. `ITERPREP` is the only one.
    pub fn emit_jump_abx(
        &mut self,
        op: Op,
        a: u8,
        span: &Range<usize>,
    ) -> Result<Label, CompileError> {
        let at = self.f.code.len();
        self.emit(Instruction::abx(op, a, 0), span);
        Ok(Label(at))
    }

    /// Point a previously emitted jump at the current position.
    pub fn patch_here(&mut self, label: Label) -> Result<(), CompileError> {
        let target = self.f.code.len();
        self.patch_to(label, target)
    }

    pub fn patch_to(&mut self, label: Label, target: usize) -> Result<(), CompileError> {
        let from = label.0;
        // A jump's displacement is relative to the instruction *after* it,
        // because the dispatch loop has already advanced `pc` when it
        // applies the offset.
        let disp = target as i64 - (from as i64 + 1);
        let ins = self.f.code[from];
        let op = ins.op().expect("emitted opcode");
        // `ITERPREP` carries an unsigned forward displacement in `Bx`; every
        // other patch site is a signed `sBx`.
        if op == Op::ITERPREP {
            let d = u16::try_from(disp).map_err(|_| CompileError::JumpTooFar {
                distance: disp,
                span: self.span_of_pc(from),
            })?;
            self.f.code[from] = Instruction::abx(op, ins.a(), d);
            return Ok(());
        }
        let patched = Instruction::try_asbx(op, ins.a(), disp as i32).ok_or_else(|| {
            CompileError::JumpTooFar {
                distance: disp,
                span: self.span_of_pc(from),
            }
        })?;
        self.f.code[from] = patched;
        Ok(())
    }

    /// Emit a backward jump to an already-known position.
    pub fn emit_jump_back(
        &mut self,
        op: Op,
        a: u8,
        target: usize,
        span: &Range<usize>,
    ) -> Result<(), CompileError> {
        let label = self.emit_jump(op, a, span);
        self.patch_to(label, target)
    }

    fn span_of_pc(&self, pc: usize) -> Range<usize> {
        match self.f.lines.binary_search_by_key(&(pc as u32), |l| l.pc) {
            Ok(i) => {
                let e = &self.f.lines[i];
                e.span_start as usize..e.span_end as usize
            }
            Err(0) => 0..0,
            Err(i) => {
                let e = &self.f.lines[i - 1];
                e.span_start as usize..e.span_end as usize
            }
        }
    }

    // ---- constants -----------------------------------------------------

    pub fn constant(&mut self, v: Value, span: &Range<usize>) -> Result<u16, CompileError> {
        let idx = self.chunk.add_constant(v);
        u16::try_from(idx).map_err(|_| CompileError::Unsupported {
            thing: "a module with more than 65536 constants",
            span: span.clone(),
        })
    }

    // ---- registers -----------------------------------------------------

    pub fn alloc(&mut self, span: &Range<usize>) -> Result<u16, CompileError> {
        let name = self.func_label();
        self.f.regs.alloc().map_err(|o| o.at(&name, span.clone()))
    }

    pub fn alloc_n(&mut self, n: u16, span: &Range<usize>) -> Result<u16, CompileError> {
        let name = self.func_label();
        self.f.regs.alloc_n(n).map_err(|o| o.at(&name, span.clone()))
    }

    pub fn mark(&self) -> Mark {
        self.f.regs.mark()
    }

    pub fn free_to(&mut self, m: Mark) {
        self.f.regs.free_to(m);
    }

    pub fn func_label(&self) -> String {
        self.f
            .name
            .as_deref()
            .unwrap_or("<lambda>")
            .to_string()
    }

    /// A register operand must fit in an 8-bit field.
    pub fn reg8(&self, r: u16, span: &Range<usize>) -> Result<u8, CompileError> {
        u8::try_from(r).map_err(|_| CompileError::TooManyRegisters {
            name: self.func_label(),
            needed: r as usize + 1,
            span: span.clone(),
        })
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

    /// Emit per-arity entry stubs for a callee's defaulted parameters
    /// (§19), and hand back the `entries` table.
    ///
    /// Call sites do nothing for a default: `CALLK` pushes a frame and jumps
    /// to `proto.entry_for(n_args)`, and the stub the arity lands on fills in
    /// what was not passed. Zero cost when every argument was supplied,
    /// because that entry point *is* the body.
    ///
    /// **Defaults are evaluated in the callee's frame**, which §19 calls the
    /// one genuine correctness trap here. Stubs get that right by
    /// construction: parameter `k`'s default compiles into register `k` of
    /// the callee, so a default that mentions an earlier parameter —
    /// `fn f(a: integer, b: integer = a * 2)` — reads the register holding
    /// `a`, exactly as `bind_params` reads the callee's scope. Compiling the
    /// default at the *call site* instead would have resolved `a` against
    /// the caller.
    ///
    /// The stubs fall through into one another, so entering at arity `k`
    /// fills `k`, `k+1`, … and then runs the body — one instruction stream,
    /// no jumps.
    /// `base` is the register (and argument index) the *first* written
    /// parameter occupies: 1 for an instance method, whose register 0 is
    /// `self` and whose `self` counts as an argument, and 0 otherwise.
    pub fn param_entries(
        &mut self,
        params: &[saule_ast::Param],
        base: u16,
        span: &Range<usize>,
    ) -> Result<Vec<u32>, CompileError> {
        // A variadic parameter gathers the surplus arguments into a table,
        // and it has to happen however the frame was entered — so it is the
        // proto's *first* instruction, before any default stub.
        if let Some(i) = params.iter().position(|p| p.variadic) {
            // `bind_params` rejects both of these at run time; refusing here
            // keeps the compiler from emitting code for a signature the
            // other engine would not accept.
            if i + 1 != params.len() || params.iter().filter(|p| p.variadic).count() > 1 {
                return Err(CompileError::unsupported(
                    "a variadic parameter that is not the last one",
                    span.clone(),
                ));
            }
            // Mixing the two would need an entry stub per arity that also
            // re-gathers, and nothing in the corpus does it.
            if params.iter().any(|p| p.default.is_some()) {
                return Err(CompileError::unsupported(
                    "a signature with both a default and a variadic parameter",
                    span.clone(),
                ));
            }
            let a = self.reg8(base + i as u16, span)?;
            self.emit(Instruction::abc(Op::VARARG, a, 0, 0), span);
            self.f.variadic_param = Some(Rc::from(params[i].name.as_str()));
            // No defaults, so every arity enters at 0 — which is the
            // `VARARG` just emitted.
            return Ok(Vec::new());
        }
        let Some(first_default) = params.iter().position(|p| p.default.is_some()) else {
            // Nothing defaulted: `entry_for` falls back to 0 for every
            // arity, which is the body.
            return Ok(Vec::new());
        };
        let base = base as usize;
        let mut entries = vec![0u32; base + params.len() + 1];
        for (k, p) in params.iter().enumerate().skip(first_default) {
            entries[base + k] = self.f.label_here() as u32;
            // A parameter after the first defaulted one with no default of
            // its own keeps whatever `push_frame` left — `nil` — which is
            // what a nullable parameter should get anyway (§19 step 5).
            if let Some(d) = &p.default {
                self.expr_to(d, (base + k) as u16)?;
            }
        }
        // Every arity at or below the first default enters the same place:
        // a call missing a *required* argument is a typecheck error, so this
        // only decides what an unchecked caller sees, and "fill every
        // default" is the sane answer.
        let first_pc = entries[base + first_default];
        for e in entries.iter_mut().take(base + first_default) {
            *e = first_pc;
        }
        // Full arity: straight to the body.
        entries[base + params.len()] = self.f.label_here() as u32;
        let _ = span;
        Ok(entries)
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

    /// Finish the module body and hand back the chunk.
    pub fn finish(mut self, result: Option<u16>, span: &Range<usize>) -> Result<Chunk, CompileError> {
        match result {
            Some(r) => {
                let a = self.reg8(r, span)?;
                self.emit(Instruction::abc(Op::RET1, a, 0, 0), span);
            }
            None => self.emit(Instruction::abc(Op::RET0, 0, 0, 0), span),
        }
        let mut proto = Proto::new(
            self.f.name.as_deref(),
            self.f.n_params,
            self.f.regs.max_regs(),
            std::mem::take(&mut self.f.code),
        );
        proto.lines = std::mem::take(&mut self.f.lines);
        proto.handlers = std::mem::take(&mut self.f.handlers);
        proto.protos = std::mem::take(&mut self.f.nested);
        proto.source = Some(Rc::clone(&self.chunk.source));
        let mut chunk = self.chunk;
        chunk.main = chunk.add_proto(proto);
        Ok(chunk)
    }
}

/// Numeric kind of an operand, as proved by the typechecker. `None` means
/// the compiler must fall back to a dynamic form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Num {
    Int,
    Float,
}

pub fn num_of(name: &str) -> Option<Num> {
    match name {
        "integer" => Some(Num::Int),
        "float" => Some(Num::Float),
        _ => None,
    }
}

/// Names the compiler needs to intern as constants alongside their values.
pub type ConstMap = HashMap<String, u16>;
