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
use saule_diagnostics::Phase;

/// Outcome for a single file.
struct FileReport {
    /// Rendered diagnostics, in the order the pipeline produced them.
    diagnostics: Vec<Report>,
    /// The same diagnostics as data, for `--json`.
    ///
    /// Built alongside the rendered ones rather than only under the flag, so
    /// the two lists cannot fall out of step with each other — the cost is a
    /// string allocation per diagnostic, and only on the path that already
    /// found an error.
    structured: Vec<saule_diagnostics::Diagnostic>,
    /// Display name, for per-file coverage and for the rendered header.
    name: String,
    /// The path as given, for `--json`.
    ///
    /// The rendered form does not need it — every diagnostic draws its own
    /// `file:line:col` header from the `NamedSource` it was built with — but a
    /// consumer reading the JSON has only this to go on, and a bare file name
    /// is ambiguous once a project has two `main.sau` under different
    /// `src_dirs`.
    path: PathBuf,
    /// `Some` only under `--dump-type-coverage`.
    coverage: Option<saule_runtime::typeck::Coverage>,
}

impl FileReport {
    /// Record one diagnostic in both forms.
    ///
    /// Generic over the error because every type in the pipeline derives
    /// `miette::Diagnostic`; the structured mapping reads the span, help and
    /// code off that trait, so neither this nor `saule-diagnostics` needs a
    /// match over any error enum.
    fn record<E>(&mut self, err: E, phase: Phase, src: NamedSource<String>)
    where
        E: std::error::Error + miette::Diagnostic + Send + Sync + 'static,
    {
        self.structured
            .push(saule_diagnostics::to_diagnostic(&err, phase));
        self.diagnostics
            .push(Report::new(err).with_source_code(src));
    }
}

/// The `--json` document.
///
/// Deliberately an envelope around the per-file lists rather than one flat
/// array: `check` is a multi-file command, and a consumer needs to know which
/// files were examined and found clean, not only which ones complained.
#[derive(serde::Serialize)]
struct JsonReport<'a> {
    /// The toolchain that produced these verdicts, long form.
    ///
    /// A compiler's judgement is only meaningful next to the compiler that
    /// issued it, and a binary on `PATH` can silently lag the tree it was
    /// built from. Recording it here means a stored result can always be
    /// attributed.
    saule_version: &'static str,
    commit: &'static str,
    /// True when nothing was reported, mirroring the exit status.
    ok: bool,
    files: Vec<JsonFile<'a>>,
    summary: JsonSummary,
}

#[derive(serde::Serialize)]
struct JsonFile<'a> {
    path: String,
    diagnostics: &'a [saule_diagnostics::Diagnostic],
}

#[derive(serde::Serialize)]
struct JsonSummary {
    files: usize,
    errors: usize,
    files_with_errors: usize,
}

/// Print the JSON document for a finished run.
fn report_json(reports: &[FileReport]) {
    let errors: usize = reports.iter().map(|r| r.structured.len()).sum();
    let files_with_errors = reports.iter().filter(|r| !r.structured.is_empty()).count();

    let doc = JsonReport {
        saule_version: saule_version::FULL,
        commit: saule_version::COMMIT,
        ok: errors == 0,
        files: reports
            .iter()
            .map(|r| JsonFile {
                path: r.path.display().to_string(),
                diagnostics: &r.structured,
            })
            .collect(),
        summary: JsonSummary {
            files: reports.len(),
            errors,
            files_with_errors,
        },
    };

    emit_json(&doc);
}

/// Serialize to stdout, or die saying why.
///
/// Serialization cannot realistically fail here — every field is a string,
/// number or bool — but a harness reading stdout deserves a parse error it
/// can see rather than a silently truncated document.
fn emit_json<T: serde::Serialize>(doc: &T) {
    match serde_json::to_string_pretty(doc) {
        Ok(text) => println!("{text}"),
        Err(e) => {
            eprintln!("error: could not serialize the check report: {e}");
            process::exit(2);
        }
    }
}

/// A run that failed before any file could be checked.
///
/// `--json` promises a JSON document on stdout; a bare message on stderr
/// would leave a harness with nothing to parse and no way to tell a crash
/// from a clean run.
fn fatal(json: bool, phase: Phase, message: String) -> ! {
    if json {
        let diagnostic = saule_diagnostics::Diagnostic::new(phase, message);
        let doc = JsonReport {
            saule_version: saule_version::FULL,
            commit: saule_version::COMMIT,
            ok: false,
            files: vec![JsonFile {
                path: String::new(),
                diagnostics: std::slice::from_ref(&diagnostic),
            }],
            summary: JsonSummary {
                files: 0,
                errors: 1,
                files_with_errors: 0,
            },
        };
        emit_json(&doc);
    } else {
        eprintln!("error: {message}");
    }
    process::exit(1);
}

