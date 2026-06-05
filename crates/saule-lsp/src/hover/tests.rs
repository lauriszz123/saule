//! Hover walker integration tests. Pulled into a sibling file so the
//! main walker stays focused on production code.

#![cfg(test)]

use super::*;
use std::sync::Once;

/// Install the interpreter's stdlib registry hooks exactly once.
/// Without this, `saule_typeck::sigs::lookup` and friends are empty
/// and stdlib hover tests can't find anything.
fn init_stdlib() {
    static ONCE: Once = Once::new();
    ONCE.call_once(saule_interpreter::init);
}

/// Lex + parse + analyse `src` (so the registries are populated)
/// and return whatever hover_at produces at the byte offset of the
/// first occurrence of `needle` (offset = needle.start + 1, i.e. a
/// position inside the token rather than at its left edge).
fn hover(src: &str, needle: &str) -> Option<String> {
    init_stdlib();
    let pos = src.find(needle).expect("needle not found") + 1;
    let tokens = saule_lexer::Lexer::new(src).tokenize().ok()?;
    let module = saule_parser::parse(tokens).ok()?;
    let _ = saule_semantic::analyze(&module);
    hover_at(&module, pos).map(|(md, _)| md)
}

/// Like [`hover`] but threads the original source through so the
/// walker's source-scanning paths (parameter type ascriptions, class
/// field type heads, `extends` / `implements` heads, named-arg keys,
/// per-import-name resolution, `return` keyword) actually fire.
#[allow(dead_code)]
fn hover_src(src: &str, needle: &str) -> Option<String> {
    init_stdlib();
    let pos = src.find(needle).expect("needle not found") + 1;
    let tokens = saule_lexer::Lexer::new(src).tokenize().ok()?;
    let module = saule_parser::parse(tokens).ok()?;
    let _ = saule_semantic::analyze(&module);
    hover_at_with_source(&module, src, pos, &ImportContext::default()).map(|(md, _)| md)
}

/// As [`hover_src`] but with an explicit `offset` past the needle's
/// start, for cases where the token of interest isn't at the left
/// edge of `needle`.
fn hover_src_at(src: &str, needle: &str, offset: usize) -> Option<String> {
    init_stdlib();
    let pos = src.find(needle).expect("needle not found") + offset;
    let tokens = saule_lexer::Lexer::new(src).tokenize().expect("lex");
    let module = saule_parser::parse(tokens).expect("parse");
    let _ = saule_semantic::analyze(&module);
    hover_at_with_source(&module, src, pos, &ImportContext::default()).map(|(md, _)| md)
}

/// As [`hover`] but the cursor is placed `offset` chars past the
/// start of `needle`, useful when the relevant token isn't the one
/// at `needle`'s left edge.
fn hover_at_offset(src: &str, needle: &str, offset: usize) -> Option<String> {
    init_stdlib();
    let pos = src.find(needle).expect("needle not found") + offset;
    let tokens = saule_lexer::Lexer::new(src).tokenize().ok()?;
    let module = saule_parser::parse(tokens).ok()?;
    let _ = saule_semantic::analyze(&module);
    hover_at(&module, pos).map(|(md, _)| md)
}

#[test]
fn hovers_top_level_function() {
    let src = "fn add(a: integer, b: integer) -> integer\n  return a + b\nend\n";
    let md = hover(src, "add").unwrap();
    assert!(md.contains("fn add"), "got: {md}");
    assert!(md.contains("a: integer"), "got: {md}");
    assert!(md.contains("-> integer"), "got: {md}");
}

#[test]
fn hovers_parameter() {
    let src = "fn add(a: integer, b: integer) -> integer\n  return a + b\nend\n";
    let md = hover(src, "a: integer").unwrap();
    assert!(md.contains("(parameter)"), "got: {md}");
    assert!(md.contains("a: integer"), "got: {md}");
}

#[test]
fn hovers_class_head() {
    let src = "\
class Point
  x: integer = 0
  y: integer = 0
end
";
    let head = hover(src, "Point").unwrap();
    assert!(head.contains("class Point"), "got: {head}");
}

#[test]
fn hovers_self_field() {
    let src = "\
class Point
  x: integer = 0
  fn get_x() -> integer
return self.x
  end
end
";
    // Position the cursor on `.x` inside `self.x`.
    let needle = "self.x\n  end";
    let pos = src.find(needle).unwrap() + "self.".len() + 1;
    let tokens = saule_lexer::Lexer::new(src).tokenize().unwrap();
    let module = saule_parser::parse(tokens).unwrap();
    let _ = saule_semantic::analyze(&module);
    let md = hover_at(&module, pos).map(|(md, _)| md).unwrap();
    assert!(md.contains("Point.x"), "got: {md}");
    assert!(md.contains(": integer"), "got: {md}");
}

