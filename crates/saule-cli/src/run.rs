//! File execution pipeline: read source → lex → parse → typecheck →
//! compile → run, then dispatch to `class Main`'s `static fn main()`.

use std::{
    fs,
    path::{Path, PathBuf},
    process,
};

use miette::{NamedSource, Report};

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

    if let Err(report) = run_source(&name, source, require_main, module_dir, &path) {
        eprintln!("{report:?}");
        print_stack_trace(&saule_vm::take_stack_trace());
        process::exit(1);
    }
}

/// How the program reached a runtime error, innermost first. A failure with no
/// call behind it has nothing to add to the snippet above it.
pub(crate) fn print_stack_trace(trace: &saule_vm::StackTrace) {
    if trace.frames.len() < 2 {
        return;
    }
    eprintln!("stack trace (most recent call first):");
    let here = std::env::current_dir().ok();
    let here = here.as_ref().and_then(|d| d.canonicalize().ok());
    for frame in &trace.frames {
        // Relative to where the command was run, as a path someone typed is.
        // Both sides canonical, since on Windows that adds a `\\\\?\\` prefix
        // which would otherwise share no component with the plain form.
        let label = Path::new(&frame.file);
        let canonical = label.canonicalize().ok();
        let file = here
            .as_ref()
            .zip(canonical.as_ref())
            .and_then(|(h, f)| f.strip_prefix(h).ok())
            .unwrap_or(label)
            .display();
        match frame.line {
            Some(line) => eprintln!("  at {} ({file}:{line})", frame.function),
            None => eprintln!("  at {} ({file})", frame.function),
        }
    }
    if trace.omitted > 0 {
        eprintln!("  ... and {} more", trace.omitted);
    }
}

/// Read, check and compile the file at `path`, without running anything.
pub(crate) fn compile_entry(
    name: &str,
    source: String,
    path: &Path,
) -> Result<(saule_ast::Module, saule_vm::program::Program), Report> {
    let module_dir = path
        .canonicalize()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .or_else(|| path.parent().map(Path::to_path_buf));
    front_end(name, source, module_dir, path)
}

fn run_source(
    name: &str,
    source: String,
    require_main: bool,
    module_dir: Option<PathBuf>,
    // The file this source came from. The program driver resolves imports
    // against it.
    entry_path: &Path,
) -> Result<(), Report> {
    let make_src = || NamedSource::new(name, source.clone());
    let (_, program) = front_end(name, source.clone(), module_dir, entry_path)?;

    // Running the module bodies declares `class Main`; `Main.main()` is the
    // project entry point. When required (project mode), missing it is a
    // hard error. For single-file mode it's invoked when present as a
    // convenience.
    let had_main =
        saule_vm::run_program(program).map_err(|e| Report::new(e).with_source_code(make_src()))?;
    if !had_main && require_main {
        eprintln!(
            "error: `{name}` must declare `class Main` with a `static fn main()` entry point"
        );
        process::exit(1);
    }
    Ok(())
}

/// Lex, parse, check and compile. The tree is returned as well, for a caller
/// that wants to read what the file declares.
fn front_end(
    name: &str,
    source: String,
    module_dir: Option<PathBuf>,
    entry_path: &Path,
) -> Result<(saule_ast::Module, saule_vm::program::Program), Report> {
    // Wire the stdlib's native signatures into `saule-typeck` before the
    // static check runs. Idempotent.
    saule_runtime::init();

    let make_src = || NamedSource::new(name, source.clone());

    let tokens = saule_lexer::Lexer::new(&source)
        .tokenize()
        .map_err(|e| Report::new(e).with_source_code(make_src()))?;

    let mut module =
        saule_parser::parse(tokens).map_err(|e| Report::new(e).with_source_code(make_src()))?;

    // An import that does not resolve is reported at the import, before
    // anything type-checks the module: otherwise the first sign of it is a
    // use of a name it would have bound, reported as an unknown type far from
    // the line that needs fixing.
    if let Some(d) = &module_dir
        && let Some(e) = saule_runtime::module::unresolved_imports(&module, d)
            .into_iter()
            .next()
    {
        return Err(Report::new(e).with_source_code(make_src()));
    }

    // Pre-collect class/interface/enum metadata from each direct import
    // so the typechecker can see imported method signatures (e.g. the
    // return type of `Json.decode(...)` from an imported `json` module).
    let seed = match &module_dir {
        Some(d) => saule_runtime::module::collect_import_seed(&module, d),
        None => saule_semantic::ModuleSeed::default(),
    };

    // Static analysis runs before anything is compiled, so we fail fast on
    // declarative errors without ever executing user code. The pipeline is:
    //
    //   1. semantic — registry build, definite-assignment, control-flow
    //                 validity (`break` / `continue` placement), name
    //                 resolution, etc.
    //   2. typeck   — null safety, return types, native arg/arity, match
    //                 exhaustiveness, etc.
    //
    // Semantic runs first because the type pass assumes a structurally
    // valid module and reads the class/interface/enum registry it installs.
    // `analyze_and_check` owns that ordering.
    if let Err(e) = saule_runtime::analyze_and_check(&mut module, seed) {
        return Err(Report::new(e).with_source_code(make_src()));
    }

    // A program, not a chunk: imports are resolved at *compile* time so a
    // class declared in one module and extended in another has one layout
    // (§14, §24.2) — and the modules the entry imports are checked here, by
    // the program driver, since nothing else reads them.
    let program = saule_vm::program::compile(entry_path)
        .map_err(|e| Report::new(e).with_source_code(make_src()))?;
    Ok((module, program))
}
