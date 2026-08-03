//! File execution pipeline: read source → lex → parse → typecheck → evaluate
//! → optionally dispatch to `class Main`'s `static fn main()`.

use std::{
    fs,
    path::{Path, PathBuf},
    process,
};

use miette::{NamedSource, Report};
use saule_interpreter::{Environment, Value, module::ModuleLoader};

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
    let sem_errors = saule_interpreter::semantic::analyze_with_seed(&module, seed);
    if let Some(first) = sem_errors.into_iter().next() {
        return Err(Report::new(first).with_source_code(make_src()));
    }

    let errors = saule_interpreter::typeck::check(&module);
    if let Some(first) = errors.into_iter().next() {
        return Err(Report::new(first).with_source_code(make_src()));
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
