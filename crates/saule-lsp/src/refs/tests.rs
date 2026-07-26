//! Resolver and collector integration tests.

#![cfg(test)]

use super::*;
use std::sync::Once;

fn init_stdlib() {
    static ONCE: Once = Once::new();
    ONCE.call_once(saule_interpreter::init);
}

fn parse_src(src: &str) -> Module {
    let toks = saule_lexer::Lexer::new(src).tokenize().expect("lex");
    saule_parser::parse(toks).expect("parse")
}

fn analyze(module: &Module) {
    let _ = saule_semantic::analyze(module);
}

/// Resolve at the byte offset of the middle of `needle`'s first
/// occurrence in `src`.
fn resolve(src: &str, needle: &str) -> Symbol {
    init_stdlib();
    let module = parse_src(src);
    analyze(&module);
    let off = src.find(needle).expect("needle") + needle.len() / 2;
    find_symbol_at(&module, src, off)
        .unwrap_or_else(|| panic!("no symbol at {needle:?}"))
        .symbol
}

fn defs_and_refs(src: &str, sym: &Symbol) -> Vec<Hit> {
    let module = parse_src(src);
    analyze(&module);
    collect_in_module(&module, src, sym)
}

#[test]
fn resolves_top_level_function_at_call_site() {
    let src = "fn add(a: integer) -> integer\n  return a\nend\nfn main() -> integer\n  return add(1)\nend\n";
    let s = resolve(src, "add(1)");
    assert!(matches!(&s, Symbol::Function(n) if n == "add"));
    let hits = defs_and_refs(src, &s);
    assert_eq!(hits.iter().filter(|h| h.is_def).count(), 1);
    assert_eq!(hits.iter().filter(|h| !h.is_def).count(), 1);
}

#[test]
fn resolves_local_only_within_file() {
    let src = "fn main() -> integer\n  local x: integer = 1\n  return x + x\nend\n";
    let s = resolve(src, "x:");
    assert!(matches!(&s, Symbol::Local { name, .. } if name == "x"));
    assert!(!s.is_workspace());
    let hits = defs_and_refs(src, &s);
    assert_eq!(hits.iter().filter(|h| h.is_def).count(), 1);
    assert_eq!(hits.iter().filter(|h| !h.is_def).count(), 2);
}

/// `self.super(...)` names no member of the enclosing class — it
/// delegates to the parent constructor, so goto-definition must land on
/// the parent's `init` and find-references must count the call site.
#[test]
fn resolves_self_super_to_parent_init() {
    let src = "\
class Base
  fn init(x: integer)
  end
end

class Child extends Base
  fn init()
    self.super(1)
  end
end
";
    let s = resolve(src, "super(1)");
    assert!(
        matches!(&s, Symbol::Method { class, name } if class == "Base" && name == "init"),
        "got: {s:?}"
    );
    let hits = defs_and_refs(src, &s);
    assert_eq!(hits.iter().filter(|h| h.is_def).count(), 1, "{hits:?}");
    assert_eq!(hits.iter().filter(|h| !h.is_def).count(), 1, "{hits:?}");
}

/// A parent that doesn't declare `init` is skipped: resolution follows
/// the same chain the interpreter's `constructor_chain` walks.
#[test]
fn resolves_self_super_through_ancestor_without_init() {
    let src = "\
class Base
  fn init(x: integer)
  end
end

class Middle extends Base
end

class Child extends Middle
  fn init()
    self.super(1)
  end
end
";
    let s = resolve(src, "super(1)");
    assert!(
        matches!(&s, Symbol::Method { class, name } if class == "Base" && name == "init"),
        "got: {s:?}"
    );
}

/// Goto-definition on an import needs the path to resolve to an
/// `ImportPath` symbol — for every spelling of the path, not just the
/// quoted one.
#[test]
fn resolves_quoted_import_path() {
    let src = "import * from \"entities/Player\"\n";
    let s = resolve(src, "entities/Player");
    assert!(matches!(&s, Symbol::ImportPath(p) if p == "entities/Player"), "got: {s:?}");
}

#[test]
fn resolves_bare_import_path() {
    let src = "import * from Geometry\n";
    let s = resolve(src, "Geometry");
    assert!(matches!(&s, Symbol::ImportPath(p) if p == "Geometry"), "got: {s:?}");
}

#[test]
fn resolves_bare_dotted_import_path() {
    let src = "import View as V from some.folder.module\n";
    let s = resolve(src, "some.folder.module");
    assert!(
        matches!(&s, Symbol::ImportPath(p) if p == "some.folder.module"),
        "got: {s:?}"
    );
}

/// When the imported name and the module spell the same word, the path is
/// the trailing one — the cursor on the name must not resolve to it.
#[test]
fn bare_import_path_is_the_trailing_occurrence() {
    init_stdlib();
    let src = "import Geometry from Geometry\n";
    let module = parse_src(src);
    analyze(&module);

    let on_name = find_symbol_at(&module, src, src.find("Geometry").expect("name") + 2);
    assert!(
        !matches!(on_name.map(|r| r.symbol), Some(Symbol::ImportPath(_))),
        "the imported name is not the module path"
    );

    let on_path = find_symbol_at(&module, src, src.rfind("Geometry").expect("path") + 2);
    assert!(
        matches!(on_path.map(|r| r.symbol), Some(Symbol::ImportPath(p)) if p == "Geometry"),
        "the trailing occurrence is the module path"
    );
}

/// No parent — `self.super(...)` has nothing to point at, and the
/// resolver must not claim a bogus `super` field.
#[test]
fn self_super_without_parent_resolves_to_no_method() {
    let src = "\
class Orphan
  fn init()
    self.super()
  end
end
";
    let s = resolve(src, "super()");
    assert!(!matches!(&s, Symbol::Method { .. }), "got: {s:?}");
}