#[test]
fn hovers_static_method_call() {
    let src = "\
class Counter
  static fn make() -> integer
return 42
  end
end

fn use_it() -> integer
  return Counter.make()
end
";
    let pos = src.find("Counter.make()").unwrap() + "Counter.".len() + 1;
    let tokens = saule_lexer::Lexer::new(src).tokenize().unwrap();
    let module = saule_parser::parse(tokens).unwrap();
    let _ = saule_semantic::analyze(&module);
    let md = hover_at(&module, pos).map(|(md, _)| md).unwrap();
    assert!(md.contains("static"), "got: {md}");
    assert!(md.contains("Counter.make"), "got: {md}");
}

#[test]
fn class_hover_lists_public_members() {
    let src = "\
class Point
  x: integer = 0
  y: integer = 0
  local secret: integer = 0
  fn move(dx: integer, dy: integer) -> nothing
self.x = self.x + dx
self.y = self.y + dy
  end
  local fn _hidden() -> nothing
  end
end
";
    let md = hover(src, "Point").unwrap();
    assert!(md.contains("class Point"), "got: {md}");
    assert!(md.contains("x: integer"), "got: {md}");
    assert!(md.contains("y: integer"), "got: {md}");
    assert!(md.contains("fn move"), "got: {md}");
    // Private members must not leak.
    assert!(!md.contains("secret"), "got: {md}");
    assert!(!md.contains("_hidden"), "got: {md}");
}

#[test]
fn hovers_stdlib_free_function() {
    // `print` is a prelude name with a registered native sig.
    let src = "fn main() -> nothing\n  print(\"hi\")\nend\n";
    let md = hover_at_offset(src, "print(", 1).unwrap();
    assert!(md.contains("fn print"), "got: {md}");
}

#[test]
fn hovers_stdlib_module_member() {
    // `Math.sqrt` should resolve through the native-sig registry
    // since `Math` isn't a real class in the semantic registry.
    let src = "\
fn root() -> float
  return Math.sqrt(2.0)
end
";
    let md = hover_at_offset(src, "Math.sqrt", "Math.".len() + 1).unwrap();
    assert!(md.contains("Math.sqrt"), "got: {md}");
    assert!(md.contains("->"), "got: {md}");
}

#[test]
fn hovers_stdlib_module_name() {
    let src = "\
fn root() -> float
  return Math.sqrt(2.0)
end
";
    let md = hover_at_offset(src, "Math.sqrt", 1).unwrap();
    assert!(md.contains("module Math") || md.contains("type Math"), "got: {md}");
    // Module body should list at least one known member.
    assert!(md.contains("sqrt"), "got: {md}");
}

#[test]
fn does_not_hover_non_prelude_native_module_without_import() {
    init_stdlib();
    saule_typeck::sigs::register(
        "Timer.getTime",
        vec![],
        vec![saule_ast::Type::Named("float".into())],
    );

    let src = "\
fn run() -> float
  return Timer.getTime()
end
";
    let tokens = saule_lexer::Lexer::new(src).tokenize().unwrap();
    let module = saule_parser::parse(tokens).unwrap();
    let _ = saule_semantic::analyze(&module);

    let pos = src.find("Timer.getTime").unwrap() + 1;
    let md = hover_at_with_source(&module, src, pos, &ImportContext::default()).map(|(m, _)| m);
    assert!(
        !md.as_deref().is_some_and(|m| m.contains("Timer")),
        "got: {md:?}"
    );
}

