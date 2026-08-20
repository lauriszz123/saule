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

//!
//! ## What lives where
//!
//! | file | holds |
//! |---|---|
//! | `mod.rs` | the [`Compiler`] struct itself, its construction, and `finish` |
//! | [`func`] | one function being compiled: scopes, locals, upvalue capture |
//! | [`emit`] | writing instructions, jumps, labels, patches, constants |
//! | [`regalloc`] | which register a value goes in (§18) |
//! | [`operand`] | is this operand pure, and can it be read in place |
//! | [`resolve`] | turning a name into a slot, a static, a callee, a type |
//! | [`coerce`] | binding arguments to parameters and to declared types (§19) |

pub mod coerce;
pub mod emit;
pub mod func;
pub mod operand;
pub mod regalloc;
pub mod resolve;

pub use emit::Label;
pub use func::FuncCtx;

use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use saule_interpreter::Value;
use saule_semantic::Bindings;
use saule_typeck::TypeTable;

use crate::chunk::{Chunk, Proto};
use crate::compile::CompileError;
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
    /// Module-scope names whose declaration the **module body** has
    /// already passed, and how many there are in total.
    ///
    /// `fn_protos` above is deliberately filled before any body is
    /// compiled, and that is right *inside* a function: by the time one
    /// runs, every top-level `fn` exists. The module body is the exception,
    /// because it executes straight-line — a call written above the
    /// declaration reaches a name that does not exist yet. The resolver
    /// agrees and leaves it unbound, so the tree-walker errors, while a
    /// `CALLK` resolved from `fn_protos` alone would cheerfully jump to the
    /// proto and return a value. See [`Compiler::callk_resolvable`].
    pub module_decls_seen: std::collections::HashSet<String>,
    /// Top-level `fn`/`class`/`interface`/`enum` names, from the pre-pass.
    ///
    /// Only these count toward the call guard. A module-level `local` is
    /// deliberately excluded: `local doubled = when(...)` declares a name
    /// *as* it makes a call, so counting locals left the module body never
    /// "fully declared" and refused every call in a file that ends in a
    /// declaration — which is most of them.
    pub module_type_decls: std::collections::HashSet<String>,
    /// Top-level declaration name → the top-level names its body mentions.
    ///
    /// The direct guards above are exact but *local*: they fire when the
    /// module body itself names something undeclared. They cannot see one
    /// level down — `C.go()` called above `fn later`, where `go`'s body
    /// reads `later`. The reference inside `go` is legal; only the call is
    /// early, and nothing at the call site distinguishes a callee that
    /// reaches an undeclared name from one that does not.
    ///
    /// A blunter guard was tried first — refuse any module-body call while
    /// a `fn` is still ahead — and reverted, because a call partway down a
    /// file with any `fn` below it is an ordinary shape and it refused two
    /// perfectly good programs. This is the precise version: one edge per
    /// mention, closed transitively at the call site.
    pub module_refs: HashMap<String, std::collections::HashSet<String>>,
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
    /// The name a `local NAME = fn …` is binding, live only while that
    /// lambda's initializer is being compiled. `lambda_to` takes it into the
    /// new `FuncCtx`'s `self_fn_name`, so it reaches exactly the one lambda
    /// the `local` names and no deeper one.
    pub binding_lambda_to: Option<Rc<str>>,
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
            module_decls_seen: std::collections::HashSet::new(),
            module_type_decls: std::collections::HashSet::new(),
            module_refs: HashMap::new(),
            loops: Vec::new(),
            layouts: Default::default(),
            mutated_receivers: Default::default(),
            shadowed_names: Default::default(),
            imports_bound: false,
            binding_lambda_to: None,
            module_slot_base: 0,
            import_bindings: Vec::new(),
            native_imports: HashMap::new(),
            callee_params: HashMap::new(),
            prelude: std::cell::RefCell::new(None),
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
