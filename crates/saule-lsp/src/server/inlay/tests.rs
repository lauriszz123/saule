//! Inlay hint tests. We bypass `Backend` entirely and run the
//! walker against a parsed+analysed module, then assert on the
//! resulting `(kind, byte, label)` triples.

use super::*;

fn raw_hints(src: &str) -> Vec<(InlayHintKind, usize, String)> {
    let tokens = saule_lexer::Lexer::new(src).tokenize().expect("lex");
    let module = saule_parser::parse(tokens).expect("parse");
    let _ = saule_semantic::analyze(&module);
    let mut cx = Cx {
        source: src,
        out: Vec::new(),
        locals: Vec::new(),
        enclosing_class: None,
        user_fns: collect_user_fns(&module),
    };
    cx.visit_module(&module);
    cx.out
        .into_iter()
        .map(|h| (h.kind, h.byte, h.label))
        .collect()
}

/// A call to a top-level user function carries its declared return
/// type into the local's hint.
#[test]
fn type_hint_from_user_function_return() {
    let src = "fn first(items: table<string>) -> string\n  return items[1]\nend\n\nlocal x = first({\"a\"})\n";
    let hints = raw_hints(src);
    assert!(
        hints
            .iter()
            .any(|(k, _, l)| *k == InlayHintKind::TYPE && l == ": string"),
        "{hints:?}"
    );
}

/// A generic user function binds its type parameters from the actual
/// arguments — including the result type of an expression-bodied
/// callback.
#[test]
fn type_hint_from_generic_user_function() {
    let src = "fn map<T, U>(items: table<T>, f: fn(T) -> U) -> table<U>\n  local out: table<U> = {}\n  return out\nend\n\nlocal lengths = map({\"a\"}, s => #s)\n";
    let hints = raw_hints(src);
    assert!(
        hints
            .iter()
            .any(|(k, _, l)| *k == InlayHintKind::TYPE && l == ": table<integer>"),
        "{hints:?}"
    );
}

/// `a ?? b` drops the left side's nullability — that's the operator's
/// whole job. Hinting `: integer?` here contradicted the checker,
/// which accepts `local co: integer = maybe() ?? 0`.
#[test]
fn type_hint_for_coalesce_drops_nullability() {
    let src = "fn maybe() -> integer?\n  return 1\nend\n\nlocal co = maybe() ?? 0\n";
    let hints = raw_hints(src);
    assert!(
        hints
            .iter()
            .any(|(k, _, l)| *k == InlayHintKind::TYPE && l == ": integer"),
        "{hints:?}"
    );
}

/// `and` / `or` evaluate to an *operand*, not a boolean (Lua
/// semantics). `name() or "default"` is a `string`, and the checker
/// accepts it as one.
#[test]
fn type_hint_for_or_takes_the_operand_type() {
    let src = "fn name() -> string\n  return \"a\"\nend\n\nlocal lo = name() or \"default\"\n";
    let hints = raw_hints(src);
    assert!(
        hints
            .iter()
            .any(|(k, _, l)| *k == InlayHintKind::TYPE && l == ": string"),
        "{hints:?}"
    );
}

/// An overloaded operator names its own result type: `Vec2 + Vec2` is
/// a `Vec2`, and `Vec2 .. Vec2` is whatever `OpConcat` declares —
/// not the hardcoded `string` the built-in rule assumes.
#[test]
fn type_hint_from_operator_overload() {
    let src = concat!(
        "class Vec2 implements OpAdd<Vec2, Vec2>, OpConcat<Vec2, integer>\n",
        "  local x: float\n",
        "  fn init(x: float)\n    self.x = x\n  end\n",
        "  fn add(other: Vec2) -> Vec2\n    return Vec2(self.x)\n  end\n",
        "  fn concat(other: Vec2) -> integer\n    return 1\n  end\n",
        "end\n\n",
        "local a = Vec2(1.0)\n",
        "local sum = a + a\n",
        "local joined = a .. a\n",
    );
    let hints = raw_hints(src);
    assert!(
        hints
            .iter()
            .any(|(k, _, l)| *k == InlayHintKind::TYPE && l == ": Vec2"),
        "expected `+` to yield Vec2: {hints:?}"
    );
    assert!(
        hints
            .iter()
            .any(|(k, _, l)| *k == InlayHintKind::TYPE && l == ": integer"),
        "expected `..` overload to yield integer: {hints:?}"
    );
}

/// A `when(...)` chain threads its value type through every stage, so
/// the generic stages instantiate instead of hinting `: table<U>`.
#[test]
fn type_hint_from_generic_pipeline() {
    let src = concat!(
        "fn map<T, U>(items: table<T>, f: fn(T) -> U) -> table<U>\n",
        "  local out: table<U> = {}\n  return out\nend\n\n",
        "fn filter<T>(items: table<T>, p: fn(T) -> boolean) -> table<T>\n",
        "  return items\nend\n\n",
        "local doubled = when({1, 2, 3}):filter(x => x % 2 == 0):map(x => x * 2)\n",
    );
    let hints = raw_hints(src);
    assert!(
        hints
            .iter()
            .any(|(k, _, l)| *k == InlayHintKind::TYPE && l == ": table<integer>"),
        "{hints:?}"
    );
}

/// When the arguments never pin a type parameter down, no hint at all
/// — a label reading `: table<U>` names something the user can't act
/// on.
#[test]
fn no_type_hint_when_type_param_stays_unbound() {
    let src = "fn make<T>(n: integer) -> table<T>\n  local out: table<T> = {}\n  return out\nend\n\nlocal xs = make(3)\n";
    let hints = raw_hints(src);
    assert!(
        !hints.iter().any(|(k, _, _)| *k == InlayHintKind::TYPE),
        "{hints:?}"
    );
}

