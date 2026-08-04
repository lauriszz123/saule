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

/// Byte range of the `n`-th (0-based) word-bounded occurrence of the
/// identifier `name`. Counting whole identifiers rather than substrings
/// is what lets a test say "the `x` in the body", not "the fourth
/// character sequence that happens to read `x`".
fn ident_span(src: &str, name: &str, n: usize) -> std::ops::Range<usize> {
    let mut spans = Vec::new();
    let bytes = src.as_bytes();
    let pat = name.as_bytes();
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut i = 0;
    while i + pat.len() <= bytes.len() {
        if &bytes[i..i + pat.len()] == pat
            && (i == 0 || !is_ident(bytes[i - 1]))
            && (i + pat.len() == bytes.len() || !is_ident(bytes[i + pat.len()]))
        {
            spans.push(i..i + pat.len());
            i += pat.len();
            continue;
        }
        i += 1;
    }
    spans
        .get(n)
        .unwrap_or_else(|| {
            panic!(
                "source has {} occurrence(s) of {name:?}, wanted #{n}",
                spans.len()
            )
        })
        .clone()
}

/// Hover with the cursor in the middle of the `n`-th occurrence of the
/// identifier `name`, through the full production path (source text plus
/// a real [`ImportContext`]) — what an editor actually asks for.
fn hover_ident(src: &str, name: &str, n: usize) -> Option<String> {
    let span = ident_span(src, name, n);
    hover_offset(src, span.start + (span.end - span.start) / 2)
}

