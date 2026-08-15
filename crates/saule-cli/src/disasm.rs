//! `saule disasm` — compile a file to bytecode and print it.
//!
//! Wired up in Phase 1, before the compiler that feeds it, because
//! `VM_DESIGN.md` §17.1 is blunt about the reason: debugging a bytecode
//! compiler without a disassembler is miserable, so the disassembler comes
//! first. Every construct codegen learns to emit becomes visible here the
//! same day, with no change to this file.
//!
//! Until then, anything the compiler cannot handle yet is reported as a
//! `CompileError::Unsupported` naming the construct — which is also exactly
//! how `--vm` will decide to fall back to the tree-walker in Phase 2.

use std::path::Path;
use std::process;

use miette::{NamedSource, Report};

/// `saule disasm <file.sau>`
pub(crate) fn cmd_disasm(path: &Path) {
    saule_interpreter::init();

    if !path.exists() {
        eprintln!("error: file '{}' does not exist", path.display());
        process::exit(1);
    }

    let Ok(source) = std::fs::read_to_string(path) else {
        eprintln!("error reading file '{}'", path.display());
        process::exit(1);
    };

    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let make_src = || NamedSource::new(&name, source.clone());

    let tokens = match saule_lexer::Lexer::new(&source).tokenize() {
        Ok(t) => t,
        Err(e) => fail(Report::new(e).with_source_code(make_src())),
    };
    let module = match saule_parser::parse(tokens) {
        Ok(m) => m,
        Err(e) => fail(Report::new(e).with_source_code(make_src())),
    };

    // The compiler's precondition: it consumes what semantic and typeck
    // proved, and asserts rather than re-diagnoses. So both have to have run
    // and been clean before a chunk is built.
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty());
    let seed = match dir {
        Some(d) => saule_interpreter::module::collect_import_seed(&module, d),
        None => saule_semantic::ModuleSeed::default(),
    };
    if let Some(first) = saule_interpreter::analyze_and_prepare(&module, seed)
        .into_iter()
        .next()
    {
        fail(Report::new(first).with_source_code(make_src()));
    }
    if let Some(first) = saule_interpreter::typeck::check(&module).into_iter().next() {
        fail(Report::new(first).with_source_code(make_src()));
    }

    match saule_vm::disassemble(&module, &name, &source) {
        Ok(text) => print!("{text}"),
        Err(e) => fail(Report::new(e).with_source_code(make_src())),
    }
}

fn fail(report: Report) -> ! {
    eprintln!("{report:?}");
    process::exit(1);
}
