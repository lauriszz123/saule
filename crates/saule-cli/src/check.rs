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
    // Wire the stdlib's native signatures into `saule-typeck`. Idempotent,
    // but it has to have happened before typeck runs on the first file.
    saule_interpreter::init();

    // One database for the whole run. Every file in a project imports some
    // of the same modules, and without this each one walks the shared part
    // of the import graph again from scratch — the cost that dominates a
    // check of anything larger than a handful of files.
    let mut db = saule_db::Db::new();

    let reports = match target {
        None => check_project(&mut db, Path::new(".")),
        Some(t) if t.is_dir() => check_project(&mut db, &t),
        Some(t) => {
            if !t.exists() {
                eprintln!("error: file '{}' does not exist", t.display());
                process::exit(1);
            }
            vec![check_file(&mut db, &t)]
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
fn check_project(db: &mut saule_db::Db, dir: &Path) -> Vec<FileReport> {
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
    files.iter().map(|f| check_file(db, f)).collect()
}

/// Run the front-end over one file and collect everything it complains about.
fn check_file(db: &mut saule_db::Db, path: &Path) -> FileReport {
    let mut diagnostics = Vec::new();

    // Canonicalised so this file is keyed the same way the import walk keys
    // it when some *other* file imports it — otherwise the two spellings
    // are two entries and neither reuses the other's work.
    let abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

    let Some(source) = db.text(&abs) else {
        eprintln!("error reading file '{}'", path.display());
        process::exit(1);
    };

    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let make_src = || NamedSource::new(&name, source.to_string());

    // Both stages recover, so every syntax error in the file is reported in
    // one go — the same reasoning the semantic and type passes below already
    // follow, applied earlier in the pipeline. Lexical errors come first
    // because they change what the tokens are, so the parse errors under them
    // are downstream of them and are best fixed in that order.
    //
    // It is still a hard stop: the recovered tree has holes in it, and a hole
    // makes the later passes report on the repair rather than on the code.
    let parsed = db.parsed(&abs);
    if !parsed.is_clean() {
        for e in &parsed.lex {
            diagnostics.push(Report::new(e.clone()).with_source_code(make_src()));
        }
        for e in &parsed.parse {
            diagnostics.push(Report::new(e.clone()).with_source_code(make_src()));
        }
        return FileReport { diagnostics };
    }

    let seed = (*db.seed(&abs)).clone();

    // Semantic first — typeck reads the registries it installs. Unlike `run`,
    // both passes report their full error list, and typeck runs even when
    // semantic found something: the two families rarely mask each other and a
    // developer would rather see both in one go.
    for e in saule_interpreter::semantic::analyze_with_seed(&parsed.module, seed) {
        diagnostics.push(Report::new(e).with_source_code(make_src()));
    }
    for e in saule_interpreter::typeck::check(&parsed.module) {
        diagnostics.push(Report::new(e).with_source_code(make_src()));
    }

    FileReport { diagnostics }
}
