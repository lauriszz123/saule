//! Saule's register-based bytecode compiler and virtual machine.
//!
//! The full design is `VM_DESIGN.md` at the repository root; the phased task
//! list is `VM_TASKS.md`. This crate is how Saule programs run.
//!
//! ```text
//!   saule-parser ─► saule-semantic ─► saule-typeck ─► saule-vm (compile → execute)
//!                                                         │
//!                                                   saule-runtime
//!                                        (values, stdlib, native packages)
//! ```
//!
//! * `saule-vm` depends on `saule-runtime`; the reverse arrow does not exist
//!   and must never be added. The runtime names a compiled closure only as
//!   the opaque [`Value::VmFunction`], behind a trait this crate implements.
//! * `saule-lsp` and `saule-db` never execute Saule code, so they depend on
//!   the front end and the runtime's module resolution, not on this crate.
//!
//! A construct the compiler cannot handle is a [`CompileError`]: there is no
//! other engine to hand the program to. The only ones a program the checker
//! accepts can still reach are programs the language rejects that the
//! checker does not catch yet ([`CompileError::Rejected`]), and limits such
//! as a function with over 255 parameters.
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

use saule_runtime::{RuntimeError, Value};

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

/// Anything that can go wrong between a parsed module and a value: the
/// front end's two passes, then this engine's two.
#[derive(Debug, thiserror::Error, miette::Diagnostic)]
pub enum PipelineError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    Semantic(#[from] saule_semantic::SemanticError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Typeck(#[from] saule_typeck::TypeCheckError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Compile(#[from] CompileError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Runtime(#[from] RuntimeError),
}

impl From<saule_runtime::PipelineError> for PipelineError {
    fn from(e: saule_runtime::PipelineError) -> Self {
        match e {
            saule_runtime::PipelineError::Semantic(e) => PipelineError::Semantic(e),
            saule_runtime::PipelineError::Typeck(e) => PipelineError::Typeck(e),
            saule_runtime::PipelineError::Runtime(e) => PipelineError::Runtime(e),
        }
    }
}

impl From<EngineError> for PipelineError {
    fn from(e: EngineError) -> Self {
        match e {
            EngineError::Compile(e) => PipelineError::Compile(e),
            EngineError::Runtime(e) => PipelineError::Runtime(e),
        }
    }
}

/// The whole pipeline over one module with no file behind it: analyse,
/// check, compile, run. Returns the module body's value — the last
/// expression statement's, or `nil`.
///
/// The in-memory counterpart of what `saule run` does to a file, and what
/// the test suites and the playground use. A module with no file has no
/// directory to resolve an `import` against, so one is a compile error.
pub fn check_and_run(
    module: &mut Module,
    name: &str,
    source: &str,
) -> Result<Value, PipelineError> {
    saule_runtime::init();
    saule_runtime::analyze_and_check(module, saule_semantic::ModuleSeed::default())?;
    Ok(run(module, name, source)?)
}

/// Compile and run a module, returning its first result value.
///
/// Assumes the caller has already run `saule_semantic::analyze` and
/// `saule_typeck::check`; [`check_and_run`] is the version that does.
pub fn run(module: &Module, name: &str, source: &str) -> Result<Value, EngineError> {
    saule_runtime::init();
    let chunk = compile(module, name, source)?;
    let vs = run_chunk(Rc::new(chunk))?;
    Ok(vs.into_iter().next().unwrap_or(Value::Nil))
}

/// Execute an already-compiled chunk, returning everything its `main`
/// returned. The entry point hand-assembled chunks and the future bytecode
/// cache both use.
pub fn run_chunk(chunk: Rc<Chunk>) -> Result<Vec<Value>, RuntimeError> {
    saule_runtime::init();
    Vm::new(chunk).run()
}

/// Execute a chunk and then its entry point, if it has one.
///
/// Running the module body only *declares* `class Main`; the program starts
/// when `Main.main()` is called.
///
/// Returns whether an entry point was found, so a caller that requires one
/// can report its absence.
pub fn run_chunk_entry(chunk: Rc<Chunk>) -> Result<bool, RuntimeError> {
    run_program(program::Program {
        modules: vec![chunk],
        entry: 0,
        via: vec![None],
    })
}

/// Execute a whole program: every module's top level in post-order, then the
/// entry point.
///
/// Post-order matters and is observable: an imported module's top level
/// runs before the module that imported it, so a module that prints at the
/// top level prints first — the order the language has always had.
pub fn run_program(program: program::Program) -> Result<bool, RuntimeError> {
    saule_runtime::init();
    let entry = program.entry;
    // Lifted out before the chunks move into the VM. Compiling a dynamic
    // native package folds its exports from the manifest and deliberately
    // does *not* `dlopen` the library behind them, so the load happens here
    // — per module, immediately before that module's body runs, so a package
    // that fails to load fails at its `import`.
    let dynamic: Vec<Vec<(String, std::ops::Range<usize>)>> = program
        .modules
        .iter()
        .map(|c| c.dynamic_imports.clone())
        .collect();
    let sources: Vec<_> = program
        .modules
        .iter()
        .map(|c| Rc::clone(&c.source))
        .collect();
    debug_assert_eq!(
        entry + 1,
        program.modules.len(),
        "post-order puts the entry last"
    );
    let mut vm = Vm::for_chunks(program.modules);
    for (i, imports) in dynamic.iter().enumerate().take(entry + 1) {
        // A failure while an imported module's top level runs is reported
        // against the entry file's `import` that led there, the way a
        // compile-time problem in that module is.
        let at_import = |e: RuntimeError| match &program.via[i] {
            None => e,
            Some(span) => imported_failure(e, &sources[i], span.clone()),
        };
        for (package, span) in imports {
            saule_runtime::dynamic_packages::preload(package, span.clone()).map_err(at_import)?;
        }
        vm.run_module(i).map_err(at_import)?;
    }
    match vm.call_static("Main", "main") {
        Some(r) => r.map(|_| true),
        None => Ok(false),
    }
}

/// `e`, raised while the top level of the module `src` ran, as an
/// `ImportFailed` at the entry file's `import` on `span`.
fn imported_failure(
    e: RuntimeError,
    src: &miette::NamedSource<String>,
    span: std::ops::Range<usize>,
) -> RuntimeError {
    match e {
        // Already anchored in that module by the VM (`Vm::attribute`).
        RuntimeError::InModule {
            module_label,
            inner,
        } => RuntimeError::ImportFailed {
            module_label,
            import_span: span,
            inner,
        },
        // A `throw` stays itself so an enclosing `catch` can still see it,
        // and a failure further down is already anchored.
        e @ (RuntimeError::Thrown { .. } | RuntimeError::ImportFailed { .. }) => e,
        other => {
            let label = src.name().to_string();
            RuntimeError::ImportFailed {
                module_label: label.clone(),
                import_span: span,
                inner: Box::new(saule_runtime::error::ImportedDiagnostic::from_inner(
                    &other,
                    label,
                    src.inner().clone(),
                )),
            }
        }
    }
}

/// Compile a module and return the disassembly instead of running it — the
/// body of the future `saule disasm <file>` subcommand.
pub fn disassemble(module: &Module, name: &str, source: &str) -> Result<String, CompileError> {
    let chunk = compile(module, name, source)?;
    Ok(disasm::chunk(&chunk))
}
