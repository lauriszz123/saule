//! The editor path, end to end, over the `examples/` projects.
//!
//! `saule run` type-checks a whole module graph; the language server checks
//! one open file with its imports *seeded* into the registries
//! (`collect_import_seed` → `analyze_with_seed` → `typeck::check`). Those are
//! different code paths, so a call that the CLI accepts can still light up red
//! in the editor — which is exactly what happened to `Ui.panel(title: "…") do
//! … end` when a trailing block bound to the wrong parameter slot.
//!
//! These tests run the server's sequence against real example sources, so the
//! editor experience is gated the same way `run_tests.sh` gates the CLI.

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crate is two levels below the workspace root")
        .to_path_buf()
}

/// Every diagnostic the language server would publish for `path`, rendered as
/// strings. Mirrors `Backend::analyse` minus the LSP plumbing.
fn diagnostics(path: &Path) -> Vec<String> {
    saule_interpreter::init();

    let src = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    let tokens = saule_lexer::Lexer::new(&src)
        .tokenize()
        .unwrap_or_else(|e| panic!("lex {path:?}: {e:?}"));
    let module = saule_parser::parse(tokens).unwrap_or_else(|e| panic!("parse {path:?}: {e:?}"));

    let dir = path.parent().expect("file has a parent directory");
    let seed = saule_interpreter::module::collect_import_seed(&module, dir);

    let mut out: Vec<String> = saule_semantic::analyze_with_seed(&module, seed)
        .iter()
        .map(|e| e.to_string())
        .collect();
    out.extend(saule_typeck::check(&module).iter().map(|e| e.to_string()));
    out
}

fn assert_clean(rel: &str) {
    let path = workspace_root().join(rel);
    let found = diagnostics(&path);
    assert!(
        found.is_empty(),
        "expected no editor diagnostics for {rel}, got:\n  {}",
        found.join("\n  ")
    );
}

/// The trailing-block UI example. `Panel(title: "Session") do … end` passes a
/// block to an initialiser whose `body` parameter sits behind a defaulted
/// `spacing`; binding it anywhere but the last parameter reports
/// `argument 2 of 'Panel' expects 'integer'` in the editor.
#[test]
fn ui_blocks_example_is_clean_in_the_editor() {
    assert_clean("examples/ui-blocks/src/main.sau");
    assert_clean("examples/ui-blocks/src/widgets.sau");
    assert_clean("examples/ui-blocks/src/canvas.sau");
}
