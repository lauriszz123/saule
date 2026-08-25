//! One function being compiled: its code, its scopes, and its upvalues.
//!
//! A [`FuncCtx`] is pushed when the compiler enters a `fn` or a lambda and
//! popped into a [`Proto`] when it leaves. The stack of them is what makes
//! capture work: an inner function reaches an outer local by walking down
//! it, and every frame it passes through records the upvalue on the way
//! back up.

use std::ops::Range;
use std::rc::Rc;

use crate::chunk::{LineEntry, Proto, UpvalDesc};
use crate::compile::ctx::regalloc::RegAlloc;
use crate::op::{Instruction, Op};

use super::Compiler;

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
/// Whether the last instruction is a return.
///
/// **Not the same question as "can control fall off the end".** A forward
/// jump patched to `code.len()` reaches the position *after* the last
/// instruction, so a proto whose final instruction is a `RET1` can still be
/// entered one past it. `FuncCtx::end_is_a_jump_target` is what records
/// that; see `pop_function`.
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
    /// Chunk jump-table indices this function's `SWITCH` instructions own.
    ///
    /// The tables themselves live in the [`Chunk`](crate::chunk::Chunk),
    /// shared by every function in the module, and their entries are
    /// **absolute instruction indices into the proto that emitted them**.
    /// So the peephole cannot relocate "the jump tables" — it has to
    /// relocate this function's, and leave a sibling's numbering alone.
    /// Recorded here because the emitter is the only thing that knows which
    /// are which.
    pub jump_tables: Vec<u16>,
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
    /// The name this lambda is being bound to by the `local` that owns it,
    /// when it mentions that name itself.
    ///
    /// `local go = fn(k) … go(k - 1) … end`. The name is not a capturable
    /// local of the enclosing frame — the register is declared *after* the
    /// initializer is compiled — and capturing it would leak, so a reference
    /// to it compiles to `SELFFUNC` instead. Only the lambda directly bound
    /// by the `local` carries it; a deeper nested lambda still refuses.
    pub self_fn_name: Option<Rc<str>>,
    /// The furthest instruction index any jump in this function has been
    /// patched to.
    ///
    /// `pop_function` appends a `RET0` when control can reach the end of the
    /// code array, and "the last instruction is a return" is not that test:
    /// a forward jump may target `code.len()`, one *past* the last
    /// instruction. While every `if` arm ended in an unconditional jump the
    /// two questions happened to coincide, which is why a single `RET1` in
    /// the last arm of `Main.loadFile` was enough to make the VM run off the
    /// end of the proto the moment those jumps stopped being emitted.
    pub max_patch_target: usize,
    /// How many `try` **bodies** enclose the statement being compiled.
    ///
    /// A tail call inside one must not replace the frame: the handler has to
    /// still be on the stack when the callee runs, or `try return f() catch`
    /// stops catching what `f` throws. `exec_try` forces the tree-walker's
    /// `Flow::TailCall` into a real call for exactly this reason, and the two
    /// engines have to draw the line in the same place — the depth at which
    /// a program dies is observable.
    ///
    /// Per function, not per compiler: a lambda written inside a `try` body
    /// gets a frame of its own, and its own `return` is in no handler's way.
    pub try_depth: u32,
    /// Lexical scopes, innermost last: `name -> register`. The compiler's
    /// own map, authoritative for register numbers.
    scopes: Vec<Vec<(Rc<str>, u16)>>,
}

impl FuncCtx {
    pub fn new(name: Option<&str>) -> FuncCtx {
        FuncCtx {
            name: name.map(Rc::from),
            try_depth: 0,
            code: Vec::new(),
            regs: RegAlloc::new(),
            lines: Vec::new(),
            handlers: Vec::new(),
            n_params: 0,
            nested: Vec::new(),
            jump_tables: Vec::new(),
            upvals: Vec::new(),
            current_class: None,
            in_method: false,
            entries: Vec::new(),
            variadic_param: None,
            self_fn_name: None,
            max_patch_target: 0,
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

impl Compiler<'_> {

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
        // `>=` and not `>`: a target equal to `code.len()` is exactly the
        // fall-off-the-end case.
        let reachable_end = self.f.max_patch_target >= self.f.code.len();
        if reachable_end || !ends_in_return(&self.f.code) {
            self.emit(Instruction::abc(Op::RET0, 0, 0, 0), span);
        }
        // Pass 3.5, and **after** the terminator above rather than before
        // it: the peephole may delete the word a jump lands on the far side
        // of, so `max_patch_target` stops meaning anything the moment it
        // runs. Deleting a dead `MOVE` cannot make a body fall off its own
        // end, so the invariant this establishes survives the pass.
        let jump_tables = std::mem::take(&mut self.f.jump_tables);
        crate::compile::peephole::run(
            &mut self.f.code,
            &mut self.f.lines,
            &mut self.f.handlers,
            &mut self.f.entries,
            &mut self.chunk.jump_tables,
            &jump_tables,
        );
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


    pub fn func_label(&self) -> String {
        self.f
            .name
            .as_deref()
            .unwrap_or("<lambda>")
            .to_string()
    }

}
