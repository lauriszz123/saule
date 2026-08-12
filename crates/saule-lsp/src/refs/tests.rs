//! Resolver and collector integration tests.
//!
//! The `#[cfg(test)]` gate lives on the `mod tests;` declaration in `refs.rs`;
//! repeating it here as an inner attribute is what
//! `clippy::duplicated_attributes` flags.

use super::*;
use std::ops::Range;
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

/// Byte range of the `n`-th (0-based) word-bounded occurrence of the
/// identifier `name` in `src`. Word-bounded so `n` counts real
/// identifiers rather than substrings (`item` inside `items`).
fn ident_span(src: &str, name: &str, n: usize) -> Range<usize> {
    let all = super::util::locate_words_in(src, &(0..src.len()), name);
    all.get(n)
        .unwrap_or_else(|| {
            panic!(
                "source has {} occurrence(s) of {name:?}, wanted #{n}",
                all.len()
            )
        })
        .clone()
}

/// Resolve with the cursor in the middle of the `n`-th occurrence of the
/// identifier `name` — the precise form of [`resolve`] used by tests
/// that need to point at a specific mention of a repeated name.
fn resolve_ident(src: &str, name: &str, n: usize) -> Symbol {
    try_resolve_ident(src, name, n).unwrap_or_else(|| panic!("no symbol at {name:?} #{n}"))
}

fn try_resolve_ident(src: &str, name: &str, n: usize) -> Option<Symbol> {
    let span = ident_span(src, name, n);
    try_resolve_at(src, span.start + (span.end - span.start) / 2)
}

fn try_resolve_at(src: &str, offset: usize) -> Option<Symbol> {
    init_stdlib();
    let module = parse_src(src);
    analyze(&module);
    find_symbol_at(&module, src, offset).map(|r| r.symbol)
}

fn defs(src: &str, sym: &Symbol) -> Vec<Hit> {
    defs_and_refs(src, sym)
        .into_iter()
        .filter(|h| h.is_def)
        .collect()
}

fn ref_count(src: &str, sym: &Symbol) -> usize {
    defs_and_refs(src, sym).iter().filter(|h| !h.is_def).count()
}

/// The single span goto-definition would jump to. Panics unless the
/// collector found exactly one defining site — zero means the feature is
/// broken, more than one means the symbol identity is ambiguous.
fn def_span(src: &str, sym: &Symbol) -> Range<usize> {
    let found = defs(src, sym);
    assert_eq!(
        found.len(),
        1,
        "expected exactly one definition for {sym:?}, got {found:?}"
    );
    found[0].span.clone()
}

/// Trimmed source line containing `offset` — readable assertions about
/// where goto-definition lands.
fn line_at(src: &str, offset: usize) -> &str {
    let start = src[..offset].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let end = src[offset..]
        .find('\n')
        .map(|i| i + offset)
        .unwrap_or(src.len());
    src[start..end].trim()
}

// ──────────────────────────────────────────────────────────────────────────────
// Baseline: functions, locals, imports, `self.super`
// ──────────────────────────────────────────────────────────────────────────────

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
    assert!(
        matches!(&s, Symbol::ImportPath(p) if p == "entities/Player"),
        "got: {s:?}"
    );
}

