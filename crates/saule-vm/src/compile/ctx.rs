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
