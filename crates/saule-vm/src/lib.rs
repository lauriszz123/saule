//! Saule's register-based bytecode compiler and virtual machine.
//!
//! The full design is `VM_DESIGN.md` at the repository root; the phased task
//! list is `VM_TASKS.md`. This crate is the second execution engine for the
//! language — `saule-interpreter`'s tree-walker is the first — and the two
//! are meant to coexist for at least a full release cycle.
//!
//! ## How the two engines relate
//!
//! ```text
//!   saule-parser ─► saule-semantic ─► saule-typeck ─┬─► saule-interpreter  (tree-walker)
//!                                                   └─► saule-vm           (compile → execute)
//!                        shared front end                two engines
//! ```
//!
//! They are **independent engines over a shared runtime**. Concretely:
//!
//! * `saule-vm` depends on `saule-interpreter`; the reverse arrow does not
//!   exist and must never be added (§22.1). Delete this crate from the
//!   workspace and the tree-walker still builds and still passes its tests.
//! * What is shared is the *runtime*, not the *execution*: [`Value`], the
//!   table implementation, `RuntimeError`, and all ~3500 lines of stdlib and
//!   native-package machinery. Reimplementing those would mean two stdlibs
//!   to keep in sync, which is a far worse kind of coupling than a
//!   dependency edge (§3.7, §24.7 Q1).
//! * Nothing in `saule-interpreter` calls into this crate, and nothing here
//!   calls the tree-walker's `eval`. Each engine executes a program on its
//!   own, start to finish.
//! * `saule-lsp` and `saule-db` never execute Saule code, so they are
//!   unaffected by any of this (§14).
//!
//! The one change this crate required in `saule-interpreter` is additive: a
//! `VmFunction` trait plus an opaque `Value::VmFunction` variant, so a
//! compiled closure can sit in a register. The tree-walker never constructs
//! that variant and never calls one.
//!
//! ## Status
//!
//! The engine is complete: it compiles and runs the language, and every
//! program in `tests/` produces the same output under both engines. What
//! remains is coverage rather than construction — a construct the compiler
//! has not been taught yet reports [`CompileError::Unsupported`] and the
//! CLI falls back to the tree-walker, so a gap is a slower run rather than
//! a failure (§21.3).
//!
//! ## Where things are
//!
//! ```text
//!   lib.rs        run / run_chunk / run_program / disassemble — the ways in
//!   op.rs         the instruction set: opcodes, operand layouts, encoding
//!   chunk/        one compiled module: protos, classes, enums, pools
//!   compile/      AST → chunk, in four passes (§17)
//!   vm/           the register machine that executes one (§5.3, §6)
//!   program.rs    resolving an import graph into a runnable set of chunks
//!   disasm.rs     reading a chunk back as text
//!   profile.rs    opt-in bytecode profiling (§16), off in shipped binaries
//! ```

pub mod chunk;
pub mod compile;
pub mod disasm;
pub mod op;
pub mod profile;
pub mod program;
pub mod vm;

use std::rc::Rc;

use saule_ast::Module;

pub use chunk::{Chunk, Proto};
pub use compile::{CompileError, compile};
pub use op::{Instruction, Op};
pub use vm::Vm;

use saule_interpreter::{RuntimeError, Value};

/// Anything that can go wrong between an AST and a value, under this engine.
#[derive(Debug, thiserror::Error, miette::Diagnostic)]
pub enum EngineError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    Compile(#[from] CompileError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Runtime(#[from] RuntimeError),
}

/// Compile and run a module, returning its first result value.
///
/// Mirrors [`saule_interpreter::run`] so a caller can switch engines by
/// switching function, which is what the `--vm` flag will do in Phase 2.
/// Assumes the caller has already run `saule_semantic::analyze` and
/// `saule_typeck::check`.
pub fn run(module: &Module, name: &str, source: &str) -> Result<Value, EngineError> {
    saule_interpreter::init();
    let chunk = compile(module, name, source)?;
    let vs = run_chunk(Rc::new(chunk))?;
    Ok(vs.into_iter().next().unwrap_or(Value::Nil))
}

/// Execute an already-compiled chunk, returning everything its `main`
/// returned. The entry point hand-assembled chunks and the future bytecode
/// cache both use.
pub fn run_chunk(chunk: Rc<Chunk>) -> Result<Vec<Value>, RuntimeError> {
    saule_interpreter::init();
    Vm::new(chunk).run()
}

/// Execute a chunk and then its entry point, if it has one.
///
/// Running the module body only *declares* `class Main`; the program starts
/// when `Main.main()` is called. The tree-walker's driver does the same
/// thing through `call_class_static_method`, and both engines have to agree
/// about what "running a project" means.
///
/// Returns whether an entry point was found, so a caller that requires one
/// can report its absence.
pub fn run_chunk_entry(chunk: Rc<Chunk>) -> Result<bool, RuntimeError> {
    run_program(program::Program { modules: vec![chunk], entry: 0 })
}

/// Execute a whole program: every module's top level in post-order, then the
/// entry point.
///
/// Post-order matters and is observable: the tree-walker runs an imported
/// module's top level on first import, so a module that prints at the top
/// level prints before the module that imported it. Running them in any
/// other order would be a visible divergence.
pub fn run_program(program: program::Program) -> Result<bool, RuntimeError> {
    saule_interpreter::init();
    let entry = program.entry;
    let mut vm = Vm::for_chunks(program.modules);
    for i in 0..=entry {
        vm.run_module(i)?;
    }
    match vm.call_static("Main", "main") {
        Some(r) => r.map(|_| true),
        None => Ok(false),
    }
}

/// Compile a module and return the disassembly instead of running it — the
/// body of the future `saule disasm <file>` subcommand.
pub fn disassemble(module: &Module, name: &str, source: &str) -> Result<String, CompileError> {
    let chunk = compile(module, name, source)?;
    Ok(disasm::chunk(&chunk))
}
