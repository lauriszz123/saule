//! One compiled function body, and the per-function tables that describe it.

use std::cell::RefCell;
use std::rc::Rc;

use saule_interpreter::value::ClassObject;

use crate::op::Instruction;

use super::{ClassIdx, ProtoIdx, TypeIdx};

/// A compiled function body: the direct replacement for `FunctionObject`'s
/// `FunctionBody::Block(Arc<[Spanned<Stmt>]>)`.
#[derive(Debug)]
pub struct Proto {
    pub name: Option<Rc<str>>,
    pub n_params: u8,
    pub is_variadic: bool,
    /// Frame size: the register high-water mark recorded by §18's allocator.
    pub max_regs: u8,
    pub code: Vec<Instruction>,
    pub upvals: Vec<UpvalDesc>,
    /// Nested closures, as indices into [`Chunk::protos`]. `CLOSURE Bx`
    /// indexes *this* vector, not the chunk's.
    pub protos: Vec<ProtoIdx>,
    /// `try`/`catch` ranges, sorted by `pc_start` so unwinding binary-searches.
    pub handlers: Vec<Handler>,
    /// `pc -> span`, sorted by `pc`. Out of band: it never touches the
    /// instruction stream, so it costs nothing until something fails (§12.3).
    pub lines: Vec<LineEntry>,
    /// Per-call-site inline caches (§8.5). Runtime scratch, not serialized.
    pub caches: RefCell<Vec<InlineCache>>,
    pub owner_class: Option<ClassIdx>,
    /// Per-arity entry points for callees with defaulted parameters (§19).
    /// `entries[n]` is the pc to start at when called with `n` arguments;
    /// empty means "always start at 0".
    pub entries: Vec<u32>,
    /// The module-relative source this proto was compiled from. Carried per
    /// proto so an error inside an imported module renders against the right
    /// file — the job `FunctionObject.source` does today.
    pub source: Option<Rc<miette::NamedSource<String>>>,
}

impl Proto {
    /// A proto with nothing in it but a name and a body.
    pub fn new(name: Option<&str>, n_params: u8, max_regs: u8, code: Vec<Instruction>) -> Proto {
        Proto {
            name: name.map(Rc::from),
            n_params,
            is_variadic: false,
            max_regs,
            code,
            upvals: Vec::new(),
            protos: Vec::new(),
            handlers: Vec::new(),
            lines: Vec::new(),
            caches: RefCell::new(Vec::new()),
            owner_class: None,
            entries: Vec::new(),
            source: None,
        }
    }

    /// Source span for a program counter, for diagnostics. Binary search
    /// over the line table; an empty table yields `0..0`, which renders as
    /// a spanless error rather than pointing at the wrong text.
    pub fn span_at(&self, pc: u32) -> std::ops::Range<usize> {
        if self.lines.is_empty() {
            return 0..0;
        }
        let i = match self.lines.binary_search_by_key(&pc, |e| e.pc) {
            Ok(i) => i,
            // `pc` sits between entries: the covering entry is the one before.
            Err(0) => return 0..0,
            Err(i) => i - 1,
        };
        let e = &self.lines[i];
        e.span_start as usize..e.span_end as usize
    }

    /// The pc to enter at for a call with `n_args` arguments (§19's entry
    /// stubs). Falls back to 0 when the proto has no defaulted parameters.
    pub fn entry_for(&self, n_args: u8) -> u32 {
        self.entries.get(n_args as usize).copied().unwrap_or(0)
    }

    /// Human-readable name for diagnostics and the disassembler.
    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or("<lambda>")
    }
}

/// Where a closure's upvalue comes from at the moment the closure is built.
#[derive(Debug, Clone)]
pub struct UpvalDesc {
    /// `true`: a register of the *parent* frame. `false`: an upvalue of the
    /// parent closure.
    pub from_parent_stack: bool,
    pub index: u8,
    /// Diagnostics only.
    pub name: Rc<str>,
}

/// A `try`/`catch` range (§12.1). The happy path costs zero instructions:
/// entering a `try` emits nothing, and only a `throw` consults this table.
#[derive(Debug, Clone)]
pub struct Handler {
    pub pc_start: u32,
    /// Exclusive.
    pub pc_end: u32,
    /// Catch-block entry.
    pub target: u32,
    /// Register the caught value lands in.
    pub err_reg: u8,
    /// Type the `catch` clause filters on.
    pub catch_ty: TypeIdx,
}

/// `pc -> source span`, sorted by `pc`.
#[derive(Debug, Clone, Copy)]
pub struct LineEntry {
    pub pc: u32,
    pub span_start: u32,
    pub span_end: u32,
}

/// A monomorphic call-site cache (§8.5).
///
/// The design sketch stores a `*const ClassObject`. This stores the `Rc`
/// instead: a raw pointer that outlives its class can be matched by a *new*
/// class allocated at the same address, which would silently read the wrong
/// field. Holding the `Rc` makes that impossible, and the hit path still
/// only does a pointer compare — `Rc::as_ptr` touches no refcount.
///
/// No invalidation is needed: Saule has no metatables and no runtime class
/// mutation, so a `(class, slot)` pair is permanently valid once observed.
#[derive(Debug, Clone, Default)]
pub enum InlineCache {
    #[default]
    Empty,
    Mono {
        class: Rc<ClassObject>,
        slot: u16,
    },
}

