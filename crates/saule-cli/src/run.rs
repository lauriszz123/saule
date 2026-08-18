//! File execution pipeline: read source → lex → parse → typecheck → evaluate
//! → optionally dispatch to `class Main`'s `static fn main()`.

use std::{
    fs,
    path::{Path, PathBuf},
    process,
};

use miette::{NamedSource, Report};
use saule_interpreter::{Environment, Value, module::ModuleLoader};

/// Which engine runs the program.
///
/// Phase 4 flipped the default: the bytecode VM runs unless the tree-walker
/// is asked for. Nothing about the *fallback* changed — a module the
/// compiler cannot handle still runs on the tree-walker, which is why the
/// flip is safe ahead of complete coverage (§21.3).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Engine {
    /// The default, and what `--vm` / `SAULE_ENGINE=vm` state explicitly.
    Vm { explicit: bool },
    /// `--interp` or `SAULE_ENGINE=interp`. The documented escape hatch.
    Interp,
}

/// The engine for this process: the flags recorded by [`select_engine`],
/// then `SAULE_ENGINE`, then the default.
///
/// The env var exists so a whole test run or benchmark sweep can switch
/// engines without touching call sites — which is how the differential
/// harnesses drive `run_tests.sh` and `run_examples_diff.sh`.
fn engine() -> Engine {
    if let Some(e) = SELECTED.with(|v| v.get()) {
        return e;
    }
    match std::env::var("SAULE_ENGINE") {
        Ok(v) if v.eq_ignore_ascii_case("vm") => Engine::Vm { explicit: true },
        Ok(v) if v.eq_ignore_ascii_case("interp") || v.eq_ignore_ascii_case("interpreter") => {
            Engine::Interp
        }
        // An unrecognised value is not worth an error — it is almost always
        // a stale script — but it must not silently mean "the other one".
        _ => Engine::Vm { explicit: false },
    }
}

thread_local! {
    static SELECTED: std::cell::Cell<Option<Engine>> = const { std::cell::Cell::new(None) };
}

/// Record the engine `--vm` / `--interp` asked for, before any running
/// happens. Neither flag leaves the default in place, so `None` here means
/// `SAULE_ENGINE` still gets its say.
pub(crate) fn select_engine(vm: bool, interp: bool) {
    let choice = match (vm, interp) {
        // clap's `conflicts_with` rules out both at once.
        (_, true) => Some(Engine::Interp),
        (true, _) => Some(Engine::Vm { explicit: true }),
        _ => None,
    };
    SELECTED.with(|v| v.set(choice));
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

    if let Err(report) = run_source(&name, source, require_main, module_dir, Some(&path)) {
        eprintln!("{report:?}");
        process::exit(1);
    }
}

fn run_source(
    name: &str,
    source: String,
    require_main: bool,
    module_dir: Option<PathBuf>,
    // The file this source came from, when it has one. The program driver
    // needs a path to resolve imports against.
    entry_path: Option<&Path>,
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

    // Bytecode engine — the default since Phase 4 — when the compiler can
    // handle the whole module.
    //
    // The fall-back is the point (§21.3): `CompileError::Unsupported` means
    // "not written yet", not "your program is wrong", so the VM could
    // become the default long before the compiler is complete. Anything
    // *else* the compiler reports is a real problem and is surfaced.
    //
    // The note is printed only when the VM was *asked* for. Now that it is
    // the default, a line on every run about a compiler gap the user cannot
    // act on is noise; the differential harnesses set `SAULE_ENGINE=vm`, so
    // they still see it and can still count fallbacks.
    if let Engine::Vm { explicit } = engine() {
        // A program, not a chunk: imports are resolved at *compile* time so
        // a class declared in one module and extended in another has one
        // layout (§14, §24.2). `entry_path` is `None` only for sources with
        // no file behind them, which cannot have imports anyway.
        let compiled = match entry_path {
            Some(p) => saule_vm::program::compile(p).map(Some),
            None => Ok(None),
        };
        let compiled = match compiled {
            Ok(Some(p)) => Ok(p),
            // No path: fall back to the single-module route.
            Ok(None) => saule_vm::compile(&module, name, &source)
                .map(|c| saule_vm::program::Program {
                    modules: vec![std::rc::Rc::new(c)],
                    entry: 0,
                })
                .map_err(saule_vm::program::ProgramError::from),
            Err(e) => Err(e),
        };
        match compiled {
            Ok(program) => {
                let had_main = saule_vm::run_program(program)
                    .map_err(|e| Report::new(e).with_source_code(make_src()))?;
                if !had_main && require_main {
                    eprintln!(
                        "error: `{name}` must declare `class Main` with a `static fn main()` entry point"
                    );
                    process::exit(1);
                }
                return Ok(());
            }
            Err(saule_vm::program::ProgramError::Compile(
                saule_vm::CompileError::Unsupported { thing, .. },
            )) => {
                if explicit {
                    eprintln!(
                        "note: the bytecode compiler does not handle `{thing}` yet — \
                         running on the tree-walking interpreter"
                    );
                }
            }
            // A module this driver could not read, resolve or order. The
            // tree-walker resolves imports its own way and may well manage
            // where this did not, so it falls back and lets the oracle
            // produce any user-facing diagnostic — the two engines must
            // never disagree about whether a program is valid.
            Err(e) if e.is_fallback() => {
                if explicit {
                    eprintln!(
                        "note: the bytecode compiler could not build a program ({e}) — \
                         running on the tree-walking interpreter"
                    );
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    // `SELECTED` is a thread-local and libtest gives each test its own
    // thread, so these do not interfere with one another. They also stop
    // before `SAULE_ENGINE` is consulted, which is process-global and would.

    #[test]
    fn no_flag_leaves_the_choice_open() {
        select_engine(false, false);
        assert_eq!(SELECTED.with(|v| v.get()), None);
    }

    #[test]
    fn the_vm_flag_is_an_explicit_vm() {
        select_engine(true, false);
        // Explicit, so the fallback note is printed: someone who names the
        // engine is asking to hear when they did not get it.
        assert_eq!(engine(), Engine::Vm { explicit: true });
    }

    #[test]
    fn the_interp_flag_selects_the_tree_walker() {
        select_engine(false, true);
        assert_eq!(engine(), Engine::Interp);
    }
}
