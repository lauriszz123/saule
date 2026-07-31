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

/// As [`hover_src_at`] but with the module's own [`ImportContext`]
/// built first, exactly as `Backend::hover` does. Needed for anything
/// that reads `---` blocks through the doc index rather than by
/// re-scanning source at a declaration anchor.
fn hover_ctx_at(src: &str, needle: &str, offset: usize) -> Option<String> {
    init_stdlib();
    let pos = src.find(needle).expect("needle not found") + offset;
    let tokens = saule_lexer::Lexer::new(src).tokenize().expect("lex");
    let module = saule_parser::parse(tokens).expect("parse");
    let _ = saule_semantic::analyze(&module);
    let ctx = build_import_context(&module, src, None);
    hover_at_with_source(&module, src, pos, &ctx).map(|(md, _)| md)
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

/// The constructor reads as the class's own parameter list rather than
/// as an `init` method buried alphabetically among the others.
#[test]
fn class_hover_hoists_the_constructor_onto_the_heading() {
    let src = "\
class Entry
  local todo: string
  local dueDate: integer?
  fn init(todo: string, dueDate: integer?)
self.todo = todo
self.dueDate = dueDate
  end
  fn getTodo() -> string
return self.todo
  end
end
";
    let md = hover(src, "Entry").unwrap();
    assert!(
        md.contains("class Entry(todo: string, dueDate: integer?)"),
        "got: {md}"
    );
    // The body must not repeat what the heading already says.
    assert!(!md.contains("fn init"), "got: {md}");
    assert!(md.contains("fn getTodo() -> string"), "got: {md}");
}

/// A class with no constructor keeps a bare heading — an empty `()`
/// would imply a zero-arg constructor the class doesn't declare.
#[test]
fn class_hover_omits_parens_when_there_is_no_constructor() {
    let src = "\
class Point
  x: integer = 0
end
";
    let md = hover(src, "Point").unwrap();
    assert!(md.contains("class Point"), "got: {md}");
    assert!(!md.contains("class Point("), "got: {md}");
}

/// A `local fn init` can't be called from outside, so advertising a
/// call shape on the heading would be a lie.
#[test]
fn class_hover_hides_a_private_constructor() {
    let src = "\
class Secret
  local seed: integer
  local fn init(seed: integer)
self.seed = seed
  end
  fn get() -> integer
return self.seed
  end
end
";
    let md = hover(src, "Secret").unwrap();
    assert!(!md.contains("class Secret("), "got: {md}");
    assert!(!md.contains("init"), "got: {md}");
    assert!(md.contains("fn get() -> integer"), "got: {md}");
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
    assert!(
        md.contains("module Math") || md.contains("type Math"),
        "got: {md}"
    );
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
    let dir = std::env::temp_dir().join(format!("saule-lsp-hover-test-{}", std::process::id()));
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
    let imports = build_import_context(&module, app_src, Some(&dir));

    // Cursor on the constructor call `Storage()` (the type
    // ascription `: Storage` isn't visited — type nodes don't
    // carry hover info, only expressions do).
    let needle = "Storage()";
    let pos = app_src.find(needle).unwrap() + 1;
    let md = hover_at_with(&module, pos, &imports)
        .map(|(m, _)| m)
        .unwrap();
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
    let dir = std::env::temp_dir().join(format!("saule-lsp-hover-fn-test-{}", std::process::id()));
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
    let imports = build_import_context(&module, app_src, Some(&dir));

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
    // A named-argument key renders as the parameter it is, qualified by
    // the callee so the reader can tell which `count:` this is.
    assert_eq!(
        md,
        "```saule\n(parameter) Main.put.count: integer = …\n```"
    );
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

/// Hovering `self.super(...)` shows the parent constructor it
/// delegates to, not an empty popup.
#[test]
fn hovers_self_super_as_parent_constructor() {
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
    let md = hover(src, "super(1)").expect("hover super");
    assert!(md.contains("fn Base.init(x: integer)"), "got: {md}");
    assert!(md.contains("Parent constructor"), "got: {md}");
    assert!(md.contains("`Child`"), "got: {md}");
}

/// Arguments of a `self.super(...)` call hover as the parent ctor's
/// parameters, same as any other call.
#[test]
fn hovers_self_super_named_argument() {
    let src = "\
class Base
  fn init(width: float)
  end
end

class Child extends Base
  fn init()
    self.super(width: 1.0)
  end
end
";
    let md = hover_src_at(src, "self.super(width: 1.0)", "self.super(w".len() - 1)
        .expect("hover named arg");
    assert!(md.contains("width"), "got: {md}");
}

// ─── `---` doc comments ─────────────────────────────────────────────────

/// Like [`hover_src`] but builds the doc index the way
/// `Backend::hover_at` does, so usage-site hovers (which resolve
/// through the semantic registries) can find doc text too.
fn hover_doc(src: &str, needle: &str) -> Option<String> {
    init_stdlib();
    let pos = src.find(needle).expect("needle not found") + 1;
    let tokens = saule_lexer::Lexer::new(src).tokenize().ok()?;
    let module = saule_parser::parse(tokens).ok()?;
    let _ = saule_semantic::analyze(&module);
    let imports = build_import_context(&module, src, None);
    hover_at_with_source(&module, src, pos, &imports).map(|(md, _)| md)
}

/// The exact shape from the design discussion.
const ENTITY: &str = "\
--- A base class for all entities.
class Entity
  --- Private variable has only descriptions:
  local var: integer = 10

  --- Some other description for the initializer
  --- @param a This is a description for the parameter.
  fn init(a: string)
  end
end
";

#[test]
fn hover_shows_class_doc() {
    let md = hover_src(ENTITY, "class Entity").expect("class hover");
    assert!(
        md.contains("A base class for all entities."),
        "missing class summary in:\n{md}"
    );
}

#[test]
fn hover_shows_private_field_doc() {
    let md = hover_src(ENTITY, "local var").expect("field hover");
    assert!(
        md.contains("Private variable has only descriptions:"),
        "missing field summary in:\n{md}"
    );
}

#[test]
fn hover_shows_method_doc_and_param_list() {
    let md = hover_src(ENTITY, "fn init").expect("method hover");
    assert!(
        md.contains("Some other description for the initializer"),
        "missing method summary in:\n{md}"
    );
    assert!(
        md.contains("`a` — This is a description for the parameter."),
        "missing rendered @param in:\n{md}"
    );
}

#[test]
fn hover_on_a_parameter_shows_only_its_own_param_doc() {
    let md = hover_src_at(ENTITY, "fn init(a: string)", 8).expect("param hover");
    assert!(
        md.contains("This is a description for the parameter."),
        "missing @param text in:\n{md}"
    );
    // The enclosing function's summary belongs on the function.
    assert!(
        !md.contains("Some other description"),
        "parameter hover leaked the method summary:\n{md}"
    );
}

#[test]
fn hover_keeps_the_signature_above_the_docs() {
    let md = hover_src(ENTITY, "fn init").expect("method hover");
    let sig = md.find("fn Entity.init").expect("signature present");
    let doc = md.find("Some other description").expect("doc present");
    assert!(sig < doc, "docs should follow the signature:\n{md}");
}

#[test]
fn hover_on_a_usage_site_shows_the_declaration_doc() {
    let src = "\
--- A base class for all entities.
class Entity
end

fn spawn() -> Entity
  return Entity()
end
";
    let md = hover_doc(src, "Entity()").expect("usage hover");
    assert!(
        md.contains("A base class for all entities."),
        "usage-site hover missing doc:\n{md}"
    );
}

#[test]
fn hover_on_undocumented_code_is_unchanged() {
    let src = "fn add(a: integer, b: integer) -> integer\n  return a + b\nend\n";
    let md = hover_src(src, "fn add").expect("hover");
    assert_eq!(
        md,
        "```saule\nfn add(a: integer, b: integer) -> integer\n```"
    );
}

#[test]
fn hover_ignores_a_plain_comment_above_a_declaration() {
    let src = "-- not documentation\nfn add(a: integer) -> integer\n  return a\nend\n";
    let md = hover_src(src, "fn add").expect("hover");
    assert!(
        !md.contains("not documentation"),
        "plain `--` comment leaked into hover:\n{md}"
    );
}

#[test]
fn hover_shows_enum_and_variant_docs() {
    let src = "\
--- Which way something faces.
enum Direction
  --- Toward the top of the screen.
  North
  South
end
";
    let enum_md = hover_src(src, "enum Direction").expect("enum hover");
    assert!(
        enum_md.contains("Which way something faces."),
        "missing enum summary in:\n{enum_md}"
    );

    let variant_md = hover_src(src, "North").expect("variant hover");
    assert!(
        variant_md.contains("Toward the top of the screen."),
        "missing variant summary in:\n{variant_md}"
    );
}

#[test]
fn hover_shows_interface_method_docs() {
    let src = "\
--- Anything that can be drawn.
interface Drawable
  --- Render to the active surface.
  fn draw(x: integer)
end
";
    let md = hover_src(src, "fn draw").expect("interface method hover");
    assert!(
        md.contains("Render to the active surface."),
        "missing interface method summary in:\n{md}"
    );
}

#[test]
fn hover_renders_a_return_tag() {
    let src = "\
--- Adds two numbers.
--- @return Their sum.
fn add(a: integer, b: integer) -> integer
  return a + b
end
";
    let md = hover_src(src, "fn add").expect("hover");
    assert!(
        md.contains("**Returns** — Their sum."),
        "missing @return in:\n{md}"
    );
}

/// A wildcard import must advertise exactly what the module loader
/// actually brings into scope. Before this test, the hover blurb
/// listed every top-level declaration regardless of `export`, so a
/// private helper class appeared in "brings into scope" and then
/// failed to resolve at the use site.
#[test]
fn wildcard_import_blurb_lists_only_exported_names() {
    init_stdlib();
    let dir = std::env::temp_dir().join(format!("saule-lsp-export-vis-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    std::fs::write(
        dir.join("json.sau"),
        "\
class JsonParser
  fn parse(s: string) -> nothing
  end
end

export class Json
  static fn decode(source: string) -> table?
    return nil
  end
end

fn helper() -> nothing
end

export fn encode(v: table) -> string
  return \"\"
end
",
    )
    .unwrap();

    let app_src = "import * from \"json\"\n\nfn run() -> nothing\nend\n";
    let tokens = saule_lexer::Lexer::new(app_src).tokenize().unwrap();
    let module = saule_parser::parse(tokens).unwrap();
    let seed = saule_interpreter::module::collect_import_seed(&module, &dir);
    let _ = saule_semantic::analyze_with_seed(&module, seed);
    let imports = build_import_context(&module, app_src, Some(&dir));

    let pos = app_src.find("json\"").unwrap();
    let md = hover_at_with_source(&module, app_src, pos, &imports)
        .map(|(md, _)| md)
        .expect("import hover");

    assert!(md.contains("Json"), "exported class missing:\n{md}");
    assert!(md.contains("encode"), "exported fn missing:\n{md}");
    assert!(
        !md.contains("JsonParser"),
        "private class leaked into the import blurb:\n{md}"
    );
    assert!(
        !md.contains("helper"),
        "private fn leaked into the import blurb:\n{md}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ─── receivers behind `!`, `?.` and `as` ────────────────────────────────

/// The source every test below hovers into: a nullable local that has to
/// be unwrapped before its fields can be reached, which is the shape all
/// of the framework's tree-walking code is written in.
const UNWRAP_CHAIN: &str = "\
class Data
  theme: string
  fn init()
    self.theme = \"\"
  end
end
class Element
  data: Data
  parent: Element?
  fn init()
    self.data = Data()
    self.parent = nil
  end
end
fn walk(node: Element?) -> string
  local current: Element? = node
  local found: string = current!.data.theme
  return found
end
";

#[test]
fn a_member_reached_through_a_force_unwrap_resolves() {
    // Regression: `receiver_class` had no `ForceUnwrap` arm, so
    // `current!.data` fell through to `None` and the hover reported the
    // *enclosing function* instead — actively misleading, since it named
    // a symbol that has nothing to do with the token under the cursor.
    let md = hover(UNWRAP_CHAIN, "data.theme").expect("hover on `data`");
    assert!(
        md.contains("Data"),
        "expected the field's declared type, got:\n{md}"
    );
    assert!(
        !md.contains("fn walk"),
        "fell back to the enclosing function:\n{md}"
    );
}

#[test]
fn a_field_two_levels_past_a_force_unwrap_resolves() {
    let md = hover_at_offset(UNWRAP_CHAIN, "data.theme", 6).expect("hover on `theme`");
    assert!(
        md.contains("theme"),
        "expected the `theme` field, got:\n{md}"
    );
    assert!(
        !md.contains("fn walk"),
        "fell back to the enclosing function:\n{md}"
    );
}

#[test]
fn a_member_reached_through_a_safe_chain_resolves() {
    let src = UNWRAP_CHAIN.replace("current!.data.theme", "current?.data!.theme");
    let md = hover_at_offset(&src, "data!.theme", 6).expect("hover on `theme`");
    assert!(
        md.contains("theme"),
        "expected the `theme` field through `?.`, got:\n{md}"
    );
}

#[test]
fn a_member_reached_through_a_cast_resolves() {
    // `as T` names the receiver's class outright, so the chain past it is
    // exactly as resolvable as a declared local.
    let src = "\
class Data
  theme: string
  fn init()
    self.theme = \"\"
  end
end
fn pick(bag: any) -> string
  return (bag as Data)!.theme
end
";
    let md = hover_at_offset(src, "!.theme", 2).expect("hover on `theme`");
    assert!(
        md.contains("theme"),
        "expected the `theme` field through `as`, got:\n{md}"
    );
}

#[test]
fn a_member_the_receiver_does_not_declare_says_so() {
    // Regression: `expr_md` answered `None` for an unresolvable member,
    // and because `record` keeps the *narrowest* span containing the
    // cursor, recording nothing here let the enclosing `fn` win — so
    // hovering a typo'd field described an unrelated function with full
    // confidence. Name the miss instead; it also stops the fallback.
    let src = UNWRAP_CHAIN.replace("current!.data.theme", "current!.data.nope");
    let md = hover_at_offset(&src, "data.nope", 6).expect("hover on `nope`");
    assert!(
        md.contains("nope") && md.contains("Data"),
        "expected the miss to be named, got:\n{md}"
    );
    assert!(
        !md.contains("fn walk"),
        "fell back to the enclosing function:\n{md}"
    );
}

#[test]
fn a_method_the_receiver_does_not_declare_says_so() {
    let src = UNWRAP_CHAIN.replace("current!.data.theme", "current!.data.nope()");
    let md = hover_at_offset(&src, "data.nope", 6).expect("hover on `nope`");
    assert!(
        md.contains("nope") && md.contains("Data"),
        "expected the miss to be named, got:\n{md}"
    );
    assert!(
        !md.contains("fn walk"),
        "fell back to the enclosing function:\n{md}"
    );
}

#[test]
fn a_member_that_does_resolve_is_unaffected() {
    // The miss path must not shadow the real one: a declared field still
    // renders as a field, with its type.
    let md = hover_at_offset(UNWRAP_CHAIN, "data.theme", 6).expect("hover on `theme`");
    assert!(
        md.contains("(field)") && md.contains("string"),
        "expected the declared field, got:\n{md}"
    );
    assert!(
        !md.contains("No member"),
        "a declared field was reported as unknown:\n{md}"
    );
}

/// A call to a free function declared in the same file resolves to its
/// signature — free functions never reach the semantic registries, so
/// this path goes through the walker's module-level `fn` index.
#[test]
fn hover_call_to_sibling_free_function() {
    let src = "fn greet(name: string) -> string\n\treturn name\nend\n\nlocal g = greet(\"a\")\n";
    let md = hover_src_at(src, "greet(\"a\")", 1).expect("hover on call site");
    assert!(md.contains("fn greet(name: string) -> string"), "{md}");
}

/// Same, for a generic function: the rendered signature keeps its type
/// parameters and the generic types in the parameter list.
#[test]
fn hover_call_to_sibling_generic_function() {
    let src = "fn map<T, U>(items: table<T>, f: fn(T) -> U) -> table<U>\n\tlocal out: table<U> = {}\n\treturn out\nend\n\nlocal lengths = map({\"a\"}, s => #s)\n";
    let md = hover_src_at(src, "map({", 1).expect("hover on generic call site");
    assert!(
        md.contains("fn map<T, U>(items: table<T>, f: fn(T) -> U) -> table<U>"),
        "{md}"
    );
}

/// A local initialized from a generic sibling function gets the
/// *instantiated* return type, with the callback's result type bound
/// from its body.
#[test]
fn local_type_from_generic_sibling_call() {
    let src = "fn map<T, U>(items: table<T>, f: fn(T) -> U) -> table<U>\n\tlocal out: table<U> = {}\n\treturn out\nend\n\nlocal lengths = map({\"a\"}, s => #s)\n";
    let md = hover_src_at(src, "lengths =", 1).expect("hover on local");
    assert!(md.contains("(local) lengths: table<integer>"), "{md}");
}

/// A type parameter the call never pins down must not escape into the
/// hover as if it were a real type — the local falls back to `any`.
#[test]
fn unbound_type_param_does_not_escape_into_local_type() {
    let src = "fn make<T>(n: integer) -> table<T>\n\tlocal out: table<T> = {}\n\treturn out\nend\n\nlocal xs = make(3)\n";
    let md = hover_src_at(src, "xs =", 1).expect("hover on local");
    assert!(md.contains("(local) xs: any"), "{md}");
}

// ─── anchoring: a declaration answers for its head, not its body ─────────────

/// The regression that motivated anchoring declarations to their name
/// token. A comment inside a function body used to fall through to the
/// enclosing `fn` — the popup then described a symbol several lines
/// away with full confidence, which is what made hover feel random.
#[test]
fn a_comment_inside_a_body_hovers_nothing() {
    let src = "\
fn build(n: integer) -> integer
  -- the child of this widget
  return n
end
";
    assert_eq!(hover_src_at(src, "child of", 1), None);
}

/// Same fallback, one level out: prose between two members of a class
/// used to render the whole class blurb.
#[test]
fn a_comment_between_class_members_hovers_nothing() {
    let src = "\
class Panel
  x: integer = 0
  -- a note about layout
  y: integer = 0
end
";
    assert_eq!(hover_src_at(src, "note about", 1), None);
}

/// Anchoring must not cost the declaration its own hover: the keyword,
/// any modifier, and the name all still answer.
#[test]
fn a_declaration_still_hovers_across_its_whole_head() {
    // `export` sits outside the decl's own span, so it is not a hover
    // target here and never was — the head under test is `fn` onward.
    let src = "export fn add(a: integer) -> integer\n  return a\nend\n";
    for needle in ["fn add", "add("] {
        let md = hover_src_at(src, needle, 1).unwrap_or_else(|| panic!("no hover on {needle}"));
        assert!(md.contains("fn add(a: integer) -> integer"), "got: {md}");
    }
}

/// A literal has nothing to say, and saying it anyway means a hover
/// fires while the cursor rests in the middle of a sentence of prose.
#[test]
fn a_string_literal_hovers_nothing() {
    let src = "fn f() -> nothing\n  local s: string = \"some prose here\"\nend\n";
    assert_eq!(hover_src_at(src, "prose", 1), None);
}

// ─── named arguments ────────────────────────────────────────────────────────

/// A named-argument key is qualified by its callee. Two `child:` keys in
/// one expression belong to different widgets and must say so.
#[test]
fn named_arg_keys_name_the_callee_they_belong_to() {
    let src = "\
class Box
  fn init(child: string, pad: integer = 0)
  end
end

class Frame
  fn init(child: string)
  end
end

fn build() -> nothing
  local a = Box(child: \"x\", pad: 2)
  local b = Frame(child: \"y\")
end
";
    let outer = hover_src_at(src, "child: \"x\"", 1).expect("hover on Box key");
    assert_eq!(
        outer,
        "```saule\n(parameter) Box.child: string\n```",
        "got: {outer}"
    );
    let inner = hover_src_at(src, "child: \"y\"", 1).expect("hover on Frame key");
    assert_eq!(
        inner,
        "```saule\n(parameter) Frame.child: string\n```",
        "got: {inner}"
    );
    // Defaults are marked the same way the declaration site marks them.
    let pad = hover_src_at(src, "pad: 2", 1).expect("hover on pad key");
    assert!(pad.contains("Box.pad: integer = …"), "got: {pad}");
}

/// A key the callee has no parameter for names the miss rather than
/// staying silent — silence would let the enclosing call answer in its
/// place, which is the failure mode this whole area is about.
#[test]
fn an_unknown_named_arg_key_says_so() {
    let src = "\
class Box
  fn init(child: string)
  end
end

fn build() -> nothing
  local a = Box(chidl: \"x\")
end
";
    let md = hover_src_at(src, "chidl", 1).expect("hover on typo'd key");
    assert!(md.contains("(unknown) Box.chidl"), "got: {md}");
}

/// An `@param` line on the callee reaches the hover on the key, so the
/// prose lives next to the declaration and surfaces at the call site.
#[test]
fn a_named_arg_key_carries_the_param_doc() {
    let src = "\
--- Wraps a child.
--- @param child What to wrap.
fn wrap(child: string) -> string
  return child
end

fn build() -> nothing
  local a = wrap(child: \"x\")
end
";
    let md = hover_ctx_at(src, "child: \"x\"", 1).expect("hover on key");
    assert!(md.contains("wrap.child: string"), "got: {md}");
    assert!(md.contains("What to wrap."), "got: {md}");
    // The function's own summary belongs on the function, not on every
    // parameter inside it.
    assert!(!md.contains("Wraps a child."), "got: {md}");
}

// ─── loop variables ─────────────────────────────────────────────────────────

/// The binding site of a loop variable hovers the same as a use of it
/// inside the body. It had no span-tracked node of its own, so it was
/// the one place in the loop where hover fell through to the function.
#[test]
fn a_numeric_loop_variable_hovers_at_its_binding_site() {
    let src = "\
fn f() -> nothing
  for i: integer = 1, 10 do
print(i)
  end
end
";
    let decl = hover_src_at(src, "i: integer = 1", 0).expect("hover on binding");
    assert_eq!(decl, "```saule\n(loop var) i: integer\n```");
    let use_ = hover_src_at(src, "print(i)", "print(".len()).expect("hover on use");
    assert_eq!(decl, use_, "binding and use disagree");
}

#[test]
fn a_for_in_loop_variable_hovers_at_its_binding_site() {
    let src = "\
fn f(names: table<string>) -> nothing
  for name: string in names do
print(name)
  end
end
";
    let md = hover_src_at(src, "name: string in", 0).expect("hover on binding");
    assert_eq!(md, "```saule\n(loop var) name: string\n```");
}

/// Free functions reached through a re-export barrel resolve for
/// named-argument hover.
///
/// A barrel (`init.sau`: nothing but `import * from ...`) declares no
/// functions itself, so a single-level scan of the import target found
/// none of them and every call to one hovered its keys as a bare name
/// with no type. Classes never had this problem — they arrive through
/// the semantic registry seed, which already follows barrels.
#[test]
fn named_arg_resolves_through_a_re_export_barrel() {
    init_stdlib();
    let dir = std::env::temp_dir().join(format!("saule-lsp-hover-barrel-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("kit")).unwrap();

    std::fs::write(
        dir.join("kit").join("overlay.sau"),
        "export fn showToast(context: integer, message: string = \"\") -> nothing\nend\n",
    )
    .unwrap();
    // The barrel: re-exports, declares nothing.
    std::fs::write(
        dir.join("kit").join("init.sau"),
        "import * from overlay\n",
    )
    .unwrap();

    let app = "\
import * from kit

fn main() -> nothing
  showToast(1, message: \"hi\")
end
";
    let tokens = saule_lexer::Lexer::new(app).tokenize().unwrap();
    let module = saule_parser::parse(tokens).unwrap();
    let seed = saule_interpreter::module::collect_import_seed(&module, &dir);
    let _ = saule_semantic::analyze_with_seed(&module, seed);
    let ctx = build_import_context(&module, app, Some(&dir));

    let pos = app.find("message: \"hi\"").unwrap() + 1;
    let md = hover_at_with_source(&module, app, pos, &ctx)
        .map(|(m, _)| m)
        .expect("hover on key");
    assert_eq!(
        md,
        "```saule\n(parameter) showToast.message: string = …\n```",
        "got: {md}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The same barrel must not forward a name its target keeps private.
#[test]
fn a_barrel_does_not_forward_a_private_function() {
    init_stdlib();
    let dir = std::env::temp_dir().join(format!("saule-lsp-hover-priv-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("kit")).unwrap();

    std::fs::write(
        dir.join("kit").join("impl.sau"),
        "fn helper(seed: integer = 0) -> nothing\nend\n",
    )
    .unwrap();
    std::fs::write(dir.join("kit").join("init.sau"), "import * from impl\n").unwrap();

    let app = "import * from kit\n\nfn main() -> nothing\n  helper(seed: 1)\nend\n";
    let tokens = saule_lexer::Lexer::new(app).tokenize().unwrap();
    let module = saule_parser::parse(tokens).unwrap();
    let seed = saule_interpreter::module::collect_import_seed(&module, &dir);
    let _ = saule_semantic::analyze_with_seed(&module, seed);
    // A wildcard binds only what its target exports, at every hop.
    assert!(
        saule_semantic::lookup_function("helper").is_none(),
        "a private function crossed the barrel"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ─── closures see the scope around them ─────────────────────────────────────

/// A lambda captures the names around it, so hover inside one has to
/// see them.
///
/// The walker used to start every lambda body with an empty scope, so a
/// captured name resolved to nothing and hover fell through to the next
/// node out — the lambda itself. A cursor on `rebuild()` four lines into
/// a callback answered `(expr): fn(boolean) -> any`, describing the
/// callback rather than the token under the cursor.
#[test]
fn a_lambda_body_sees_the_enclosing_scope() {
    let src = "\
class Panel
  fn build(scratch: table) -> nothing
    local rebuild: function = fn()
      print(1)
    end
    local count: integer = 0
    self.each(fn(next: boolean)
      scratch.sound = next
      rebuild()
      print(count)
    end)
  end
  fn each(f: function) -> nothing
  end
end
";
    // Captured parameter, captured local, and the lambda's own param.
    assert_eq!(
        hover_src_at(src, "scratch.sound = next", 1).as_deref(),
        Some("```saule\n(parameter) scratch: table\n```")
    );
    assert_eq!(
        hover_src_at(src, "rebuild()\n", 1).as_deref(),
        Some("```saule\n(local) rebuild: fn() -> any\n```")
    );
    assert_eq!(
        hover_src_at(src, "print(count)", "print(".len()).as_deref(),
        Some("```saule\n(local) count: integer\n```")
    );
    assert_eq!(
        hover_src_at(src, "next\n", 0).as_deref(),
        Some("```saule\n(parameter) next: boolean\n```")
    );
}

/// A method body still does *not* see a sibling method's locals — only
/// lambdas inherit.
#[test]
fn a_method_body_does_not_see_a_siblings_locals() {
    let src = "\
class Panel
  fn a() -> nothing
    local secret: integer = 1
  end
  fn b() -> nothing
    print(secret)
  end
end
";
    let md = hover_src_at(src, "print(secret)", "print(".len());
    assert!(
        md.as_deref().is_none_or(|m| !m.contains("(local) secret")),
        "leaked a sibling method's local: {md:?}"
    );
}

// ─── enum variants are members too ──────────────────────────────────────────

/// `CrossAxisAlignment.Stretch` is a variant, not a class member. Enums
/// reach the member path on equal terms with classes, so a lookup that
/// only consulted the class registry reported `(unknown)` on a name
/// declared three lines up.
#[test]
fn an_enum_variant_hovers_as_a_variant() {
    let src = "\
enum Align
  Start
  Stretch
end

enum Event
  Click(x: integer, y: integer)
end

fn probe() -> nothing
  local a = Align.Stretch
  local e = Event.Click(1, 2)
end
";
    assert_eq!(
        hover_src_at(src, "Align.Stretch\n", "Align.".len()).as_deref(),
        Some("```saule\n(variant) Align.Stretch\n```")
    );
    // A tuple variant carries its arity.
    assert_eq!(
        hover_src_at(src, "Event.Click(1", "Event.".len()).as_deref(),
        Some("```saule\n(variant) Event.Click(_, _)\n```")
    );
    // A name the enum really doesn't have is still reported as a miss.
    let md = hover_src_at(src, "Align.Stretch\n", "Align.".len()).unwrap();
    assert!(!md.contains("(unknown)"), "got: {md}");
}

/// An untyped `table` accepts any key, so "no member" would be a false
/// statement about correct code.
#[test]
fn a_key_on_an_untyped_table_is_not_an_unknown_member() {
    let src = "fn probe(scratch: table) -> nothing\n  scratch.sound = true\nend\n";
    let md = hover_src_at(src, "scratch.sound", "scratch.".len() + 1).expect("hover");
    assert!(md.contains("(key) sound: any"), "got: {md}");
    assert!(!md.contains("No member"), "got: {md}");
}
