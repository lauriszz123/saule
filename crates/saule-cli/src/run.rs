//! File execution pipeline: read source → lex → parse → typecheck → evaluate
//! → optionally dispatch to `class Main`'s `static fn main()`.

use std::{
    fs,
    path::{Path, PathBuf},
    process,
};

use miette::{NamedSource, Report};
use saule_interpreter::{Environment, Value, module::ModuleLoader};

/// Whether to use the bytecode VM: the `--vm` flag or `SAULE_ENGINE=vm`.
///
/// The env var exists so a whole test run or benchmark sweep can switch
/// engines without touching call sites — which is how the differential
/// harness will drive `run_tests.sh` in Phase 3.
fn use_vm() -> bool {
    VM_REQUESTED.with(|v| v.get())
        || std::env::var("SAULE_ENGINE").is_ok_and(|v| v.eq_ignore_ascii_case("vm"))
}

thread_local! {
    static VM_REQUESTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Record that `--vm` was passed, before any running happens.
pub(crate) fn request_vm() {
    VM_REQUESTED.with(|v| v.set(true));
}

pub(crate) fn run_file(path: PathBuf, require_main: bool) {
    if !path.exists() {
        eprintln!("error: file '{}' does not exist", path.display());
        process::exit(1);
    }
    let source = fs::read_to_string(&path).unwrap_or_else(|err| {
        eprintln!("error reading file '{}': {}", path.display(), err);
        process::exit(1);
    });

    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());

    let module_dir = path
        .canonicalize()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .or_else(|| path.parent().map(Path::to_path_buf));

    if let Err(report) = run_source(&name, source, require_main, module_dir) {
        eprintln!("{report:?}");
        process::exit(1);
    }
}

fn run_source(
    name: &str,
    source: String,
    require_main: bool,
    module_dir: Option<PathBuf>,
) -> Result<(), Report> {
    // Wire the stdlib's native signatures into `saule-typeck` before the
    // static check runs. Idempotent.
    saule_interpreter::init();

    let make_src = || NamedSource::new(name, source.clone());

    let tokens = saule_lexer::Lexer::new(&source)
        .tokenize()
        .map_err(|e| Report::new(e).with_source_code(make_src()))?;

    let module =
        saule_parser::parse(tokens).map_err(|e| Report::new(e).with_source_code(make_src()))?;

    // Pre-collect class/interface/enum metadata from each direct import
    // so the typechecker can see imported method signatures (e.g. the
    // return type of `Json.decode(...)` from an imported `json` module).
    let seed = match &module_dir {
        Some(d) => saule_interpreter::module::collect_import_seed(&module, d),
        None => saule_semantic::ModuleSeed::default(),
    };

    // Static analysis runs *before* evaluation so we fail fast on declarative
    // errors without ever executing user code. The pipeline is:
    //
    //   1. semantic — registry build, definite-assignment, control-flow
    //                 validity (`break` / `continue` placement), name
    //                 resolution, etc.
    //   2. typeck   — null safety, return types, native arg/arity, match
    //                 exhaustiveness, etc.
    //
    // Semantic runs first because the type pass assumes a structurally
    // valid module and reads the class/interface/enum registry it installs.
    let sem_errors = saule_interpreter::analyze_and_prepare(&module, seed);
    if let Some(first) = sem_errors.into_iter().next() {
        return Err(Report::new(first).with_source_code(make_src()));
    }

    let errors = saule_interpreter::typeck::check(&module);
    if let Some(first) = errors.into_iter().next() {
        return Err(Report::new(first).with_source_code(make_src()));
    }

    // Bytecode engine, when asked for and when the compiler can handle the
    // whole module.
    //
    // The fall-back is the point (§21.3): `CompileError::Unsupported` means
    // "not written yet", not "your program is wrong", so `--vm` is usable
    // long before the compiler is complete. Anything *else* the compiler
    // reports is a real problem and is surfaced.
    if use_vm() {
        match saule_vm::compile(&module, name, &source) {
            Ok(chunk) => {
                let had_main = saule_vm::run_chunk_entry(std::rc::Rc::new(chunk))
                    .map_err(|e| Report::new(e).with_source_code(make_src()))?;
                if !had_main && require_main {
                    eprintln!(
                        "error: `{name}` must declare `class Main` with a `static fn main()` entry point"
                    );
                    process::exit(1);
                }
                return Ok(());
            }
            Err(saule_vm::CompileError::Unsupported { thing, .. }) => {
                eprintln!(
                    "note: the bytecode compiler does not handle `{thing}` yet — \
                     running on the tree-walking interpreter"
                );
            }
            Err(e) => return Err(Report::new(e).with_source_code(make_src())),
        }
    }

    // Execute the file's top-level statements so declarations register.
    // The environment carries the file's directory plus a shared module
    // loader so `import "..."` can resolve relative paths and dedupe
    // already-loaded modules.
    let loader = ModuleLoader::new();
    let env = Environment::with_prelude_and_context(module_dir, Some(loader));
    saule_interpreter::run_in(&module, &env)
        .map_err(|e| Report::new(e).with_source_code(make_src()))?;

    // Project entry point: `class Main` with `static fn main()`.
    // When required (project mode), missing it is a hard error.
    // For single-file mode it's invoked when present as a convenience.
    let main_class = match env.borrow().get("Main") {
        Some(Value::Class(c)) => Some(c),
        _ => None,
    };
    match main_class {
        Some(c) if c.lookup_static_method("main").is_some() => {
            saule_interpreter::call_class_static_method(&c, "main", &[])
                .map_err(|e| Report::new(e).with_source_code(make_src()))?;
        }
        _ if require_main => {
            eprintln!(
                "error: `{name}` must declare `class Main` with a `static fn main()` entry point"
            );
            process::exit(1);
        }
        _ => {}
    }

    Ok(())
}
