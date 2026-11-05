//! `saule test` — run a project's tests.
//!
//! A test is a top-level function with no parameters whose name starts with
//! `test_`. It passes unless it fails: a false `assert`, a `throw` nothing
//! catches, or any runtime error. A test file is any `.sau` under `tests/`, or
//! one ending `_test.sau` under the project's source directories.
//!
//! Each file is its own program and runs its top level once, so one file
//! cannot disturb another. Tests within a file share that top level, and one
//! failing does not stop the rest.

use std::path::{Path, PathBuf};
use std::process;

use miette::{NamedSource, Report};
use saule_ast::{Decl, Module, Stmt};

use crate::project;
use crate::run;

/// The tests a module declares, in source order.
fn test_names(module: &Module) -> Vec<String> {
    module
        .stmts
        .iter()
        .filter_map(|stmt| match &stmt.value {
            Stmt::Decl(d) => match &d.value {
                Decl::Function { name, params, .. }
                    if name.starts_with("test_") && params.is_empty() =>
                {
                    Some(name.clone())
                }
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// Every test file of the project at `root`, each once and in a stable order.
fn discover(root: &Path, src_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut files = saule_project::scan_sources(&root.join("tests"));
    for dir in src_dirs {
        files.extend(saule_project::scan_sources(dir).into_iter().filter(|f| {
            f.file_stem()
                .is_some_and(|s| s.to_string_lossy().ends_with("_test"))
        }));
    }
    files.sort();
    files.dedup();
    files
}

#[derive(Default)]
struct Tally {
    passed: usize,
    failed: usize,
}

fn print_failure(report: Report) {
    eprintln!("{report:?}");
    let trace = saule_vm::take_stack_trace();
    run::print_stack_trace(&trace);
}

fn run_test_file(path: &Path, filter: Option<&str>, tally: &mut Tally) {
    let shown = saule_project::user_path(path);
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            println!("{shown}\n  FAIL  could not read it: {e}");
            tally.failed += 1;
            return;
        }
    };
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| shown.clone());
    let make_src = || NamedSource::new(name.clone(), source.clone());

    let compiled = run::compile_entry(&name, source.clone(), path);
    let (module, program) = match compiled {
        Ok(c) => c,
        Err(report) => {
            println!("{shown}\n  FAIL  does not compile");
            print_failure(report);
            tally.failed += 1;
            return;
        }
    };

    let names: Vec<String> = test_names(&module)
        .into_iter()
        .filter(|n| filter.is_none_or(|f| n.contains(f)))
        .collect();
    if names.is_empty() {
        return;
    }

    println!("{shown}");
    let outcome = saule_vm::run_tests(program, &names, |test, result| match result {
        Ok(()) => {
            println!("  ok    {test}");
            tally.passed += 1;
        }
        Err(e) => {
            println!("  FAIL  {test}");
            print_failure(Report::new(e).with_source_code(make_src()));
            tally.failed += 1;
        }
    });
    if let Err(e) = outcome {
        println!("  FAIL  the file's top level failed, so its tests could not run");
        print_failure(Report::new(e).with_source_code(make_src()));
        tally.failed += names.len();
    }
}

/// `saule test [TARGET] [--filter TEXT]`. A directory, or nothing, is a
/// project; a file is that file on its own.
pub(crate) fn cmd_test(target: Option<PathBuf>, filter: Option<String>) {
    let target = target.unwrap_or_else(|| PathBuf::from("."));
    let files = if target.is_dir() {
        let project = project::configure_project(&target, /* require_entry */ false);
        discover(&target, &project.src_dirs)
    } else {
        vec![target]
    };

    let mut tally = Tally::default();
    for file in &files {
        run_test_file(file, filter.as_deref(), &mut tally);
    }

    if tally.passed + tally.failed == 0 {
        println!(
            "No tests found. A test is a top-level `fn test_name()`, in a file under \
             `tests/` or one ending `_test.sau`."
        );
        return;
    }
    println!("\n{} passed, {} failed", tally.passed, tally.failed);
    if tally.failed > 0 {
        process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(source: &str) -> Vec<String> {
        let tokens = saule_lexer::Lexer::new(source).tokenize().expect("lex");
        test_names(&saule_parser::parse(tokens).expect("parse"))
    }

    #[test]
    fn a_test_is_a_top_level_function_named_test_with_no_parameters() {
        let source = "fn test_a()\nend\nfn helper()\nend\nfn test_b(x: integer)\nend\n\
                      class C\n  fn test_c()\n  end\nend\n";
        assert_eq!(names(source), vec!["test_a"]);
    }
}
