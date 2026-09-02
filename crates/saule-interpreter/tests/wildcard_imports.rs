//! `import * from "..."` used to make the name resolver give up: any
//! module containing a glob had its undefined-name diagnostics switched
//! off wholesale, so a typo (or a name the author simply never imported)
//! was silently accepted. The module loader can enumerate what a glob
//! actually binds, so it now hands that set to the resolver and the
//! checks stay live.

use std::path::{Path, PathBuf};

use saule_ast::Module;
use saule_semantic::SemanticError;

/// Analyse `src` as if it lived in `dir`, mirroring what the CLI and the
/// LSP do: collect the import seed off disk, then run the semantic pass.
fn undefined_names(src: &str, dir: &Path) -> Vec<String> {
    saule_interpreter::init();
    let module = parse(src);
    let seed = saule_interpreter::module::collect_import_seed(&module, dir);
    saule_semantic::analyze_with_seed(&module, seed)
        .into_iter()
        .filter_map(|e| match e {
            SemanticError::UndefinedName { name, .. } => Some(name),
            _ => None,
        })
        .collect()
}

fn parse(src: &str) -> Module {
    let tokens = saule_lexer::Lexer::new(src).tokenize().expect("lex");
    saule_parser::parse(tokens).expect("parse")
}

/// Fresh scratch directory, unique per test so the suite can run in
/// parallel.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("saule-wildcard-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

const SHAPES: &str = "\
export class Circle
  static fn area(r: float) -> float
    return 3.14 * r * r
  end
end

export fn describe() -> string
  return \"shape\"
end

class Hidden
end
";

#[test]
fn glob_over_a_file_module_still_reports_undefined_names() {
    let dir = scratch("file");
    std::fs::write(dir.join("shapes.sau"), SHAPES).unwrap();

    let found = undefined_names(
        "\
import * from \"shapes\"

fn run() -> nil
  local a = Circle.area(2.0)
  local b = describe()
  local c = Tween(1.5)
end
",
        &dir,
    );

    assert_eq!(found, ["Tween"], "exports must resolve, `Tween` must not");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn glob_does_not_expose_unexported_declarations() {
    let dir = scratch("private");
    std::fs::write(dir.join("shapes.sau"), SHAPES).unwrap();

    let found = undefined_names(
        "\
import * from \"shapes\"

fn run() -> nil
  local h = Hidden()
end
",
        &dir,
    );

    assert_eq!(found, ["Hidden"]);

    let _ = std::fs::remove_dir_all(&dir);
}

/// An `init.sau` barrel re-exports what it imports, so a glob over the
/// folder has to see through one more level.
#[test]
fn glob_over_a_barrel_sees_reexported_names() {
    let dir = scratch("barrel");
    let lib = dir.join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(lib.join("shapes.sau"), SHAPES).unwrap();
    std::fs::write(lib.join("init.sau"), "import * from \"shapes\"\n").unwrap();

    let found = undefined_names(
        "\
import * from \"lib\"

fn run() -> nil
  local a = Circle.area(2.0)
  local c = Missing()
end
",
        &dir,
    );

    assert_eq!(found, ["Missing"]);

    let _ = std::fs::remove_dir_all(&dir);
}

/// When a glob target can't be enumerated at all, the resolver has no
/// way to know what came into scope — it must fall back to staying quiet
/// rather than flooding the file with false positives. (The unresolved
/// import itself is reported separately, by the import-resolution pass.)
#[test]
fn unenumerable_glob_suppresses_undefined_names() {
    let dir = scratch("missing");

    let found = undefined_names(
        "\
import * from \"no_such_module\"

fn run() -> nil
  local c = Whatever()
end
",
        &dir,
    );

    assert!(
        found.is_empty(),
        "expected no undefined names, got {found:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A named import list has never been ambiguous — only the names it
/// lists come into scope, glob or no glob elsewhere in the file.
#[test]
fn named_import_alongside_a_glob_still_reports_undefined_names() {
    let dir = scratch("named");
    std::fs::write(dir.join("shapes.sau"), SHAPES).unwrap();

    let found = undefined_names(
        "\
import Circle as Round from \"shapes\"

fn run() -> nil
  local a = Round.area(2.0)
  local b = describe()
end
",
        &dir,
    );

    assert_eq!(found, ["describe"], "only the listed name is in scope");

    let _ = std::fs::remove_dir_all(&dir);
}