/// `saule check [TARGET]` — dispatch on whether `TARGET` is a directory,
/// exactly as `run` does, so the two commands agree about what a "project" is.
pub(crate) fn cmd_check(target: Option<PathBuf>, dump_type_coverage: bool, json: bool) {
    // Wire the stdlib's native signatures into `saule-typeck`. Idempotent,
    // but it has to have happened before typeck runs on the first file.
    saule_runtime::init();

    // One database for the whole run. Every file in a project imports some
    // of the same modules, and without this each one walks the shared part
    // of the import graph again from scratch — the cost that dominates a
    // check of anything larger than a handful of files.
    let mut db = saule_db::Db::new();

    let reports = match target {
        None => check_project(&mut db, Path::new("."), dump_type_coverage, json),
        Some(t) if t.is_dir() => check_project(&mut db, &t, dump_type_coverage, json),
        Some(t) => {
            if !t.exists() {
                fatal(
                    json,
                    Phase::Io,
                    format!("file '{}' does not exist", t.display()),
                );
            }
            vec![check_file(&mut db, &t, dump_type_coverage, json)]
        }
    };

    if dump_type_coverage {
        report_coverage(&reports);
    }

    let files = reports.len();
    let total: usize = reports.iter().map(|r| r.diagnostics.len()).sum();

    // `--json` replaces the rendered diagnostics rather than adding to them:
    // the whole point is a stream a program can parse, and interleaving
    // miette's box drawing with it would defeat that.
    if json {
        report_json(&reports);
        if total > 0 {
            process::exit(1);
        }
        return;
    }

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
fn check_project(
    db: &mut saule_db::Db,
    dir: &Path,
    dump_type_coverage: bool,
    json: bool,
) -> Vec<FileReport> {
    let project = crate::project::configure_project(dir, /* require_entry */ false);
    let files = project.source_files();
    // On stderr even under `--json`, so it cannot corrupt the document on
    // stdout. An empty project is not an error, so it is not a diagnostic.
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
    files
        .iter()
        .map(|f| check_file(db, f, dump_type_coverage, json))
        .collect()
}

/// Run the front-end over one file and collect everything it complains about.
fn check_file(
    db: &mut saule_db::Db,
    path: &Path,
    dump_type_coverage: bool,
    json: bool,
) -> FileReport {
    // Canonicalised so this file is keyed the same way the import walk keys
    // it when some *other* file imports it — otherwise the two spellings
    // are two entries and neither reuses the other's work.
    let abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

    let Some(source) = db.text(&abs) else {
        fatal(
            json,
            Phase::Io,
            format!("error reading file '{}'", path.display()),
        );
    };

    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let make_src = || NamedSource::new(&name, source.to_string());

    let mut file = FileReport {
        diagnostics: Vec::new(),
        structured: Vec::new(),
        name: name.clone(),
        path: path.to_path_buf(),
        coverage: None,
    };

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
            file.record(e.clone(), Phase::Lex, make_src());
        }
        for e in &parsed.parse {
            file.record(e.clone(), Phase::Parse, make_src());
        }
        return file;
    }

    // Unresolved imports first, each at its `import`. They also end the
    // check before the type pass: every name such an import would have bound
    // is unknown, and typeck would only restate that at each use, burying
    // the one line to fix. The editor draws the same line.
    let unresolved = abs
        .parent()
        .map(|dir| saule_runtime::module::unresolved_imports(&parsed.module, dir))
        .unwrap_or_default();
    let skip_types = !unresolved.is_empty();
    for e in unresolved {
        file.record(e, Phase::Import, make_src());
    }

    let seed = (*db.seed(&abs)).clone();

    // Semantic first — typeck reads the registries it installs. Unlike `run`,
    // both passes report their full error list, and typeck runs even when
    // semantic found something: the two families rarely mask each other and a
    // developer would rather see both in one go.
    for e in saule_runtime::semantic::analyze_with_seed(&parsed.module, seed) {
        file.record(e, Phase::Semantic, make_src());
    }
    // Same walk either way — `check_with_types` is `check` plus a sink, so
    // asking for coverage cannot change which diagnostics are produced.
    file.coverage = if skip_types {
        None
    } else if dump_type_coverage {
        let (errors, table) = saule_runtime::typeck::check_with_types(&parsed.module);
        for e in errors {
            file.record(e, Phase::Type, make_src());
        }
        Some(saule_runtime::typeck::coverage::measure(
            &parsed.module,
            &table,
        ))
    } else {
        for e in saule_runtime::typeck::check(&parsed.module) {
            file.record(e, Phase::Type, make_src());
        }
        None
    };

    file
}

/// Print per-file and total type coverage.
///
/// The number that matters is the last one: the bytecode compiler emits a
/// typed arithmetic opcode only where both operands are concretely `integer`
/// or `float`, so that percentage is what `VM_DESIGN.md` §24.1 sets its ~90%
/// bar against. Everything else is context.
fn report_coverage(reports: &[FileReport]) {
    let mut total = saule_runtime::typeck::Coverage::default();
    println!("type coverage:");
    for r in reports {
        let Some(c) = &r.coverage else { continue };
        total.merge(c);
        if c.exprs > 0 {
            println!("  {:<28} {}", r.name, c.summary());
        }
    }
    println!("  {:<28} {}", "TOTAL", total.summary());
}
