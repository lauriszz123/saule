//! A top-level `fn` brought in by an `import` used to reach the
//! typechecker with no signature at all. Classes, interfaces and enums
//! were carried across in the import seed; plain functions were not, so
//! `atLeast(a, b)` inferred as "type unknown" and every checked position
//! it fed — a `local` with an annotation, an assignment to a typed slot —
//! failed with `cannot determine the type of this expression`.
//!
//! The seed now carries function signatures too, which both silences that
//! false positive and makes the imported function's return type checkable.

use std::path::{Path, PathBuf};

use saule_ast::Module;
use saule_typeck::TypeCheckError;

/// Typecheck `src` as if it lived in `dir`, the way the CLI and the LSP
/// do: collect the import seed off disk, run the semantic pass (which
/// installs the registries), then the type pass.
fn type_errors(src: &str, dir: &Path) -> Vec<String> {
    saule_interpreter::init();
    let module = parse(src);
    let seed = saule_interpreter::module::collect_import_seed(&module, dir);
    let _ = saule_semantic::analyze_with_seed(&module, seed);
    saule_typeck::check(&module)
        .iter()
        .map(TypeCheckError::to_string)
        .collect()
}

fn parse(src: &str) -> Module {
    let tokens = saule_lexer::Lexer::new(src).tokenize().expect("lex");
    saule_parser::parse(tokens).expect("parse")
}

/// Fresh scratch directory, unique per test so the suite can run in
/// parallel.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("saule-impsig-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

const GEOMETRY: &str = "\
export fn atLeast(value: float, floor: float) -> float
  if value < floor then
    return floor
  end
  return value
end
";

#[test]
fn an_imported_functions_return_type_is_known() {
    let dir = scratch("known");
    std::fs::write(dir.join("geometry.sau"), GEOMETRY).unwrap();

    let errs = type_errors(
        "\
import * from \"geometry\"

fn run() -> float
  local widest: float = 0.0
  widest = atLeast(widest, 3.0)
  return widest
end
",
        &dir,
    );
    assert!(errs.is_empty(), "expected no type errors, got {errs:?}");
}

#[test]
fn an_imported_functions_return_type_is_enforced() {
    let dir = scratch("enforced");
    std::fs::write(dir.join("geometry.sau"), GEOMETRY).unwrap();

    // Knowing the type is only worth having if it is actually checked:
    // `atLeast` returns `float`, so a `string` slot must be rejected
    // rather than silently skipped the way an unknown type once was.
    let errs = type_errors(
        "\
import * from \"geometry\"

fn run() -> string
  local s: string = atLeast(1.0, 3.0)
  return s
end
",
        &dir,
    );
    assert!(
        errs.iter().any(|e| e.contains("string")),
        "expected a mismatch mentioning `string`, got {errs:?}"
    );
}

#[test]
fn an_import_alias_carries_the_signature_under_the_local_name() {
    let dir = scratch("alias");
    std::fs::write(dir.join("geometry.sau"), GEOMETRY).unwrap();

    let errs = type_errors(
        "\
import atLeast as floorAt from \"geometry\"

fn run() -> string
  local s: string = floorAt(1.0, 3.0)
  return s
end
",
        &dir,
    );
    assert!(
        errs.iter().any(|e| e.contains("string")),
        "expected the aliased call to resolve and mismatch, got {errs:?}"
    );
}

#[test]
fn a_local_declaration_wins_over_an_imported_one_of_the_same_name() {
    let dir = scratch("shadow");
    std::fs::write(dir.join("geometry.sau"), GEOMETRY).unwrap();

    // The module's own `atLeast` returns `string`, so this binding is
    // fine — the imported `float` version must not shadow it.
    let errs = type_errors(
        "\
import * from \"geometry\"

fn atLeast(a: float, b: float) -> string
  return \"x\"
end

fn run() -> string
  local s: string = atLeast(1.0, 3.0)
  return s
end
",
        &dir,
    );
    assert!(errs.is_empty(), "expected no type errors, got {errs:?}");
}
