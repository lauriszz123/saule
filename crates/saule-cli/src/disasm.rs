//! `saule disasm` — compile to bytecode and print it.
//!
//! Wired up in Phase 1, before the compiler that feeds it, because
//! `VM_DESIGN.md` §17.1 is blunt about the reason: debugging a bytecode
//! compiler without a disassembler is miserable, so the disassembler comes
//! first. Every construct codegen learns to emit becomes visible here the
//! same day, with no change to this file.
//!
//! Anything the compiler cannot handle is reported as a compile error naming
//! the construct, the same one `saule run` reports.
//!
//! ## Why this goes through the program driver
//!
//! It used to compile the target file on its own, through
//! [`saule_vm::disassemble`], which is the single-module route. That works
//! only for a file with nothing behind it: a module that is part of an import
//! graph fails, because the compiler resolves imports at *compile* time so
//! that a class declared in one module and extended in another has one layout
//! (§14, §24.2). A file compiled in isolation cannot see its parent class, so
//! it was rejected with `cannot compile a class extending one the compiler
//! cannot see`, `cannot compile a name the resolver could not classify`, or
//! `cannot compile an import in a program with no file behind it` — none of
//! which was a fault in the code, as the same project type-checks clean.
//!
//! That ruled out exactly the files most worth looking at. Measured over this
//! repository before the change: all 97 single-file fixtures and all 16
//! benchmarks disassembled, but only 14 of 68 files under `examples/` and 2 of
//! 12 across the two sibling libraries.
//!
//! So it now builds a [`saule_vm::program::Program`] rooted at the target,
//! exactly as `saule run` does, and prints chunks out of it. The target's own
//! chunk is what you asked for; `--deps` adds the modules it imports, and a
//! directory disassembles the whole project.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process;

use miette::{NamedSource, Report};

/// `saule disasm [TARGET] [--deps]`
///
/// `TARGET` follows the same rule as `run` and `check`: a directory, or no
/// argument at all, means project mode; a file means that file.
pub(crate) fn cmd_disasm(target: Option<PathBuf>, deps: bool) {
    saule_runtime::init();

    match target {
        None => disasm_project(Path::new(".")),
        Some(t) if t.is_dir() => disasm_project(&t),
        Some(t) => {
            if !t.exists() {
                eprintln!("error: file '{}' does not exist", t.display());
                process::exit(1);
            }
            let mut emitted = BTreeSet::new();
            match disasm_rooted_at(&t, deps, &mut emitted) {
                Ok(text) => print!("{text}"),
                Err(report) => fail(report),
            }
        }
    }
}

/// Disassemble every module a project owns.
///
/// Rooted at each source file in turn rather than only at the entry, for the
/// reason `check` walks every file: a module nothing imports yet is precisely
/// the one worth looking at. It also means a library — which has no entry
/// point at all — disassembles like anything else.
///
/// Each root pulls in its own import graph, so the first file usually emits
/// most of the project and the rest are already accounted for. `emitted`
/// keeps a module from being printed twice when several roots reach it.
fn disasm_project(dir: &Path) {
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
        return;
    }

    let mut emitted = BTreeSet::new();
    let mut out = String::new();
    let mut failures = 0usize;

    for file in &files {
        // Project mode wants every module, so each root contributes its whole
        // graph; `emitted` is what keeps that from repeating itself.
        match disasm_rooted_at(file, /* deps */ true, &mut emitted) {
            Ok(text) => out.push_str(&text),
            Err(report) => {
                // One unusable file does not stop the rest. A project with a
                // construct the compiler cannot emit yet should still show
                // the chunks it can.
                eprintln!("{report:?}");
                failures += 1;
            }
        }
    }

    print!("{out}");

    if failures > 0 {
        eprintln!(
            "disassembled {} module(s); {failures} file(s) failed",
            emitted.len()
        );
        process::exit(1);
    }
}

/// Compile the import graph rooted at `path` and render chunks from it.
///
/// Returns the text rather than printing it, so project mode can collect the
/// whole run and a failure part-way through does not leave half a chunk on
/// stdout.
fn disasm_rooted_at(
    path: &Path,
    deps: bool,
    emitted: &mut BTreeSet<String>,
) -> Result<String, Report> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| Report::msg(format!("error reading file '{}': {e}", path.display())))?;

    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let make_src = || NamedSource::new(&name, source.clone());

    let tokens = saule_lexer::Lexer::new(&source)
        .tokenize()
        .map_err(|e| Report::new(e).with_source_code(make_src()))?;
    let mut module =
        saule_parser::parse(tokens).map_err(|e| Report::new(e).with_source_code(make_src()))?;

    // An unresolved import is reported at the `import` itself, before anything
    // type-checks: otherwise the first sign of it is a use of a name it would
    // have bound, reported as an unknown type far from the line to fix.
    let module_dir = path
        .canonicalize()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .or_else(|| path.parent().map(Path::to_path_buf))
        .filter(|p| !p.as_os_str().is_empty());

    if let Some(dir) = &module_dir
        && let Some(e) = saule_runtime::module::unresolved_imports(&module, dir)
            .into_iter()
            .next()
    {
        return Err(Report::new(e).with_source_code(make_src()));
    }

    // The compiler's precondition: it consumes what semantic and typeck
    // proved, and asserts rather than re-diagnoses. So both have to have run
    // and been clean before a chunk is built. The seed carries each direct
    // import's class, interface and enum metadata, which is what lets the
    // type pass see an imported method's signature.
    let seed = match &module_dir {
        Some(dir) => saule_runtime::module::collect_import_seed(&module, dir),
        None => saule_semantic::ModuleSeed::default(),
    };
    if let Err(e) = saule_runtime::analyze_and_check(&mut module, seed) {
        return Err(Report::new(e).with_source_code(make_src()));
    }

    // A program, not a chunk — see the module comment. The driver checks the
    // imported modules itself, since nothing else reads them.
    let program = saule_vm::program::compile(path)
        .map_err(|e| Report::new(e).with_source_code(make_src()))?;

    let mut out = String::new();

    // Post-order, so a module appears after the ones it imports — the order
    // in which they were compiled, and the useful one to read.
    let chunks: Vec<_> = if deps {
        program.modules.iter().collect()
    } else {
        vec![program.entry_chunk()]
    };

    for chunk in chunks {
        // The chunk's own source name is both the dedup key and the first line
        // of its disassembly (`; chunk from <name>`), so a reader and this set
        // agree about what a module is called.
        let label = chunk.source.name().to_string();
        if !emitted.insert(label) {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&saule_vm::disasm::chunk(chunk));
    }

    Ok(out)
}

fn fail(report: Report) -> ! {
    eprintln!("{report:?}");
    process::exit(1);
}