#[test]
fn type_hint_for_inferred_local_from_constructor() {
    let src = "class Point\n  x: integer = 0\nend\n\nfn main()\n  local p = Point()\nend\n";
    let hints = raw_hints(src);
    let type_hints: Vec<_> = hints
        .iter()
        .filter(|(k, _, _)| *k == InlayHintKind::TYPE)
        .collect();
    assert_eq!(type_hints.len(), 1, "got {hints:?}");
    assert_eq!(type_hints[0].2, ": Point");
}

#[test]
fn no_type_hint_when_already_annotated() {
    let src = "fn main()\n  local x: integer = 1\nend\n";
    let hints = raw_hints(src);
    let type_hints: Vec<_> = hints
        .iter()
        .filter(|(k, _, _)| *k == InlayHintKind::TYPE)
        .collect();
    assert!(type_hints.is_empty(), "got {hints:?}");
}

#[test]
fn type_hint_for_int_literal() {
    let src = "fn main()\n  local n = 42\nend\n";
    let hints = raw_hints(src);
    let labels: Vec<&String> = hints
        .iter()
        .filter(|(k, _, _)| *k == InlayHintKind::TYPE)
        .map(|(_, _, l)| l)
        .collect();
    assert_eq!(labels, vec![&": integer".to_string()]);
}

#[test]
fn parameter_hint_for_positional_call_within_class() {
    // Free top-level functions aren't resolved by inlay yet, so
    // exercise the param-hint path through a sibling-class call.
    let src = "class Calc\n  fn add(a: integer, b: integer) -> integer\n    return a + b\n  end\n  fn main()\n    local r = self.add(1, 2)\n  end\nend\n";
    let hints = raw_hints(src);
    let labels: Vec<&String> = hints
        .iter()
        .filter(|(k, _, _)| *k == InlayHintKind::PARAMETER)
        .map(|(_, _, l)| l)
        .collect();
    assert!(labels.contains(&&"a:".to_string()), "got {hints:?}");
    assert!(labels.contains(&&"b:".to_string()), "got {hints:?}");
}

#[test]
fn parameter_hint_suppressed_when_arg_matches_param_name() {
    let src = "class Calc\n  fn add(a: integer, b: integer) -> integer\n    return a + b\n  end\n  fn main()\n    local a = 1\n    local b = 2\n    local r = self.add(a, b)\n  end\nend\n";
    let hints = raw_hints(src);
    let param_hints: Vec<_> = hints
        .iter()
        .filter(|(k, _, _)| *k == InlayHintKind::PARAMETER)
        .collect();
    assert!(param_hints.is_empty(), "got {param_hints:?}");
}

#[test]
fn parameter_hint_for_class_constructor() {
    let src = "class Point\n  x: integer = 0\n  y: integer = 0\n  fn init(x: integer, y: integer)\n    self.x = x\n    self.y = y\n  end\nend\n\nfn main()\n  local p = Point(1, 2)\nend\n";
    let hints = raw_hints(src);
    let labels: Vec<&String> = hints
        .iter()
        .filter(|(k, _, _)| *k == InlayHintKind::PARAMETER)
        .map(|(_, _, l)| l)
        .collect();
    assert!(labels.contains(&&"x:".to_string()), "got {hints:?}");
    assert!(labels.contains(&&"y:".to_string()), "got {hints:?}");
}

fn init_stdlib() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        saule_interpreter::init();
    });
}

#[test]
fn parameter_hint_for_free_top_level_fn() {
    let src = "fn add(x: integer, y: integer) -> integer\n  return x + y\nend\n\nfn main()\n  local r = add(1, 2)\nend\n";
    let hints = raw_hints(src);
    let labels: Vec<&String> = hints
        .iter()
        .filter(|(k, _, _)| *k == InlayHintKind::PARAMETER)
        .map(|(_, _, l)| l)
        .collect();
    assert!(labels.contains(&&"x:".to_string()), "got {hints:?}");
    assert!(labels.contains(&&"y:".to_string()), "got {hints:?}");
}

#[test]
fn parameter_hint_for_stdlib_module_call() {
    init_stdlib();
    // `String.find(s, pattern, init?)` — first two positionals get
    // `s:` and `pattern:` from the static names table.
    let src = "fn main()\n  local i = String.find(\"hello\", \"l\")\nend\n";
    let hints = raw_hints(src);
    let labels: Vec<&String> = hints
        .iter()
        .filter(|(k, _, _)| *k == InlayHintKind::PARAMETER)
        .map(|(_, _, l)| l)
        .collect();
    assert!(labels.contains(&&"s:".to_string()), "got {hints:?}");
    assert!(labels.contains(&&"pattern:".to_string()), "got {hints:?}");
}

#[test]
fn parameter_hint_suppressed_for_println() {
    init_stdlib();
    // `println` is registered as a purely-variadic native
    // (`println(...any)`). The walker treats variadic slots as
    // unlabel-able, so a `println("hello")` should produce no
    // parameter inlay hint — labelling the first arg `value:`
    // when there are no fixed positional slots would be noise.
    let src = "fn main()\n  println(\"hello\")\nend\n";
    let hints = raw_hints(src);
    let labels: Vec<&String> = hints
        .iter()
        .filter(|(k, _, _)| *k == InlayHintKind::PARAMETER)
        .map(|(_, _, l)| l)
        .collect();
    assert!(labels.is_empty(), "expected no param hints, got {hints:?}");
}
