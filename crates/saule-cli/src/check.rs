//! `saule check` — static analysis without execution.
//!
//! The same front-end `run` uses (lex → parse → semantic → typeck), stopping
//! before evaluation. It exists because until now the only way to find out
//! whether a project type-checked was to *run* it, which is no use in CI, in a
//! pre-commit hook, or on a library that has no entry point at all.
//!
//! Two deliberate differences from [`crate::run`]:
//!
//!   * **Every diagnostic, not just the first.** `run` fails fast because it
//!     is about to execute and the first error already settled that. `check`
//!     is the opposite: you want the whole list so you can fix it in one pass.
//!   * **Every file, not just what the entry point reaches.** A module nobody
//!     imports yet is precisely the one whose errors you want to hear about
//!     before you import it.

use std::{
    fs,
    path::{Path, PathBuf},
    process,
};

use miette::{NamedSource, Report};

/// Outcome for a single file. The path is not carried: every diagnostic
/// already renders its own `file:line:col` header from the `NamedSource` it
/// was built with, so repeating it in the summary would only duplicate it.
struct FileReport {
    /// Rendered diagnostics, in the order the pipeline produced them.
    diagnostics: Vec<Report>,
}

/// `saule check [TARGET]` — dispatch on whether `TARGET` is a directory,
/// exactly as `run` does, so the two commands agree about what a "project" is.
pub(crate) fn cmd_check(target: Option<PathBuf>) {
    let reports = match target {
        None => check_project(Path::new(".")),
        Some(t) if t.is_dir() => check_project(&t),
        Some(t) => {
            if !t.exists() {
                eprintln!("error: file '{}' does not exist", t.display());
                process::exit(1);
            }
            vec![check_file(&t)]
        }
    };

    let files = reports.len();
    let total: usize = reports.iter().map(|r| r.diagnostics.len()).sum();

    for report in &reports {
        for diag in &report.diagnostics {
            eprintln!("{diag:?}");
        }
    }

    if total == 0 {
        println!(
            "checked {files} file{}: no errors",
            if files == 1 { "" } else { "s" }
        );
        return;
    }

    let bad = reports.iter().filter(|r| !r.diagnostics.is_empty()).count();
    eprintln!(
        "checked {files} file{}: {total} error{} in {bad} file{}",
        if files == 1 { "" } else { "s" },
        if total == 1 { "" } else { "s" },
        if bad == 1 { "" } else { "s" }
    );
    process::exit(1);
}

/// Configure the project (so `src_dirs` and dependencies resolve), then check
/// every `.sau` file it owns.
fn check_project(dir: &Path) -> Vec<FileReport> {
    let project = crate::project::configure_project(dir, /* require_entry */ false);
    let files = project.source_files();
    if files.is_empty() {
        eprintln!(
            "warning: no `.sau` files found under {}",
            project
                .src_dirs
                .iter()
                .map(|d| d.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    files.iter().map(|f| check_file(f)).collect()
}

/// Run the front-end over one file and collect everything it complains about.
fn check_file(path: &Path) -> FileReport {
    let mut diagnostics = Vec::new();

    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(err) => {
            eprintln!("error reading file '{}': {}", path.display(), err);
            process::exit(1);
        }
    };

    // Wire the stdlib's native signatures into `saule-typeck`. Idempotent, but
    // it has to have happened before typeck runs on the first file.
    saule_interpreter::init();

    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let make_src = || NamedSource::new(&name, source.clone());

    // Lex and parse are hard stops: without an AST there is nothing for the
    // later passes to say anything about, and reporting "undefined name" for
    // every identifier in a file with one stray bracket is noise, not help.
    let tokens = match saule_lexer::Lexer::new(&source).tokenize() {
        Ok(t) => t,
        Err(e) => {
            diagnostics.push(Report::new(e).with_source_code(make_src()));
            return FileReport { diagnostics };
        }
    };
    let module = match saule_parser::parse(tokens) {
        Ok(m) => m,
        Err(e) => {
            diagnostics.push(Report::new(e).with_source_code(make_src()));
            return FileReport { diagnostics };
        }
    };

    let module_dir = path
        .canonicalize()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .or_else(|| path.parent().map(Path::to_path_buf));

    let seed = match &module_dir {
        Some(d) => saule_interpreter::module::collect_import_seed(&module, d),
        None => saule_semantic::ModuleSeed::default(),
    };

    // Semantic first — typeck reads the registries it installs. Unlike `run`,
    // both passes report their full error list, and typeck runs even when
    // semantic found something: the two families rarely mask each other and a
    // developer would rather see both in one go.
    for e in saule_interpreter::semantic::analyze_with_seed(&module, seed) {
        diagnostics.push(Report::new(e).with_source_code(make_src()));
    }
    for e in saule_interpreter::typeck::check(&module) {
        diagnostics.push(Report::new(e).with_source_code(make_src()));
    }

    FileReport { diagnostics }
}