/// End-to-end: write two `.sau` files into a tempdir, import the
/// first from the second, and confirm hover on the imported class
/// name surfaces its full definition (the user's reported case).
#[test]
fn hovers_imported_class_from_disk() {
    init_stdlib();
    let dir = std::env::temp_dir().join(format!(
        "saule-lsp-hover-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let storage_path = dir.join("storage.sau");
    std::fs::write(
        &storage_path,
        "\
class Storage
  name: string = \"\"
  fn save(payload: string) -> nothing
  end
end
",
    )
    .unwrap();

    let app_src = "\
import Storage from \"storage\"

fn run() -> nothing
  local s: Storage = Storage()
end
";
    let tokens = saule_lexer::Lexer::new(app_src).tokenize().unwrap();
    let module = saule_parser::parse(tokens).unwrap();

    // Mirror what `Backend::hover_at` does: collect the seed,
    // analyse, build the import context, then hover.
    let seed = saule_interpreter::module::collect_import_seed(&module, &dir);
    let _ = saule_semantic::analyze_with_seed(&module, seed);
    let imports = build_import_context(&module, Some(&dir));

    // Cursor on the constructor call `Storage()` (the type
    // ascription `: Storage` isn't visited — type nodes don't
    // carry hover info, only expressions do).
    let needle = "Storage()";
    let pos = app_src.find(needle).unwrap() + 1;
    let md = hover_at_with(&module, pos, &imports).map(|(m, _)| m).unwrap();
    assert!(md.contains("class Storage"), "got: {md}");
    assert!(md.contains("name: string"), "got: {md}");
    assert!(md.contains("fn save"), "got: {md}");

    // Hovering on the import statement itself surfaces the path.
    let import_pos = app_src.find("import Storage").unwrap() + 2;
    let md = hover_at_with(&module, import_pos, &imports)
        .map(|(m, _)| m)
        .unwrap();
    assert!(md.contains("(import)"), "got: {md}");
    assert!(md.contains("Storage"), "got: {md}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// Importing a top-level free function should make hover on a call
/// site surface its signature, even though free functions don't go
/// through the semantic class registry.
#[test]
fn hovers_imported_free_function() {
    init_stdlib();
    let dir = std::env::temp_dir().join(format!(
        "saule-lsp-hover-fn-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    std::fs::write(
        dir.join("util.sau"),
        "\
fn greet(name: string) -> string
  return \"hi \" .. name
end
",
    )
    .unwrap();

    let app_src = "\
import greet from \"util\"

fn main() -> nothing
  print(greet(\"world\"))
end
";
    let tokens = saule_lexer::Lexer::new(app_src).tokenize().unwrap();
    let module = saule_parser::parse(tokens).unwrap();

    let seed = saule_interpreter::module::collect_import_seed(&module, &dir);
    let _ = saule_semantic::analyze_with_seed(&module, seed);
    let imports = build_import_context(&module, Some(&dir));

    let pos = app_src.find("greet(\"world\")").unwrap() + 1;
    let md = hover_at_with(&module, pos, &imports)
        .map(|(m, _)| m)
        .unwrap();
    assert!(md.contains("fn greet"), "got: {md}");
    assert!(md.contains("name: string"), "got: {md}");
    assert!(md.contains("-> string"), "got: {md}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// The user's reported case: an unannotated `local newEntry =
/// Entry(...)` followed by method calls on `newEntry`. Hover on
/// the local should surface its inferred type, and method-call
/// hover should resolve through it.
#[test]
fn hovers_local_inferred_from_constructor() {
    let src = "\
class Entry
  todo: string = \"\"
  done: boolean = false
  fn setDone(value: boolean) -> nothing
self.done = value
  end
end

fn use_it() -> nothing
  local newEntry = Entry()
  newEntry.setDone(true)
end
";
    // Hover on the local-binding use site (the second `newEntry`).
    let pos = src.find("newEntry.setDone").unwrap() + 1;
    let tokens = saule_lexer::Lexer::new(src).tokenize().unwrap();
    let module = saule_parser::parse(tokens).unwrap();
    let _ = saule_semantic::analyze(&module);
    let md = hover_at(&module, pos).map(|(m, _)| m).unwrap();
    assert!(md.contains("(local)"), "got: {md}");
    assert!(md.contains("newEntry: Entry"), "got: {md}");

    // Hover on the `setDone` member should resolve via the
    // local's inferred type back to the method signature.
    let pos = src.find("newEntry.setDone").unwrap() + "newEntry.".len() + 1;
    let md = hover_at(&module, pos).map(|(m, _)| m).unwrap();
    assert!(md.contains("Entry.setDone"), "got: {md}");
    assert!(md.contains("value: boolean"), "got: {md}");
}

/// Annotated `local s: Storage = ...` should give the same hover
/// info as the inferred case via the type ascription.
#[test]
fn hovers_local_with_annotation() {
    let src = "\
class Storage
  fn save() -> nothing
  end
end

fn run() -> nothing
  local s: Storage = Storage()
  s.save()
end
";
    let pos = src.find("s.save()").unwrap();
    let tokens = saule_lexer::Lexer::new(src).tokenize().unwrap();
    let module = saule_parser::parse(tokens).unwrap();
    let _ = saule_semantic::analyze(&module);
    let md = hover_at(&module, pos).map(|(m, _)| m).unwrap();
    assert!(md.contains("(local)"), "got: {md}");
    assert!(md.contains("s: Storage"), "got: {md}");
}

/// User-reported case: hover inside a `match` arm should resolve
/// pattern-bound names (here `task` from `case task then ...`) to
/// the scrutinee's inferred type, including when the scrutinee is
/// a method call returning a nullable.
#[test]
fn hovers_match_bind_pattern_from_method_call() {
    let src = "\
class Task
  name: string = \"\"
end

class Storage
  fn remove(index: integer) -> Task?
return nil
  end
end

fn run() -> nothing
  local storage = Storage()
  match storage.remove(1)
case nil then print(\"missing\")
case task then print(task.name)
  end
end
";
    // Cursor on `task` in the pattern position.
    let pos = src.find("case task").unwrap() + "case ".len() + 1;
    let tokens = saule_lexer::Lexer::new(src).tokenize().unwrap();
    let module = saule_parser::parse(tokens).unwrap();
    let _ = saule_semantic::analyze(&module);
    let md = hover_at(&module, pos).map(|(m, _)| m).unwrap();
    assert!(md.contains("(binding)"), "got: {md}");
    assert!(md.contains("task: Task"), "got: {md}");

    // Cursor on `task` in the body — should resolve as the same
    // local binding.
    let pos = src.find("task.name").unwrap() + 1;
    let md = hover_at(&module, pos).map(|(m, _)| m).unwrap();
    assert!(md.contains("(binding)"), "got: {md}");
    assert!(md.contains("task: Task"), "got: {md}");

    // Cursor on `.name` — should resolve through the binding's
    // type back to the field on `Task`.
    let pos = src.find("task.name").unwrap() + "task.".len() + 1;
    let md = hover_at(&module, pos).map(|(m, _)| m).unwrap();
    assert!(md.contains("Task.name"), "got: {md}");
    assert!(md.contains("string"), "got: {md}");
}

/// Enum variant patterns and their payload bindings should both
/// hover correctly inside a `match` arm.
#[test]
fn hovers_match_variant_pattern_and_payload() {
    let src = "\
enum Event
  Click(x: integer, y: integer),
  Quit
end

fn describe(e: Event) -> string
  return match e
case Event.Click(x, y) then \"click\"
case Event.Quit then \"bye\"
  end
end
";
    // Cursor on the variant head `Click`.
    let pos = src.find("Event.Click(").unwrap() + "Event.".len() + 1;
    let tokens = saule_lexer::Lexer::new(src).tokenize().unwrap();
    let module = saule_parser::parse(tokens).unwrap();
    let _ = saule_semantic::analyze(&module);
    let md = hover_at(&module, pos).map(|(m, _)| m).unwrap();
    assert!(md.contains("Event.Click"), "got: {md}");
    assert!(md.contains("x: integer"), "got: {md}");

    // Cursor on the payload binding `x`.
    let pos = src.find("Click(x, y)").unwrap() + "Click(".len();
    let md = hover_at(&module, pos).map(|(m, _)| m).unwrap();
    assert!(md.contains("(binding)"), "got: {md}");
    assert!(md.contains("x: integer"), "got: {md}");
}

// ──────────────────────────────────────────────────────────────────────────────
// Source-threaded hovers: parameter / field / extends / implements / named-arg
// keys / per-import-name / return keyword
// ──────────────────────────────────────────────────────────────────────────────

#[test]
fn hovers_param_type_ascription() {
    let src = "\
class Storage
end

fn use_it(s: Storage) -> nil
  return nil
end
";
    let md = hover_src_at(src, "Storage)", 1).expect("hover");
    assert!(md.contains("class Storage"), "got: {md}");
}

#[test]
fn hovers_return_type_ascription() {
    let src = "\
class Storage
end

fn make() -> Storage
  return Storage()
end
";
    let md = hover_src_at(src, "-> Storage", 4).expect("hover");
    assert!(md.contains("class Storage"), "got: {md}");
}

#[test]
fn hovers_field_type_ascription() {
    let src = "\
class Item
end

class List
  head: Item = Item()
end
";
    let md = hover_src_at(src, "head: Item", "head: ".len()).expect("hover");
    assert!(md.contains("class Item"), "got: {md}");
}

#[test]
fn hovers_extends_head() {
    let src = "\
class Animal
end

class Dog extends Animal
end
";
    let md = hover_src_at(src, "extends Animal", "extends ".len()).expect("hover");
    assert!(md.contains("class Animal"), "got: {md}");
}

#[test]
fn hovers_implements_head() {
    let src = "\
interface Greeter
  fn hello() -> string
end

class Cat implements Greeter
  fn hello() -> string
    return \"meow\"
  end
end
";
    let md = hover_src_at(src, "implements Greeter", "implements ".len()).expect("hover");
    assert!(md.contains("interface Greeter"), "got: {md}");
}

#[test]
fn hovers_return_keyword() {
    let src = "\
fn forty_two() -> integer
  return 42
end
";
    let md = hover_src_at(src, "return 42", 1).expect("hover");
    assert!(md.contains("(return)"), "got: {md}");
    assert!(md.contains("integer"), "got: {md}");
}

#[test]
fn hovers_named_call_arg_key() {
    // The parser only supports named-argument syntax on free / static
    // calls (sibling static dispatch inside a class), not `:method`
    // calls. Mirror the shape used by tests/named_params.sau.
    let src = "\
class Main
  static local fn put(item: string, count: integer = 1)
  end

  static fn main()
    put(\"x\", count: 3)
  end
end
";
    let md = hover_src_at(src, "count: 3", 1).expect("hover");
    assert!(md.contains("(named arg)"), "got: {md}");
    assert!(md.contains("count"), "got: {md}");
    assert!(md.contains("integer"), "got: {md}");
}

#[test]
fn hovers_local_type_ascription() {
    let src = "\
class Storage
end

local s: Storage = Storage()
";
    let md = hover_src_at(src, "s: Storage = Storage()", "s: ".len()).expect("hover");
    assert!(md.contains("class Storage"), "got: {md}");
}

#[test]
fn hovers_chained_method_inference() {
    // The parser doesn't support `a.b().c()` directly, but the
    // semantic equivalent — chaining through a local — exercises the
    // same `receiver_class` path that resolves the inferred class of
    // a method call's return type.
    let src = "\
class A
  fn b() -> A
    return self
  end
  fn c() -> A
    return self
  end
end

class Main
  static fn main()
    local a = A()
    local mid = a.b()
    local r = mid.c()
  end
end
";
    let md = hover_src_at(src, "mid.c()", "mid.".len() + 1).expect("hover");
    assert!(md.contains("-> A"), "got: {md}");
}

/// A multi-return (tuple) static method spreads across a `local q, r =
/// Class.method()` binding: each name should hover as the matching tuple
/// component, not the whole tuple.
#[test]
fn hovers_multi_return_local_spread() {
    let src = "\
class Util
  static fn divmod(a: integer, b: integer) -> (integer, integer)
    return a, b
  end
end

class Main
  static fn main()
    local q, r = Util.divmod(17, 5)
    println(q)
    println(r)
  end
end
";
    // `q` resolves to the first tuple component.
    let md = hover_at_offset(src, "println(q)", "println(".len()).expect("hover q");
    assert!(md.contains("(local)"), "got: {md}");
    assert!(md.contains("q: integer"), "got: {md}");

    // `r` likewise resolves to the second component.
    let md = hover_at_offset(src, "println(r)", "println(".len()).expect("hover r");
    assert!(md.contains("(local)"), "got: {md}");
    assert!(md.contains("r: integer"), "got: {md}");

    // Hovering the binding site itself (`local q, r = …`) also works.
    let md = hover_at_offset(src, "local q, r =", "local ".len()).expect("hover binding q");
    assert!(md.contains("(local)"), "got: {md}");
    assert!(md.contains("q: integer"), "got: {md}");
    let md = hover_at_offset(src, "local q, r =", "local q, ".len()).expect("hover binding r");
    assert!(md.contains("(local)"), "got: {md}");
    assert!(md.contains("r: integer"), "got: {md}");
}

/// In single-value context a multi-return collapses to its first
/// component: `local justQ = Util.divmod(...)` is `integer`, not a tuple.
#[test]
fn hovers_multi_return_single_binding_collapses() {
    let src = "\
class Util
  static fn divmod(a: integer, b: integer) -> (integer, integer)
    return a, b
  end
end

class Main
  static fn main()
    local justQ = Util.divmod(20, 6)
    println(justQ)
  end
end
";
    let md = hover_at_offset(src, "println(justQ)", "println(".len()).expect("hover justQ");
    assert!(md.contains("(local)"), "got: {md}");
    assert!(md.contains("justQ: integer"), "got: {md}");
}