fn hover_offset(src: &str, offset: usize) -> Option<String> {
    init_stdlib();
    let tokens = saule_lexer::Lexer::new(src).tokenize().expect("lex");
    let module = saule_parser::parse(tokens).expect("parse");
    let _ = saule_semantic::analyze(&module);
    let ctx = build_import_context(&module, src, None);
    hover_at_with_source(&module, src, offset, &ctx).map(|(md, _)| md)
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
    assert_eq!(md, "```saule\n(parameter) Main.put.count: integer = …\n```");
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

/// A local initialized from a `when(...)` chain of generic stages gets
/// the type the *chain* produces. Reading the last stage's declared
/// return alone reported `table<U>` — the parameter name of a function
/// the call site had already instantiated.
#[test]
fn local_type_from_generic_pipeline() {
    let src = concat!(
        "fn map<T, U>(items: table<T>, f: fn(T) -> U) -> table<U>\n",
        "\tlocal out: table<U> = {}\n\treturn out\nend\n\n",
        "fn filter<T>(items: table<T>, p: fn(T) -> boolean) -> table<T>\n",
        "\treturn items\nend\n\n",
        "local doubled = when({1, 2, 3}):filter(x => x % 2 == 0):map(x => x * 2)\n",
    );
    let md = hover_src_at(src, "doubled =", 1).expect("hover on local");
    assert!(md.contains("(local) doubled: table<integer>"), "{md}");
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
        outer, "```saule\n(parameter) Box.child: string\n```",
        "got: {outer}"
    );
    let inner = hover_src_at(src, "child: \"y\"", 1).expect("hover on Frame key");
    assert_eq!(
        inner, "```saule\n(parameter) Frame.child: string\n```",
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
    std::fs::write(dir.join("kit").join("init.sau"), "import * from overlay\n").unwrap();

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
        md, "```saule\n(parameter) showToast.message: string = …\n```",
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
    // A tuple variant shows its payload named and typed, which is what tells
    // you the order to destructure it in.
    assert_eq!(
        hover_src_at(src, "Event.Click(1", "Event.".len()).as_deref(),
        Some("```saule\n(variant) Event.Click(x: integer, y: integer)\n```")
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

/// A loop variable takes its type from what the iterable yields.
///
/// `for item in items` over a `table<T>` is the ordinary way to write a
/// loop, and an unannotated variable used to default to `any` — the one
/// answer hover already had enough in hand to rule out.
#[test]
fn a_loop_variable_is_typed_by_what_it_iterates() {
    let src = "\
fn filter<T>(items: table<T>) -> nothing
  for item in items do
    print(item)
  end
end

fn pairs(scores: table<string, integer>) -> nothing
  for name, score in scores do
    print(name)
  end
end

fn indexed(names: table<string>) -> nothing
  for i, n in names do
    print(i)
  end
end
";
    // The generic element type, at the binding and at a use.
    let ty = |needle: &str, delta: usize| {
        hover_src_at(src, needle, delta).unwrap_or_else(|| panic!("no hover on {needle}"))
    };
    assert_eq!(ty("item in items", 0), "```saule\n(loop var) item: T\n```");
    assert_eq!(
        ty("print(item)", "print(".len()),
        "```saule\n(loop var) item: T\n```"
    );

    // Two variables over `table<K, V>` take key then value.
    assert_eq!(
        ty("name, score in", 0),
        "```saule\n(loop var) name: string\n```"
    );
    assert_eq!(
        ty("score in scores", 0),
        "```saule\n(loop var) score: integer\n```"
    );

    // Array-style `table<V>` has no declared key, so the index is an integer.
    assert_eq!(
        ty("i, n in names", 0),
        "```saule\n(loop var) i: integer\n```"
    );
    assert_eq!(
        ty("n in names do\n    print(i)", 0),
        "```saule\n(loop var) n: string\n```"
    );
}

/// An explicit ascription is the author's word on the matter and is not
/// second-guessed; an iterable that says nothing still yields `any`.
#[test]
fn loop_variable_inference_defers_to_annotations() {
    let src = "\
fn annotated(names: table<string>) -> nothing
  for n: any in names do
    print(n)
  end
end

fn untyped(bag: table) -> nothing
  for v in bag do
    print(v)
  end
end
";
    assert_eq!(
        hover_src_at(src, "n: any in", 0).as_deref(),
        Some("```saule\n(loop var) n: any\n```")
    );
    assert_eq!(
        hover_src_at(src, "v in bag", 0).as_deref(),
        Some("```saule\n(loop var) v: any\n```")
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// Lambdas
// ──────────────────────────────────────────────────────────────────────────────

/// A lambda parameter hovers as a parameter, at its binding site and at
/// every use inside the body.
#[test]
fn a_lambda_parameter_hovers_at_its_binding_and_its_uses() {
    let src = "\
fn main() -> nothing
  local double: fn(integer) -> integer = fn(x: integer)
    return x * 2
  end
  print(double(2))
end
";
    assert_eq!(
        hover_ident(src, "x", 0).as_deref(),
        Some("```saule\n(parameter) x: integer\n```")
    );
    assert_eq!(
        hover_ident(src, "x", 1).as_deref(),
        Some("```saule\n(parameter) x: integer\n```")
    );
}

/// A class named in a lambda parameter's type ascription resolves to the
/// class, the same way it does on a `fn` declaration's parameter.
#[test]
fn a_lambda_param_type_ascription_resolves() {
    let src = "\
class Storage
  fn put(v: integer) -> nothing
  end
end

fn main() -> nothing
  local save: fn(Storage) -> nothing = fn(s: Storage)
    s.put(1)
  end
end
";
    let md = hover_ident(src, "Storage", 2).expect("hover on the lambda param's type");
    assert!(md.contains("class Storage"), "got: {md}");
}

/// The lambda's own return-type annotation resolves too.
#[test]
fn a_lambda_return_type_ascription_resolves() {
    let src = "\
class Storage
  fn init()
  end
end

fn main() -> nothing
  local make: fn() -> Storage = fn() -> Storage
    return Storage()
  end
end
";
    let md = hover_ident(src, "Storage", 2).expect("hover on the lambda's return type");
    assert!(md.contains("class Storage"), "got: {md}");
}

/// A lambda parameter shadows an enclosing local of the same name only
/// inside the lambda.
#[test]
fn a_lambda_param_shadows_an_enclosing_local() {
    let src = "\
fn main() -> nothing
  local value: string = \"outer\"
  local f: fn(integer) -> integer = fn(value: integer)
    return value + 1
  end
  print(value)
end
";
    assert_eq!(
        hover_ident(src, "value", 2).as_deref(),
        Some("```saule\n(parameter) value: integer\n```"),
        "inside the lambda the parameter wins"
    );
    assert_eq!(
        hover_ident(src, "value", 3).as_deref(),
        Some("```saule\n(local) value: string\n```"),
        "past the lambda the enclosing local is visible again"
    );
}

/// A lambda passed inline as a call argument still gets a scope of its
/// own — the parameter hovers as a parameter, not as whatever the name
/// means outside.
#[test]
fn a_lambda_argument_binds_its_own_parameters() {
    let src = "\
fn apply(items: table<integer>, f: fn(integer) -> integer) -> nothing
end

fn main() -> nothing
  apply({1, 2}, fn(n: integer)
    return n * 2
  end)
end
";
    assert_eq!(
        hover_ident(src, "n", 1).as_deref(),
        Some("```saule\n(parameter) n: integer\n```")
    );
}

/// A local declared inside a block-bodied lambda does not leak out of it.
#[test]
fn a_lambda_body_local_does_not_leak_out() {
    let src = "\
fn main() -> nothing
  local run: fn() -> integer = fn()
    local scratch: integer = 1
    return scratch
  end
  print(scratch)
end
";
    assert_eq!(
        hover_ident(src, "scratch", 1).as_deref(),
        Some("```saule\n(local) scratch: integer\n```"),
        "inside the lambda"
    );
    let after = hover_ident(src, "scratch", 2);
    assert!(
        after.as_deref().is_none_or(|m| !m.contains("(local)")),
        "leaked the lambda's local past its body: {after:?}"
    );
}

/// A `return` inside a lambda belongs to the lambda, not to the
/// function the lambda was written in.
#[test]
fn a_return_inside_a_lambda_reports_the_lambdas_own_type() {
    let src = "\
fn main() -> string
  local count: fn() -> integer = fn() -> integer
    return 1
  end
  return \"done\"
end
";
    assert_eq!(
        hover_offset(src, src.find("return 1").expect("inner return") + 1).as_deref(),
        Some("```saule\n(return) -> integer\n```"),
        "the lambda's own annotation, not the enclosing function's"
    );
    assert_eq!(
        hover_offset(src, src.rfind("return \"done\"").expect("outer return") + 1).as_deref(),
        Some("```saule\n(return) -> string\n```"),
        "the enclosing function's own return is unaffected"
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// Pipes
// ──────────────────────────────────────────────────────────────────────────────

/// A pipe stage is a call to a free function, so its name hovers as that
/// function — not as the type of the pipeline it sits in.
#[test]
fn a_pipe_stage_name_hovers_as_the_function_it_calls() {
    let src = "\
fn shout(msg: string) -> string
  return msg .. \"!\"
end

fn main() -> nothing
  print(when(\"hi\"):shout())
end
";
    let md = hover_ident(src, "shout", 1).expect("hover on the stage name");
    assert!(md.contains("fn shout"), "got: {md}");
    assert!(md.contains("msg: string"), "got: {md}");
}

/// An argument passed to a stage is an ordinary expression.
#[test]
fn a_pipe_stage_argument_hovers_as_itself() {
    let src = "\
fn repeatStr(msg: string, times: integer) -> string
  return msg
end

fn main() -> nothing
  local count: integer = 3
  print(when(\"hi\"):repeatStr(count))
end
";
    assert_eq!(
        hover_ident(src, "count", 1).as_deref(),
        Some("```saule\n(local) count: integer\n```")
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// try / catch
// ──────────────────────────────────────────────────────────────────────────────

/// The catch variable hovers at its binding site, the way a loop
/// variable and a parameter do — not only where it is used.
#[test]
fn a_catch_variable_hovers_at_its_binding_site() {
    let src = "\
fn main() -> nothing
  try
    throw \"boom\"
  catch err: string
    print(err)
  end
end
";
    assert_eq!(
        hover_ident(src, "err", 0).as_deref(),
        Some("```saule\n(error) err: string\n```"),
        "at the `catch` clause"
    );
    assert_eq!(
        hover_ident(src, "err", 1).as_deref(),
        Some("```saule\n(error) err: string\n```"),
        "in the catch body"
    );
}

/// A class named as the caught type resolves to the class.
#[test]
fn a_catch_type_ascription_resolves() {
    let src = "\
class ParseError
  fn init()
  end
end

fn main() -> nothing
  try
    throw ParseError()
  catch e: ParseError
    print(\"failed\")
  end
end
";
    let md = hover_ident(src, "ParseError", 2).expect("hover on the caught type");
    assert!(md.contains("class ParseError"), "got: {md}");
}

/// The catch variable is scoped to the catch block.
#[test]
fn a_catch_variable_does_not_leak_past_the_block() {
    let src = "\
fn main() -> nothing
  try
    throw \"boom\"
  catch err: string
    print(err)
  end
  print(err)
end
";
    let after = hover_ident(src, "err", 2);
    assert!(
        after.as_deref().is_none_or(|m| !m.contains("(error)")),
        "leaked the catch binding past its block: {after:?}"
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// Type ascriptions the walker has to find by scanning
// ──────────────────────────────────────────────────────────────────────────────

/// Multi-binding locals carry ascriptions too.
#[test]
fn a_multi_binding_local_hovers_its_names_and_types() {
    let src = "\
class Point
  fn init()
  end
end

fn make() -> (Point, Point)
  return Point(), Point()
end

fn main() -> nothing
  local a: Point, b: Point = make()
  print(a)
end
";
    assert_eq!(
        hover_ident(src, "a", 0).as_deref(),
        Some("```saule\n(local) a: Point\n```")
    );
    let md = hover_ident(src, "Point", 5).expect("hover on the first binding's type");
    assert!(md.contains("class Point"), "got: {md}");
}

/// An interface method's return type resolves, like its parameter types
/// already do.
#[test]
fn an_interface_method_return_type_resolves() {
    let src = "\
class Storage
  fn init()
  end
end

interface Factory
  fn build(seed: Storage) -> Storage
end
";
    let param_ty = hover_ident(src, "Storage", 1).expect("hover on the param type");
    assert!(param_ty.contains("class Storage"), "got: {param_ty}");
    let return_ty = hover_ident(src, "Storage", 2).expect("hover on the return type");
    assert!(return_ty.contains("class Storage"), "got: {return_ty}");
}

/// An enum's tuple-variant payload type resolves.
#[test]
fn an_enum_payload_type_ascription_resolves() {
    let src = "\
class Point
  fn init()
  end
end

enum Event
  Click(at: Point),
  Quit
end
";
    let md = hover_ident(src, "Point", 1).expect("hover on the payload type");
    assert!(md.contains("class Point"), "got: {md}");
}

// ──────────────────────────────────────────────────────────────────────────────
// Members through inheritance and containers
// ──────────────────────────────────────────────────────────────────────────────

/// A method declared on the parent hovers with the parent's signature
/// when reached through a subclass instance.
#[test]
fn an_inherited_method_hovers_through_a_subclass_receiver() {
    let src = "\
class Base
  fn greet() -> string
    return \"hello\"
  end
end

class Child extends Base
end

fn main() -> nothing
  local c: Child = Child()
  print(c.greet())
end
";
    let md = hover_ident(src, "greet", 1).expect("hover on the inherited call");
    assert!(md.contains("greet"), "got: {md}");
    assert!(md.contains("-> string"), "got: {md}");
    assert!(!md.contains("(unknown)"), "got: {md}");
}

/// Likewise for an inherited field.
#[test]
fn an_inherited_field_hovers_through_self_in_the_subclass() {
    let src = "\
class Base
  label: string = \"base\"
end

class Child extends Base
  fn shout() -> string
    return self.label
  end
end
";
    let md = hover_ident(src, "label", 1).expect("hover on the inherited field");
    assert!(md.contains(": string"), "got: {md}");
    assert!(!md.contains("(unknown)"), "got: {md}");
}

/// A member read off an element of a `table<Class>`.
#[test]
fn a_member_of_an_indexed_element_resolves() {
    let src = "\
class Item
  name: string = \"\"
end

fn first(items: table<Item>) -> string
  return items[1].name
end
";
    let md = hover_ident(src, "name", 1).expect("hover on the indexed element's field");
    assert!(md.contains("Item.name"), "got: {md}");
}

/// A static field reached through the class name.
#[test]
fn a_static_field_hovers_through_the_class_name() {
    let src = "\
class Config
  static local retries: integer = 3

  static fn get() -> integer
    return Config.retries
  end
end
";
    let md = hover_ident(src, "retries", 1).expect("hover on the static field");
    assert!(md.contains("retries"), "got: {md}");
    assert!(!md.contains("(unknown)"), "got: {md}");
}

/// `self` inside an enum method names the enum.
#[test]
fn self_inside_an_enum_method_names_the_enum() {
    let src = "\
enum Status
  Ok,
  Err

  fn describe() -> string
    return match self
      case Status.Ok then \"ok\"
      case Status.Err then \"err\"
    end
  end
end
";
    let md = hover_ident(src, "self", 0).expect("hover on self");
    assert!(md.contains("Status"), "got: {md}");
}

// ──────────────────────────────────────────────────────────────────────────────
// Shadowing
// ──────────────────────────────────────────────────────────────────────────────

/// A parameter shadows a field of the same name; `self.size` still names
/// the field.
#[test]
fn a_method_param_shadows_a_field_of_the_same_name() {
    let src = "\
class Box
  size: integer = 0

  fn resize(size: integer) -> nothing
    self.size = size
  end
end
";
    assert_eq!(
        hover_ident(src, "size", 3).as_deref(),
        Some("```saule\n(parameter) size: integer\n```"),
        "the assignment's right-hand side is the parameter"
    );
    let field = hover_ident(src, "size", 2).expect("hover on self.size");
    assert!(field.contains("Box.size"), "got: {field}");
}

/// A local declared in an inner block shadows the outer one, and the
/// outer one is back after the block.
#[test]
fn an_inner_block_local_shadows_and_then_restores() {
    let src = "\
fn main() -> nothing
  local n: integer = 1
  if n > 0 then
    local n: string = \"inner\"
    print(n)
  end
  print(n)
end
";
    assert_eq!(
        hover_ident(src, "n", 3).as_deref(),
        Some("```saule\n(local) n: string\n```")
    );
    assert_eq!(
        hover_ident(src, "n", 4).as_deref(),
        Some("```saule\n(local) n: integer\n```")
    );
}

/// A `repeat … until` condition can see the body's locals, so hover
/// must too.
#[test]
fn a_repeat_until_condition_sees_the_body_locals() {
    let src = "\
fn main() -> nothing
  repeat
    local done: boolean = true
  until done
end
";
    assert_eq!(
        hover_ident(src, "done", 1).as_deref(),
        Some("```saule\n(local) done: boolean\n```")
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// Expressions that only get visited as someone else's child
// ──────────────────────────────────────────────────────────────────────────────

/// The class named by an `as` cast resolves.
#[test]
fn a_cast_target_type_resolves() {
    let src = "\
class Storage
  fn init()
  end
end

fn pick(bag: any) -> nothing
  local s: Storage? = bag as Storage
end
";
    let md = hover_ident(src, "Storage", 2).expect("hover on the cast's target type");
    assert!(md.contains("class Storage"), "got: {md}");
}

/// A parameter default is an expression like any other.
#[test]
fn a_param_default_expression_hovers() {
    let src = "\
class Limits
  static local max: integer = 10
end

fn clamp(value: integer, ceiling: integer = Limits.max) -> integer
  return value
end
";
    let md = hover_ident(src, "max", 1).expect("hover inside the default");
    assert!(md.contains("max"), "got: {md}");
    assert!(!md.contains("(unknown)"), "got: {md}");
}

/// So is a field's default.
#[test]
fn a_field_default_expression_hovers() {
    let src = "\
class Limits
  static local max: integer = 10
end

class Box
  ceiling: integer = Limits.max
end
";
    let md = hover_ident(src, "max", 1).expect("hover inside the field default");
    assert!(md.contains("max"), "got: {md}");
    assert!(!md.contains("(unknown)"), "got: {md}");
}

/// An argument nested inside another call still hovers as itself.
#[test]
fn a_nested_call_argument_hovers_as_itself() {
    let src = "\
fn double(n: integer) -> integer
  return n * 2
end

fn main() -> nothing
  local seed: integer = 2
  print(double(double(seed)))
end
";
    assert_eq!(
        hover_ident(src, "seed", 1).as_deref(),
        Some("```saule\n(local) seed: integer\n```")
    );
}

/// A value inside a table literal hovers as itself.
#[test]
fn a_table_literal_entry_hovers_as_itself() {
    let src = "\
fn main() -> nothing
  local n: integer = 1
  local t: table<integer> = {n, n + 1}
end
";
    assert_eq!(
        hover_ident(src, "n", 1).as_deref(),
        Some("```saule\n(local) n: integer\n```")
    );
}

/// A static method's own declaration says it is static.
#[test]
fn a_static_method_declaration_reads_as_static() {
    let src = "\
class Util
  static fn twice(n: integer) -> integer
    return n * 2
  end
end
";
    let md = hover_ident(src, "twice", 0).expect("hover on the declaration");
    assert!(md.contains("static fn"), "got: {md}");
    assert!(md.contains("Util.twice"), "got: {md}");
}

// ──────────────────────────────────────────────────────────────────────────────
// Negative cases
// ──────────────────────────────────────────────────────────────────────────────

/// A keyword that closes a block is not a symbol.
#[test]
fn a_block_terminator_hovers_nothing() {
    let src = "\
fn build(n: integer) -> integer
  return n
end
";
    let end_kw = src.rfind("end").expect("end");
    assert!(
        hover_offset(src, end_kw + 1).is_none(),
        "got: {:?}",
        hover_offset(src, end_kw + 1)
    );
}

/// A name that is declared nowhere must not be described as a binding
/// that exists.
#[test]
fn an_undeclared_name_is_not_reported_as_a_binding() {
    let src = "\
fn build() -> nothing
  print(mysteryValue)
end
";
    let md = hover_ident(src, "mysteryValue", 0);
    assert!(
        md.as_deref()
            .is_none_or(|m| !m.contains("(local)") && !m.contains("(parameter)")),
        "got: {md:?}"
    );
}

// ─── untyped lambda parameters take the callee's type ───────────────────────

/// A lambda parameter written without a type parses as `any`. The slot it
/// fills is the only place its real type can come from, so a pipeline
/// stage binds it from the value flowing in: `when(nums):filter(x => …)`
/// over a `table<integer>` makes `x` an `integer`.
#[test]
fn a_pipe_stage_lambda_param_takes_the_stages_element_type() {
    let src = "\
fn keep(items: table<integer>, f: fn(integer) -> boolean) -> table<integer>
  return items
end

fn main() -> nothing
  local evens = when({1, 2, 3}):keep(x => x > 1)
end
";
    assert_eq!(
        hover_ident(src, "x", 1).as_deref(),
        Some("```saule\n(parameter) x: integer\n```")
    );
}

/// The generic form is the one that matters in practice — `T` is bound
/// from the piped value, so the predicate's parameter is concrete.
#[test]
fn a_generic_pipe_stage_binds_its_lambda_param() {
    let src = "\
fn filter<T>(items: table<T>, predicate: fn(T) -> boolean) -> table<T>
  return items
end

fn main() -> nothing
  local evens = when({1, 2, 3}):filter(x => x > 1)
end
";
    assert_eq!(
        hover_ident(src, "x", 1).as_deref(),
        Some("```saule\n(parameter) x: integer\n```")
    );
}

/// Same for an ordinary call argument.
#[test]
fn a_call_argument_lambda_param_takes_the_declared_type() {
    let src = "\
fn apply(items: table<string>, f: fn(string) -> integer) -> integer
  return 0
end

fn main() -> nothing
  local n = apply({\"a\"}, s => #s)
end
";
    assert_eq!(
        hover_ident(src, "s", 1).as_deref(),
        Some("```saule\n(parameter) s: string\n```")
    );
}

/// An explicit annotation on the lambda always wins over the slot — only
/// an omitted one gets filled in. (A written `: any` is indistinguishable
/// from an omitted type in the AST, so the check uses a type that isn't.)
#[test]
fn an_annotated_lambda_param_is_not_overridden() {
    let src = "\
fn apply(items: table<string>, f: fn(string) -> integer) -> integer
  return 0
end

fn main() -> nothing
  local n = apply({\"a\"}, (s: integer) => s)
end
";
    assert_eq!(
        hover_ident(src, "s", 1).as_deref(),
        Some("```saule\n(parameter) s: integer\n```")
    );
}

/// A parameter the arguments never pin down stays unknown rather than
/// being reported as the signature's own parameter name.
#[test]
fn an_unbound_stage_param_does_not_leak_its_name() {
    let src = "\
fn build<T>(seed: integer, make: fn(T) -> T) -> integer
  return seed
end

fn main() -> nothing
  local n = build(1, x => x)
end
";
    let md = hover_ident(src, "x", 1);
    assert!(
        md.as_deref().is_none_or(|m| !m.contains(": T")),
        "leaked the signature's own parameter name: {md:?}"
    );
}

/// The reported case: a two-stage chain, where the second stage's lambda
/// parameter comes from the type the *first* stage produced.
#[test]
fn a_later_pipe_stage_lambda_param_follows_the_chain() {
    let src = "\
fn filter<T>(items: table<T>, predicate: fn(T) -> boolean) -> table<T>
  return items
end

fn map<T, U>(items: table<T>, f: fn(T) -> U) -> table<U>
  local out: table<U> = {}
  return out
end

fn main() -> nothing
  local doubled = when({1, 2, 3})
                  :filter(x => x % 2 == 0)
                  :map(x => x * 2)
end
";
    assert_eq!(
        hover_ident(src, "x", 3).as_deref(),
        Some("```saule\n(parameter) x: integer\n```"),
        "the `map` stage's parameter, typed by what `filter` produced"
    );
}

// ── Trailing blocks ─────────────────────────────────────────────────────────

/// A trailing block's parameter reads as the callee declared it, not as the
/// `any` an omitted annotation parses to — the same refinement an inline
/// lambda argument gets.
#[test]
fn a_trailing_block_parameter_hovers_with_the_declared_type() {
    let src = "\
fn each(items: table<integer>, body: fn(integer) -> nil) -> nil
  body(items[1])
end

fn main() -> nil
  each({1}) do (n)
    print(n)
  end
end
";
    assert_eq!(
        hover_ident(src, "n", 0).as_deref(),
        Some("```saule\n(parameter) n: integer\n```"),
        "binding site"
    );
    assert_eq!(
        hover_ident(src, "n", 1).as_deref(),
        Some("```saule\n(parameter) n: integer\n```"),
        "use inside the block"
    );
}

/// The block binds to the parameter the slot rule gives it, so a named
/// argument ahead of it must not shift its type. Here `spacing` is named, so
/// the block is `body` and `n` is its `string` parameter — reading the block
/// as slot 0 would type `n` as `integer`.
#[test]
fn a_trailing_block_after_a_named_argument_hovers_from_the_right_slot() {
    let src = "\
fn view(spacing: integer, body: fn(string) -> nil) -> nil
  body(\"x\")
end

fn main() -> nil
  view(spacing: 10) do (n)
    print(n)
  end
end
";
    assert_eq!(
        hover_ident(src, "n", 0).as_deref(),
        Some("```saule\n(parameter) n: string\n```")
    );
}

/// A trailing block body is an ordinary scope: it sees the names around it,
/// and its own parameter shadows them only inside.
#[test]
fn a_trailing_block_body_sees_the_enclosing_scope() {
    let src = "\
fn repeated(times: integer, body: fn() -> nil) -> nil
  body()
end

fn main() -> nil
  local label: string = \"hi\"
  repeated(times: 2) do
    print(label)
  end
end
";
    assert_eq!(
        hover_ident(src, "label", 1).as_deref(),
        Some("```saule\n(local) label: string\n```"),
        "captured local is visible inside the block"
    );
}

/// Hovering a named argument's key still resolves to the callee's parameter
/// when a trailing block follows it.
#[test]
fn a_named_argument_before_a_trailing_block_still_hovers_as_a_parameter() {
    let src = "\
fn view(spacing: integer, body: fn() -> nil) -> nil
  body()
end

fn main() -> nil
  view(spacing: 10) do
    print(1)
  end
end
";
    let md = hover_ident(src, "spacing", 1).expect("hover on the named-arg key");
    assert!(
        md.contains("(parameter)") && md.contains("spacing: integer"),
        "{md}"
    );
}

/// A generic callee binds its type parameters from the arguments, and the
/// trailing block's parameter follows — `n` is the element type of the table
/// that was passed, not `T`.
#[test]
fn a_trailing_block_parameter_resolves_a_generic_element_type() {
    let src = "\
fn eachOf<T>(items: table<T>, body: fn(T) -> nil) -> nil
  body(items[1])
end

fn main() -> nil
  eachOf({\"a\"}) do (n)
    print(n)
  end
end
";
    assert_eq!(
        hover_ident(src, "n", 0).as_deref(),
        Some("```saule\n(parameter) n: string\n```")
    );
}