#[test]
fn resolves_bare_import_path() {
    let src = "import * from Geometry\n";
    let s = resolve(src, "Geometry");
    assert!(
        matches!(&s, Symbol::ImportPath(p) if p == "Geometry"),
        "got: {s:?}"
    );
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

// ──────────────────────────────────────────────────────────────────────────────
// Lambdas
// ──────────────────────────────────────────────────────────────────────────────

/// An arrow lambda's parameter is a local whose definition is the
/// parameter itself.
#[test]
fn resolves_arrow_lambda_param_from_its_body() {
    let src = "\
fn main()
  local double: fn(integer) -> integer = x => x * 2
  println(double(2))
end
";
    let s = resolve_ident(src, "x", 1);
    assert!(
        matches!(&s, Symbol::Local { name, .. } if name == "x"),
        "got: {s:?}"
    );
    assert_eq!(def_span(src, &s), ident_span(src, "x", 0));
    assert_eq!(ref_count(src, &s), 1);
}

/// Same for the block-bodied `fn(...) ... end` lambda form.
#[test]
fn resolves_block_lambda_param_from_its_body() {
    let src = "\
fn main()
  local size: fn(string) -> integer = fn(s)
    return #s
  end
  println(size(\"saule\"))
end
";
    let s = resolve_ident(src, "s", 1);
    assert!(
        matches!(&s, Symbol::Local { name, .. } if name == "s"),
        "got: {s:?}"
    );
    assert_eq!(def_span(src, &s), ident_span(src, "s", 0));
}

/// A lambda body closes over the enclosing scope, so a captured local
/// must resolve to that local — not be mistaken for a free function.
#[test]
fn resolves_local_captured_by_a_lambda() {
    let src = "\
fn main()
  local factor: integer = 3
  local scale: fn(integer) -> integer = x => x * factor
  println(scale(2))
end
";
    let s = resolve_ident(src, "factor", 1);
    assert!(
        matches!(&s, Symbol::Local { name, .. } if name == "factor"),
        "captured local, got: {s:?}"
    );
    assert_eq!(def_span(src, &s), ident_span(src, "factor", 0));
    assert_eq!(ref_count(src, &s), 1);
}

/// A lambda parameter shadows an enclosing function parameter of the
/// same name inside the lambda, and only inside it.
#[test]
fn lambda_param_shadows_enclosing_function_param() {
    let src = "\
fn outer(value: integer) -> integer
  local bump: fn(integer) -> integer = value => value + 1
  return bump(value)
end
";
    let inner = resolve_ident(src, "value", 2);
    assert_eq!(
        def_span(src, &inner),
        ident_span(src, "value", 1),
        "the lambda body sees the lambda's own parameter"
    );

    let outer = resolve_ident(src, "value", 3);
    assert_eq!(
        def_span(src, &outer),
        ident_span(src, "value", 0),
        "past the lambda the enclosing parameter is visible again"
    );
}

/// Two nested lambdas: the innermost binding wins, and the outer one is
/// still reachable from the outer body.
#[test]
fn resolves_innermost_of_nested_lambda_params() {
    let src = "\
fn main()
  local outer: fn(integer) -> integer = a => a + 1
  local nested: fn(integer) -> integer = a => a * 2
  println(outer(1) + nested(2))
end
";
    let first = resolve_ident(src, "a", 1);
    assert_eq!(def_span(src, &first), ident_span(src, "a", 0));
    let second = resolve_ident(src, "a", 3);
    assert_eq!(def_span(src, &second), ident_span(src, "a", 2));
}

/// A free function called from inside a lambda body still resolves to
/// its top-level declaration.
#[test]
fn resolves_function_called_from_a_lambda_body() {
    let src = "\
fn helper(v: integer) -> integer
  return v + 1
end

fn main()
  local f: fn(integer) -> integer = x => helper(x)
  println(f(1))
end
";
    let s = resolve_ident(src, "helper", 1);
    assert!(
        matches!(&s, Symbol::Function(n) if n == "helper"),
        "got: {s:?}"
    );
    assert_eq!(def_span(src, &s), ident_span(src, "helper", 0));
    assert_eq!(ref_count(src, &s), 1);
}

/// A typed lambda parameter is a real receiver: a method called on it
/// resolves to the parameter's class.
#[test]
fn resolves_method_called_on_a_lambda_param() {
    let src = "\
class Greeter
  fn hi()
  end
end

fn main()
  local run: fn(Greeter) -> nil = fn(g: Greeter)
    g.hi()
  end
end
";
    let s = resolve_ident(src, "hi", 1);
    assert!(
        matches!(&s, Symbol::Method { class, name } if class == "Greeter" && name == "hi"),
        "got: {s:?}"
    );
    assert_eq!(def_span(src, &s), ident_span(src, "hi", 0));
}

// ──────────────────────────────────────────────────────────────────────────────
// Parameters and nesting
// ──────────────────────────────────────────────────────────────────────────────

/// A parameter default may name an earlier parameter; the cursor there
/// resolves to that parameter's binding.
#[test]
fn resolves_param_referenced_in_a_later_param_default() {
    let src = "\
fn area(width: integer, height: integer = width) -> integer
  return width * height
end
";
    let s = resolve_ident(src, "width", 1);
    assert!(
        matches!(&s, Symbol::Local { name, .. } if name == "width"),
        "got: {s:?}"
    );
    assert_eq!(def_span(src, &s), ident_span(src, "width", 0));
}

/// A method parameter shadows a field of the same name: `size` is the
/// parameter, `self.size` is the field.
#[test]
fn method_param_shadows_field_of_the_same_name() {
    let src = "\
class Box
  local size: integer

  fn resize(size: integer)
    self.size = size
  end
end
";
    let param = resolve_ident(src, "size", 3);
    assert!(
        matches!(&param, Symbol::Local { name, .. } if name == "size"),
        "the assignment's rhs is the parameter, got: {param:?}"
    );
    assert_eq!(def_span(src, &param), ident_span(src, "size", 1));

    let field = resolve_ident(src, "size", 2);
    assert!(
        matches!(&field, Symbol::Field { class, name } if class == "Box" && name == "size"),
        "`self.size` is the field, got: {field:?}"
    );
    assert_eq!(def_span(src, &field), ident_span(src, "size", 0));
}

/// Two functions sharing a parameter name bind separately — a body
/// never resolves a name to a sibling's parameter.
#[test]
fn same_named_params_in_sibling_functions_stay_distinct() {
    let src = "\
local fn scale(n: integer) -> integer
  return n * 2
end

fn run(n: integer) -> integer
  return scale(n)
end
";
    let in_scale = resolve_ident(src, "n", 1);
    assert_eq!(def_span(src, &in_scale), ident_span(src, "n", 0));
    let in_run = resolve_ident(src, "n", 3);
    assert_eq!(def_span(src, &in_run), ident_span(src, "n", 2));
}

// ──────────────────────────────────────────────────────────────────────────────
// Pipes
// ──────────────────────────────────────────────────────────────────────────────

/// A `when(x):stage()` stage is an ordinary free-function call, so the
/// stage name must navigate to that function.
#[test]
fn resolves_pipe_stage_function_name() {
    let src = "\
fn shout(msg: string) -> string
  return msg .. \"!\"
end

fn main()
  println(when(\"hi\"):shout())
end
";
    let s = resolve_ident(src, "shout", 1);
    assert!(
        matches!(&s, Symbol::Function(n) if n == "shout"),
        "got: {s:?}"
    );
    assert_eq!(def_span(src, &s), ident_span(src, "shout", 0));
}

/// Extra arguments passed to a pipe stage are ordinary expressions.
#[test]
fn resolves_local_passed_as_pipe_stage_argument() {
    let src = "\
fn repeatStr(msg: string, times: integer) -> string
  return msg
end

fn main()
  local count: integer = 3
  println(when(\"hi\"):repeatStr(count))
end
";
    let s = resolve_ident(src, "count", 1);
    assert!(
        matches!(&s, Symbol::Local { name, .. } if name == "count"),
        "got: {s:?}"
    );
    assert_eq!(def_span(src, &s), ident_span(src, "count", 0));
}

// ──────────────────────────────────────────────────────────────────────────────
// `match` patterns
// ──────────────────────────────────────────────────────────────────────────────

/// A `case v then …` binding is a local defined by the pattern.
#[test]
fn resolves_match_arm_binding_from_arm_body() {
    let src = "\
fn label(n: integer) -> string
  return match n
    case 0 then \"zero\"
    case v then \"many: \" .. v
  end
end
";
    let s = resolve_ident(src, "v", 1);
    assert!(
        matches!(&s, Symbol::Local { name, .. } if name == "v"),
        "got: {s:?}"
    );
    assert_eq!(def_span(src, &s), ident_span(src, "v", 0));
    assert_eq!(ref_count(src, &s), 1);
}

/// A binding used in the arm's guard resolves the same way.
#[test]
fn resolves_match_arm_binding_from_a_guard() {
    let src = "\
fn pick(n: integer) -> string
  return match n
    case v when v > 10 then \"big\"
    case _ then \"small\"
  end
end
";
    let s = resolve_ident(src, "v", 1);
    assert_eq!(def_span(src, &s), ident_span(src, "v", 0));
}

/// Each arm binds its own `q`; the definition is the pattern of the arm
/// the cursor is in, not the first arm that happens to spell it.
#[test]
fn tuple_pattern_bindings_are_per_arm() {
    let src = "\
fn divmod(a: integer, b: integer) -> (integer, integer)
  return a / b, a % b
end

fn describe(a: integer, b: integer) -> string
  return match divmod(a, b)
    case (q, 0) then \"clean: \" .. q
    case (q, r) then q .. \" rem \" .. r
  end
end
";
    let first = resolve_ident(src, "q", 1);
    assert_eq!(def_span(src, &first), ident_span(src, "q", 0));
    let second = resolve_ident(src, "q", 3);
    assert_eq!(def_span(src, &second), ident_span(src, "q", 2));
}

/// Enum name and variant name inside a pattern resolve to their own
/// declarations, not to each other.
#[test]
fn resolves_enum_and_variant_inside_a_pattern() {
    let src = "\
enum Status
  Ok,
  Err
end

fn describe(s: Status) -> string
  return match s
    case Status.Ok then \"ok\"
    case Status.Err then \"err\"
  end
end
";
    let variant = resolve_ident(src, "Ok", 1);
    assert!(
        matches!(&variant, Symbol::EnumVariant { enum_name, variant } if enum_name == "Status" && variant == "Ok"),
        "got: {variant:?}"
    );
    assert_eq!(def_span(src, &variant), ident_span(src, "Ok", 0));

    let enum_name = resolve_ident(src, "Status", 2);
    assert!(
        matches!(&enum_name, Symbol::Enum(n) if n == "Status"),
        "got: {enum_name:?}"
    );
    assert_eq!(def_span(src, &enum_name), ident_span(src, "Status", 0));
}

/// A variant payload destructured by a pattern binds locals usable in
/// the arm body.
#[test]
fn resolves_variant_payload_binding() {
    let src = "\
enum Event
  Click(x: integer, y: integer),
  Quit
end

fn describe(e: Event) -> string
  return match e
    case Event.Click(x, y) then \"at \" .. x .. \",\" .. y
    case Event.Quit then \"bye\"
  end
end
";
    let s = resolve_ident(src, "x", 2);
    assert!(
        matches!(&s, Symbol::Local { name, .. } if name == "x"),
        "got: {s:?}"
    );
    assert_eq!(
        def_span(src, &s),
        ident_span(src, "x", 1),
        "the pattern binding, not the enum's payload field declaration"
    );
}

/// A variant referenced as a value (constructing it) points at the same
/// declaration a pattern does.
#[test]
fn resolves_enum_variant_at_a_construction_site() {
    let src = "\
enum Status
  Ok,
  Err
end

fn main()
  local s: Status = Status.Ok
  println(s)
end
";
    let s = resolve_ident(src, "Ok", 1);
    assert!(
        matches!(&s, Symbol::EnumVariant { enum_name, variant } if enum_name == "Status" && variant == "Ok"),
        "got: {s:?}"
    );
    assert_eq!(def_span(src, &s), ident_span(src, "Ok", 0));
}

// ──────────────────────────────────────────────────────────────────────────────
// Classes: fields, methods, inheritance
// ──────────────────────────────────────────────────────────────────────────────

#[test]
fn resolves_field_accessed_through_self() {
    let src = "\
class Counter
  local count: integer

  fn init()
    self.count = 0
  end

  fn bump()
    self.count = self.count + 1
  end
end
";
    let s = resolve_ident(src, "count", 2);
    assert!(
        matches!(&s, Symbol::Field { class, name } if class == "Counter" && name == "count"),
        "got: {s:?}"
    );
    assert_eq!(def_span(src, &s), ident_span(src, "count", 0));
    assert_eq!(ref_count(src, &s), 3);
}

#[test]
fn resolves_method_called_on_a_typed_local() {
    let src = "\
class Greeter
  fn hi() -> string
    return \"hello\"
  end
end

fn main()
  local g: Greeter = Greeter()
  println(g.hi())
end
";
    let s = resolve_ident(src, "hi", 1);
    assert!(
        matches!(&s, Symbol::Method { class, name } if class == "Greeter" && name == "hi"),
        "got: {s:?}"
    );
    assert_eq!(def_span(src, &s), ident_span(src, "hi", 0));
}

/// A static method reached through the class name.
#[test]
fn resolves_static_method_through_the_class_name() {
    let src = "\
class Util
  static fn twice(n: integer) -> integer
    return n * 2
  end
end

fn main()
  println(Util.twice(2))
end
";
    let s = resolve_ident(src, "twice", 1);
    assert!(
        matches!(&s, Symbol::Method { class, name } if class == "Util" && name == "twice"),
        "got: {s:?}"
    );
    assert_eq!(def_span(src, &s), ident_span(src, "twice", 0));
}

/// A static method called by bare name from a sibling method of the same
/// class. This used to resolve to a free `Symbol::Function`, which no
/// file declares — leaving goto-definition with nothing to jump to.
#[test]
fn resolves_static_method_called_by_bare_name() {
    let src = "\
class Main
  static fn help()
    println(\"usage\")
  end

  static fn main()
    help()
  end
end
";
    let s = resolve_ident(src, "help", 1);
    assert!(
        matches!(&s, Symbol::Method { class, name } if class == "Main" && name == "help"),
        "got: {s:?}"
    );
    assert_eq!(def_span(src, &s), ident_span(src, "help", 0));
    assert_eq!(ref_count(src, &s), 1, "the bare call site");
}

/// The bare-name path walks the inheritance chain, like the interpreter's
/// static lookup — the definition lives on the parent.
#[test]
fn resolves_inherited_static_method_called_by_bare_name() {
    let src = "\
class Base
  static fn shared()
    println(\"base\")
  end
end

class Child extends Base
  static fn run()
    shared()
  end
end
";
    let s = resolve_ident(src, "shared", 1);
    assert!(
        matches!(&s, Symbol::Method { class, name } if class == "Base" && name == "shared"),
        "got: {s:?}"
    );
    assert_eq!(def_span(src, &s), ident_span(src, "shared", 0));
}

/// A local shadowing a static's name still resolves to the local — bare
/// names hit locals before the class's statics.
#[test]
fn a_local_shadows_a_same_named_static_method() {
    let src = "\
class Main
  static fn help()
    println(\"usage\")
  end

  static fn main()
    local help = 1
    println(help)
  end
end
";
    let s = resolve_ident(src, "help", 2);
    assert!(matches!(&s, Symbol::Local { .. }), "got: {s:?}");
}

/// An instance method is unreachable by bare name, so such an identifier
/// must not be claimed as a reference to it.
#[test]
fn a_bare_name_does_not_resolve_to_an_instance_method() {
    let src = "\
class Greeter
  fn hi()
    println(\"hello\")
  end

  static fn run()
    hi()
  end
end
";
    let s = resolve_ident(src, "hi", 1);
    assert!(!matches!(&s, Symbol::Method { .. }), "got: {s:?}");
}

/// The class name at a construction site is a reference to the class.
#[test]
fn resolves_class_name_at_a_construction_site() {
    let src = "\
class Player
  fn init()
  end
end

fn main()
  local p: Player = Player()
  println(p)
end
";
    let s = resolve_ident(src, "Player", 2);
    assert!(
        matches!(&s, Symbol::Class(n) if n == "Player"),
        "got: {s:?}"
    );
    assert_eq!(def_span(src, &s), ident_span(src, "Player", 0));
}

/// A method inherited from a parent must navigate to where it is
/// actually declared, not to the subclass the receiver is typed as.
#[test]
fn resolves_inherited_method_to_the_declaring_class() {
    let src = "\
class Base
  fn greet() -> string
    return \"hello\"
  end
end

class Child extends Base
end

fn main()
  local c: Child = Child()
  println(c.greet())
end
";
    let s = resolve_ident(src, "greet", 1);
    assert!(
        matches!(&s, Symbol::Method { class, name } if class == "Base" && name == "greet"),
        "got: {s:?}"
    );
    assert_eq!(
        line_at(src, def_span(src, &s).start),
        "fn greet() -> string"
    );
}

/// A field inherited from a parent, reached through `self` in the
/// subclass.
#[test]
fn resolves_inherited_field_to_the_declaring_class() {
    let src = "\
class Base
  local name: string

  fn init(name: string)
    self.name = name
  end
end

class Child extends Base
  fn shout() -> string
    return self.name
  end
end
";
    let s = resolve_ident(src, "name", 4);
    assert!(
        matches!(&s, Symbol::Field { class, name } if class == "Base" && name == "name"),
        "got: {s:?}"
    );
    assert_eq!(def_span(src, &s), ident_span(src, "name", 0));
}

/// Chained field access: the receiver's class comes from the previous
/// field's declared type.
#[test]
fn resolves_field_through_a_chained_receiver() {
    let src = "\
class Inner
  value: integer

  fn init(value: integer)
    self.value = value
  end
end

class Outer
  inner: Inner

  fn init(inner: Inner)
    self.inner = inner
  end
end

fn main()
  local o: Outer = Outer(Inner(1))
  println(o.inner.value)
end
";
    let s = resolve_ident(src, "value", 4);
    assert!(
        matches!(&s, Symbol::Field { class, name } if class == "Inner" && name == "value"),
        "got: {s:?}"
    );
    assert_eq!(def_span(src, &s), ident_span(src, "value", 0));
}

/// The parent named in `extends` is a reference to that class.
#[test]
fn resolves_parent_class_in_an_extends_clause() {
    let src = "\
class Base
  fn init()
  end
end

class Child extends Base
  fn init()
    self.super()
  end
end
";
    let s = resolve_ident(src, "Base", 1);
    assert!(matches!(&s, Symbol::Class(n) if n == "Base"), "got: {s:?}");
    assert_eq!(def_span(src, &s), ident_span(src, "Base", 0));
}

/// Likewise for an interface named in `implements`.
#[test]
fn resolves_interface_in_an_implements_clause() {
    let src = "\
interface Printable
  fn toString() -> string
end

class Object implements Printable
  fn toString() -> string
    return \"Object\"
  end
end
";
    let s = resolve_ident(src, "Printable", 1);
    assert!(
        matches!(&s, Symbol::Interface(n) if n == "Printable"),
        "got: {s:?}"
    );
    assert_eq!(def_span(src, &s), ident_span(src, "Printable", 0));
}

// ──────────────────────────────────────────────────────────────────────────────
// Type annotations
// ──────────────────────────────────────────────────────────────────────────────

/// A class named in a local's type annotation navigates to the class.
#[test]
fn resolves_class_name_in_a_local_type_annotation() {
    let src = "\
class Player
  fn init()
  end
end

fn main()
  local p: Player = Player()
  println(p)
end
";
    let s = resolve_ident(src, "Player", 1);
    assert!(
        matches!(&s, Symbol::Class(n) if n == "Player"),
        "got: {s:?}"
    );
    assert_eq!(def_span(src, &s), ident_span(src, "Player", 0));
}

/// …and in a parameter's type, and in a return type.
#[test]
fn resolves_class_name_in_param_and_return_types() {
    let src = "\
class Player
  fn init()
  end
end

fn clone(p: Player) -> Player
  return p
end
";
    let param_ty = resolve_ident(src, "Player", 1);
    assert!(
        matches!(&param_ty, Symbol::Class(n) if n == "Player"),
        "got: {param_ty:?}"
    );

    let return_ty = resolve_ident(src, "Player", 2);
    assert!(
        matches!(&return_ty, Symbol::Class(n) if n == "Player"),
        "got: {return_ty:?}"
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// Statements: loops, blocks, try/catch, multi-binding
// ──────────────────────────────────────────────────────────────────────────────

#[test]
fn resolves_for_in_loop_variable() {
    let src = "\
fn main()
  local items: table<integer> = {1, 2, 3}
  for item in items do
    println(item)
  end
end
";
    let s = resolve_ident(src, "item", 1);
    assert!(
        matches!(&s, Symbol::Local { name, .. } if name == "item"),
        "got: {s:?}"
    );
    assert_eq!(def_span(src, &s), ident_span(src, "item", 0));
    assert_eq!(ref_count(src, &s), 1);
}

#[test]
fn resolves_numeric_for_loop_variable() {
    let src = "\
fn main()
  for i: integer = 1, 3 do
    println(i)
  end
end
";
    let s = resolve_ident(src, "i", 1);
    assert!(
        matches!(&s, Symbol::Local { name, .. } if name == "i"),
        "got: {s:?}"
    );
    assert_eq!(def_span(src, &s), ident_span(src, "i", 0));
}

/// The loop variable is scoped to the loop: a same-named local declared
/// after it is a different binding.
#[test]
fn loop_variable_does_not_leak_past_the_loop() {
    let src = "\
fn main()
  for i: integer = 1, 3 do
    println(i)
  end
  local i: integer = 9
  println(i)
end
";
    let inside = resolve_ident(src, "i", 1);
    assert_eq!(def_span(src, &inside), ident_span(src, "i", 0));
    let after = resolve_ident(src, "i", 3);
    assert_eq!(def_span(src, &after), ident_span(src, "i", 2));
}

#[test]
fn resolves_catch_variable() {
    let src = "\
fn main()
  try
    throw \"boom\"
  catch err: string
    println(err)
  end
end
";
    let s = resolve_ident(src, "err", 1);
    assert!(
        matches!(&s, Symbol::Local { name, .. } if name == "err"),
        "got: {s:?}"
    );
    assert_eq!(def_span(src, &s), ident_span(src, "err", 0));
    assert_eq!(ref_count(src, &s), 1);
}

/// A local inside the `try` block that happens to share the catch
/// variable's name is a separate binding — the catch variable's
/// definition is the `catch` clause itself.
#[test]
fn catch_variable_is_distinct_from_a_same_named_body_local() {
    let src = "\
fn main()
  try
    local err: string = \"inner\"
    println(err)
  catch err: string
    println(err)
  end
end
";
    let caught = resolve_ident(src, "err", 3);
    assert_eq!(
        def_span(src, &caught),
        ident_span(src, "err", 2),
        "goto-definition from the catch body lands on the catch clause"
    );

    let inner = resolve_ident(src, "err", 1);
    assert_eq!(def_span(src, &inner), ident_span(src, "err", 0));
}

/// A shadowing local in a nested block resolves to the inner binding,
/// and the outer one is visible again after the block.
#[test]
fn resolves_shadowing_local_in_a_nested_block() {
    let src = "\
fn main()
  local n: integer = 1
  if n > 0 then
    local n: integer = 2
    println(n)
  end
  println(n)
end
";
    let inner = resolve_ident(src, "n", 3);
    assert_eq!(def_span(src, &inner), ident_span(src, "n", 2));
    let outer = resolve_ident(src, "n", 4);
    assert_eq!(def_span(src, &outer), ident_span(src, "n", 0));
}

/// Multi-binding `local a: T, b: U = f()` defines each name separately.
#[test]
fn resolves_each_name_of_a_multi_binding_local() {
    let src = "\
fn divmod(a: integer, b: integer) -> (integer, integer)
  return a / b, a % b
end

fn main()
  local q: integer, r: integer = divmod(7, 2)
  println(q + r)
end
";
    let q = resolve_ident(src, "q", 1);
    assert!(
        matches!(&q, Symbol::Local { name, .. } if name == "q"),
        "got: {q:?}"
    );
    assert_eq!(def_span(src, &q), ident_span(src, "q", 0));

    let r = resolve_ident(src, "r", 1);
    assert_eq!(def_span(src, &r), ident_span(src, "r", 0));
}

/// A local referenced inside a table literal.
#[test]
fn resolves_local_used_inside_a_table_literal() {
    let src = "\
fn main()
  local n: integer = 1
  local t: table<integer> = {n, n + 1}
  println(#t)
end
";
    let s = resolve_ident(src, "n", 1);
    assert!(
        matches!(&s, Symbol::Local { name, .. } if name == "n"),
        "got: {s:?}"
    );
    assert_eq!(def_span(src, &s), ident_span(src, "n", 0));
    assert_eq!(ref_count(src, &s), 2);
}

// ──────────────────────────────────────────────────────────────────────────────
// Find-references: the same symbol identity, seen from the other side
// ──────────────────────────────────────────────────────────────────────────────

/// Every spelling of a class counts as a reference — the annotation, the
/// constructor call, and the parameter type.
#[test]
fn class_references_include_type_annotations() {
    let src = "\
class Player
  fn init()
  end
end

fn spawn(into: table<Player>) -> Player
  local p: Player = Player()
  return p
end
";
    let s = resolve_ident(src, "Player", 4);
    assert!(
        matches!(&s, Symbol::Class(n) if n == "Player"),
        "got: {s:?}"
    );
    let hits = defs_and_refs(src, &s);
    let refs: Vec<_> = hits.iter().filter(|h| !h.is_def).map(|h| &h.span).collect();
    assert_eq!(
        refs.len(),
        4,
        "table<Player>, the return type, the annotation and the call: {refs:?}"
    );
}

/// A call through the subclass and a call through the parent are
/// references to the same declaration.
#[test]
fn inherited_method_references_come_from_every_receiver() {
    let src = "\
class Base
  fn greet() -> string
    return \"hello\"
  end
end

class Child extends Base
end

fn main()
  local b: Base = Base()
  local c: Child = Child()
  println(b.greet())
  println(c.greet())
end
";
    let s = resolve_ident(src, "greet", 2);
    assert!(
        matches!(&s, Symbol::Method { class, name } if class == "Base" && name == "greet"),
        "got: {s:?}"
    );
    assert_eq!(ref_count(src, &s), 2);
}

/// A function used both as a pipe stage and as a plain call is the same
/// symbol, counted once per site.
#[test]
fn pipe_stage_and_plain_call_are_the_same_function() {
    let src = "\
fn shout(msg: string) -> string
  return msg .. \"!\"
end

fn main()
  println(when(\"hi\"):shout())
  println(shout(\"there\"))
end
";
    let s = resolve_ident(src, "shout", 1);
    assert!(
        matches!(&s, Symbol::Function(n) if n == "shout"),
        "got: {s:?}"
    );
    assert_eq!(def_span(src, &s), ident_span(src, "shout", 0));
    assert_eq!(ref_count(src, &s), 2);
}

/// A captured local's references include the uses inside the lambda.
#[test]
fn captured_local_references_include_the_lambda_body() {
    let src = "\
fn main()
  local factor: integer = 3
  local scale: fn(integer) -> integer = x => x * factor
  println(scale(factor))
end
";
    let s = resolve_ident(src, "factor", 0);
    assert_eq!(ref_count(src, &s), 2);
}

// ──────────────────────────────────────────────────────────────────────────────
// Negative cases
// ──────────────────────────────────────────────────────────────────────────────

/// Identifier-looking text inside a string literal is not a symbol.
#[test]
fn does_not_resolve_inside_a_string_literal() {
    let src = "\
class Player
  fn init()
  end
end

fn main()
  println(\"Player\")
end
";
    let inside_literal = try_resolve_ident(src, "Player", 1);
    assert!(
        !matches!(inside_literal, Some(Symbol::Class(_))),
        "a class name spelled inside a string is not a reference: {inside_literal:?}"
    );
}

/// A field on a receiver of unknown class resolves to nothing usable —
/// goto-definition must not jump into an unrelated class's field of the
/// same name.
#[test]
fn unknown_receiver_field_has_no_definition() {
    let src = "\
class Point
  x: integer

  fn init(x: integer)
    self.x = x
  end
end

fn main(anything: any)
  println(anything.x)
end
";
    let s = resolve_ident(src, "x", 4);
    if let Symbol::Field { class, .. } = &s {
        assert!(
            class != "Point",
            "an `any` receiver must not borrow Point's field: {s:?}"
        );
    }
    assert!(defs(src, &s).is_empty(), "no definition site: {s:?}");
}

// ── Trailing blocks ─────────────────────────────────────────────────────────

/// A trailing block's parameter is a symbol like any other: goto-definition
/// from a use inside the block lands on the `do (n)` binding site.
#[test]
fn trailing_block_param_resolves_to_its_binding() {
    let src = "\
fn each(items: table<integer>, body: fn(integer) -> nil) -> nil
  body(items[1])
end

fn main() -> nil
  each({1}) do (n)
    print(n)
    print(n + 1)
  end
end
";
    // Occurrence 0 is the `do (n)` binding; 1 and 2 are the uses.
    let sym = try_resolve_ident(src, "n", 1).expect("symbol on the first use");
    let span = def_span(src, &sym);
    assert_eq!(line_at(src, span.start), "each({1}) do (n)");
    assert_eq!(ref_count(src, &sym), 2, "both uses inside the block");
}

/// A local captured by a trailing block resolves to the enclosing
/// declaration, not to anything inside the block.
#[test]
fn a_capture_inside_a_trailing_block_resolves_outward() {
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
    let sym = try_resolve_ident(src, "label", 1).expect("symbol on the capture");
    let span = def_span(src, &sym);
    assert_eq!(line_at(src, span.start), "local label: string = \"hi\"");
}

/// The callee of a call carrying a trailing block is still found as a
/// reference to the function.
#[test]
fn a_call_with_a_trailing_block_counts_as_a_reference() {
    let src = "\
fn repeated(times: integer, body: fn() -> nil) -> nil
  body()
end

fn main() -> nil
  repeated(times: 1) do
    print(1)
  end
  repeated(times: 2) do
    print(2)
  end
end
";
    let sym = try_resolve_ident(src, "repeated", 0).expect("symbol on the declaration");
    assert_eq!(ref_count(src, &sym), 2, "both call sites");
}

/// A modifier chain resolves past its first link.
///
/// `Text(…).font(28.0)` was fine — its receiver is a constructor call,
/// the one call shape `receiver_class` knew. `.foregroundStyle(…)` after
/// it was not: its receiver is a call whose *callee is a member*, which
/// fell straight through to `None`, so IntelliJ answered "cannot find
/// declaration to go to" for every modifier but the first.
#[test]
fn goto_resolves_every_link_of_a_method_chain() {
    let src = "\
class Color
end

class Text
  fn font(size: float) -> Text
return self
  end
  fn foregroundStyle(color: Color) -> Text
return self
  end
  fn lineLimit(lines: integer) -> Text
return self
  end
end

fn build(c: Color)
  local t = Text().font(28.0).foregroundStyle(c).lineLimit(2)
end
";
    for (needle, n) in [("font", 1), ("foregroundStyle", 1), ("lineLimit", 1)] {
        assert_eq!(
            resolve_ident(src, needle, n),
            Symbol::Method {
                class: "Text".into(),
                name: needle.into(),
            },
            "link {needle:?} did not resolve"
        );
    }
}

/// The chain still resolves when a link is inherited rather than
/// declared on the receiver's own class — `lookup_method` walks the
/// parent chain, and the returned `Text` is what the next link needs.
#[test]
fn goto_resolves_a_chain_through_an_inherited_modifier() {
    let src = "\
class View
  fn padding(amount: float) -> View
return self
  end
end

class Text extends View
  fn font(size: float) -> Text
return self
  end
end

fn build()
  local t = Text().font(28.0).padding(4.0)
end
";
    assert_eq!(
        resolve_ident(src, "padding", 1),
        Symbol::Method {
            class: "View".into(),
            name: "padding".into(),
        }
    );
}
