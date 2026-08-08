//! Signature help tests. Bypasses `Backend` by replicating the
//! handler's pure inner logic (parse + analyse + walk + dispatch
//! to `build_help`) against an in-memory source string.

use super::*;
use std::sync::Once;
use tower_lsp::lsp_types::{ParameterLabel, SignatureInformation};

fn init_stdlib() {
    static ONCE: Once = Once::new();
    ONCE.call_once(saule_interpreter::init);
}

fn help(src: &str, cursor_at: &str, offset_into: usize) -> Option<SignatureHelp> {
    init_stdlib();
    let offset = src.find(cursor_at).expect("needle") + offset_into;
    let tokens = saule_lexer::Lexer::new(src).tokenize().expect("lex");
    let module = saule_parser::parse(tokens).expect("parse");
    let _ = saule_semantic::analyze(&module);
    help_from_module(&module, src, offset)
}

/// The label of the single signature reported at `offset`, panicking
/// when there is none — for the cases that assert *which* call
/// answered rather than whether one did.
fn label_at(src: &str, offset: usize) -> String {
    help_at(src, offset)
        .unwrap_or_else(|| panic!("no signature help at {offset}"))
        .signatures
        .first()
        .expect("at least one signature")
        .label
        .clone()
}

#[test]
fn signature_for_class_constructor_first_arg() {
    let src = "class Point\n  x: integer = 0\n  y: integer = 0\n  fn init(x: integer, y: integer)\n    self.x = x\n    self.y = y\n  end\nend\n\nfn main()\n  local p = Point(1, 2)\nend\n";
    // Cursor right after the `(`
    let h = help(src, "Point(1", 6).expect("help");
    let sig = &h.signatures[0];
    assert!(sig.label.starts_with("Point("), "label={}", sig.label);
    assert!(sig.label.contains("x: integer"), "label={}", sig.label);
    assert!(sig.label.contains("y: integer"), "label={}", sig.label);
    assert_eq!(h.active_parameter, Some(0));
}

#[test]
fn signature_active_param_advances_after_comma() {
    let src = "class Point\n  fn init(x: integer, y: integer)\n  end\nend\n\nfn main()\n  local p = Point(1, 2)\nend\n";
    // Cursor on the `2` (second arg)
    let h = help(src, "1, 2", 3).expect("help");
    assert_eq!(h.active_parameter, Some(1));
}

#[test]
fn signature_for_method_call() {
    let src = "class Foo\n  fn bar(n: integer) -> integer\n    return n\n  end\nend\n\nfn main()\n  local f: Foo = Foo()\n  local r = f.bar(7)\nend\n";
    let h = help(src, "bar(7", 4).expect("help");
    let sig = &h.signatures[0];
    assert!(sig.label.starts_with("f.bar("), "label={}", sig.label);
    assert!(sig.label.contains("n: integer"), "label={}", sig.label);
}

#[test]
fn no_signature_outside_call() {
    let src = "fn main()\n  local x = 1\nend\n";
    assert!(help(src, "x = 1", 0).is_none());
}

// ── textual fallback (mid-keystroke recovery) ─────────────────

/// Drive the textual fallback directly, without parsing. We still
/// need the registries seeded so `with_classes` / `lookup_method`
/// work — analyse a *prelude* containing the class def so the
/// in-progress snippet doesn't have to be syntactically valid.
fn fallback_help(prelude: &str, snippet: &str, cursor_at_end: usize) -> Option<SignatureHelp> {
    init_stdlib();
    let tokens = saule_lexer::Lexer::new(prelude).tokenize().expect("lex");
    let module = saule_parser::parse(tokens).expect("parse");
    let _ = saule_semantic::analyze(&module);
    // Pretend the snippet sits right after the prelude in the
    // same buffer; the fallback only reads `source[..offset]`.
    let combined = format!("{prelude}{snippet}");
    let offset = combined.len() - cursor_at_end;
    textual_fallback(&combined, offset)
}

#[test]
fn fallback_keeps_help_after_first_comma() {
    // User has typed `Point(1, ` with no closing paren — parser
    // would fail, but textual fallback should still surface the
    // sig with `active_parameter = 1`.
    let prelude = "class Point\n  fn init(x: integer, y: integer)\n  end\nend\n";
    let snippet = "Point(1, ";
    let h = fallback_help(prelude, snippet, 0).expect("fallback help");
    let sig = &h.signatures[0];
    assert!(sig.label.starts_with("Point("), "label={}", sig.label);
    assert_eq!(h.active_parameter, Some(1));
}

#[test]
fn fallback_active_param_zero_right_after_open_paren() {
    let prelude = "class Point\n  fn init(x: integer, y: integer)\n  end\nend\n";
    let snippet = "Point(";
    let h = fallback_help(prelude, snippet, 0).expect("fallback help");
    assert_eq!(h.active_parameter, Some(0));
}

#[test]
fn fallback_resolves_method_call_via_dot() {
    let prelude =
        "class Foo\n  fn bar(n: integer, m: integer) -> integer\n    return n\n  end\nend\n";
    let snippet = "Foo().bar(1, ";
    let h = fallback_help(prelude, snippet, 0).expect("fallback help");
    assert!(h.signatures[0].label.starts_with("bar("));
    assert_eq!(h.active_parameter, Some(1));
}

#[test]
fn fallback_clamps_active_param_to_arity() {
    let prelude = "class P\n  fn init(x: integer)\n  end\nend\n";
    let snippet = "P(1, 2, 3, ";
    let h = fallback_help(prelude, snippet, 0).expect("fallback help");
    // Only one param exists — clamp instead of going out of range.
    assert_eq!(h.active_parameter, Some(0));
}

// ── stdlib (native) signature help ────────────────────────────

#[test]
fn signature_for_stdlib_module_member() {
    let src = "fn main()\n  local n = Math.floor(3.14)\nend\n";
    let h = help(src, "floor(3", 6).expect("help");
    let sig = &h.signatures[0];
    assert!(sig.label.starts_with("Math.floor("), "label={}", sig.label);
    assert_eq!(h.active_parameter, Some(0));
}

#[test]
fn fallback_signature_for_stdlib_member_mid_typing() {
    // No closing paren → parser would fail; textual fallback
    // should still resolve `Math.floor(`.
    let prelude = "";
    let snippet = "  local n = Math.floor(";
    let h = fallback_help(prelude, snippet, 0).expect("fallback help");
    let sig = &h.signatures[0];
    assert!(sig.label.starts_with("Math.floor("), "label={}", sig.label);
    assert_eq!(h.active_parameter, Some(0));
}

#[test]
fn fallback_signature_for_stdlib_two_arg_after_comma() {
    // `Math.atan` is registered with 2 params (`(n, n?)`).
    let prelude = "";
    let snippet = "  local r = Math.atan(1, ";
    let h = fallback_help(prelude, snippet, 0).expect("fallback help");
    assert!(h.signatures[0].label.starts_with("Math.atan("));
    assert_eq!(h.active_parameter, Some(1));
}

// ── user-defined free top-level functions ─────────────────────

#[test]
fn signature_for_free_top_level_user_fn() {
    let src = "fn add(x: integer, y: integer) -> integer\n  return x + y\nend\n\nfn main()\n  local r = add(1, 2)\nend\n";
    let h = help(src, "add(1", 4).expect("help");
    let label = &h.signatures[0].label;
    assert!(label.starts_with("add("), "label={label}");
    assert!(label.contains("x: integer"), "label={label}");
    assert!(label.contains("y: integer"), "label={label}");
    assert!(label.contains("-> integer"), "label={label}");
    assert_eq!(h.active_parameter, Some(0));
}

#[test]
fn signature_for_free_user_fn_advances_active_param() {
    let src = "fn add(x: integer, y: integer) -> integer\n  return x + y\nend\n\nfn main()\n  local r = add(1, 2)\nend\n";
    // Position cursor between `1, ` and `2)` — second arg.
    let h = help(src, "1, 2", 3).expect("help");
    assert_eq!(h.active_parameter, Some(1));
}

#[test]
fn fallback_signature_for_free_user_fn() {
    // Mid-keystroke: closing paren missing on the call site, but
    // the `fn add` declaration parses fine on its own.
    let prelude = "fn add(x: integer, y: integer) -> integer\n  return x + y\nend";
    let snippet = "\nfn main()\n  local r = add(";
    let h = fallback_help(prelude, snippet, 0).expect("fallback help");
    let label = &h.signatures[0].label;
    assert!(label.starts_with("add("), "label={label}");
    assert!(label.contains("x: integer"), "label={label}");
    assert_eq!(h.active_parameter, Some(0));
}

// ── better native param names ─────────────────────────────────

#[test]
fn signature_for_stdlib_uses_real_param_names() {
    // `Math.floor(n: number) -> integer` — names should come from
    // the static stdlib table, not synthesised `arg0`.
    let src = "fn main()\n  local n = Math.floor(3.14)\nend\n";
    let h = help(src, "floor(3", 6).expect("help");
    let label = &h.signatures[0].label;
    assert!(label.contains("n: "), "expected `n:` in {label}");
    assert!(!label.contains("arg0"), "should not contain arg0: {label}");
}

#[test]
fn signature_for_stdlib_string_find_uses_real_param_names() {
    let src = "fn main()\n  local i, j = String.find(\"hello\", \"l\")\nend\n";
    let h = help(src, "find(\"", 5).expect("help");
    let label = &h.signatures[0].label;
    assert!(label.contains("s: "), "expected `s:` in {label}");
    assert!(
        label.contains("pattern: "),
        "expected `pattern:` in {label}"
    );
    assert!(label.contains("init"), "expected `init` in {label}");
}

// ── `self.super(...)` → parent constructor ────────────────────

#[test]
fn signature_for_self_super_shows_parent_init() {
    let src = "class Base
  fn init(x: integer, y: integer)
  end
end

class Child extends Base
  fn init()
    self.super(1, 2)
  end
end
";
    let h = help(src, "self.super(1", "self.super(".len()).expect("help");
    let label = &h.signatures[0].label;
    assert!(label.starts_with("Base.init("), "label={label}");
    assert!(label.contains("x: integer"), "label={label}");
    assert_eq!(h.active_parameter, Some(0));
}

/// Mid-keystroke `self.super(1, ` with no closing paren. The
/// registries still hold the last good parse (both classes), which
/// is what the enclosing-class text scan is resolved against.
#[test]
fn fallback_signature_for_self_super_mid_typing() {
    let prelude = "class Base
  fn init(x: integer, y: integer)
  end
end

class Child extends Base
  fn init()
  end
end
";
    let snippet = "class Child extends Base
  fn init()
    self.super(1, ";
    let h = fallback_help(prelude, snippet, 0).expect("fallback help");
    assert!(h.signatures[0].label.starts_with("Base.init("));
    assert_eq!(h.active_parameter, Some(1));
}

// ── coverage matrix: every call form the language has ─────────
//
// One shared fixture, two passes: the finished-code path (parens
// closed) and the mid-keystroke path (the user has typed `(` and
// nothing after it yet). A form missing from here is a form where
// the parameter popup silently does nothing.

const FIXTURE: &str = "\
class Color
  fn apply(alpha: float)
  end
end

class Widget
  fn init(x: float, y: float)
  end
  fn moveTo(x: float, y: float)
  end
  static fn make(n: integer) -> Widget
    return Widget(0.0, 0.0)
  end
end

enum Shape
  Circle(r: float)
  Square
end

fn add(x: integer, y: integer) -> integer
  return x + y
end
";

/// Byte offset just past `needle`, searched after the fixture so
/// call sites are found rather than the declarations above.
fn call_offset(src: &str, needle: &str) -> usize {
    let start = FIXTURE.len();
    src[start..]
        .find(needle)
        .map(|i| start + i + needle.len())
        .unwrap_or_else(|| panic!("needle {needle:?} not found"))
}

/// The signature the user actually sees highlighted. The list is
/// ordered by source position and can carry several nesting levels,
/// so `active_signature` — not index 0 — is the current one.
fn label(h: Option<SignatureHelp>, case: &str) -> String {
    let h = h.unwrap_or_else(|| panic!("no signature help for {case}"));
    let i = h.active_signature.unwrap_or(0) as usize;
    h.signatures[i].label.clone()
}

fn active_label(h: &SignatureHelp) -> &str {
    &h.signatures[h.active_signature.unwrap_or(0) as usize].label
}

/// Finished code: the call's parens are closed, so the document
/// parses and the AST walker resolves the callee.
#[test]
fn signature_help_covers_every_call_form() {
    init_stdlib();
    let cases: Vec<(&str, &str, &str, &str)> = vec![
        ("free fn", "  local r = add(1, 2)\n", "add(1", "add("),
        (
            "constructor",
            "  local w = Widget(1.0, 2.0)\n",
            "Widget(1.0",
            "Widget(",
        ),
        (
            "method on annotated local",
            "  local w: Widget = Widget(1.0, 2.0)\n  w.moveTo(3.0, 4.0)\n",
            "moveTo(3.0",
            "w.moveTo(",
        ),
        (
            "method on inferred local",
            "  local w = Widget(1.0, 2.0)\n  w.moveTo(3.0, 4.0)\n",
            "moveTo(3.0",
            "w.moveTo(",
        ),
        (
            "static method",
            "  Widget.make(1)\n",
            "make(1",
            "Widget.make(",
        ),
        (
            // No plain dotted path to show — the receiver is a call
            // — so the heading falls back to the resolved owner.
            "method on constructor result",
            "  Widget(1.0, 2.0).moveTo(3.0, 4.0)\n",
            "moveTo(3.0",
            "Widget.moveTo(",
        ),
        (
            "method on call result",
            "  Widget.make(1).moveTo(3.0, 4.0)\n",
            "moveTo(3.0",
            "Widget.moveTo(",
        ),
        (
            "stdlib module fn",
            "  local n = Math.floor(3.14)\n",
            "floor(3.14",
            "Math.floor(",
        ),
        ("bare native", "  println(1)\n", "println(1", "println("),
        (
            "enum tuple variant",
            "  local s = Shape.Circle(1.0)\n",
            "Circle(1.0",
            "Shape.Circle(",
        ),
        (
            "function-typed local",
            "  local f: fn(integer) -> integer = fn(n: integer) -> integer return n end\n  local z = f(1)\n",
            // Inside the args — a needle ending in `)` would put the
            // caret past the close paren, which is outside the call.
            "f(1",
            "f(",
        ),
        (
            "nested inner call",
            "  local r = add(add(1, 2), 3)\n",
            "add(1",
            "add(",
        ),
        (
            "named argument",
            "  local w = Widget(x: 1.0, y: 2.0)\n",
            "Widget(x: 1.0",
            "Widget(",
        ),
        // The caret is on the *slot* that takes the table, not
        // inside its braces — inside is data, and covered by
        // `a_table_literal_is_data_not_a_parameter_slot`.
        (
            "table-literal argument",
            "  local t = add({1, 2}, 3)\n",
            "add(",
            "add(",
        ),
        // The piped value fills slot 0, so only `y` is left.
        (
            "pipeline stage",
            "  local n = when(4):add(3)\n",
            "add(3",
            "add(y: integer)",
        ),
    ];
    for (case, body, needle, expected) in cases {
        let src = format!("{FIXTURE}\nfn probe()\n{body}end\n");
        let got = label(help_at(&src, call_offset(&src, needle)), case);
        assert!(got.starts_with(expected), "{case}: got {got:?}");
    }
}

/// Call forms that only exist inside a class body.
#[test]
fn signature_help_covers_in_class_call_forms() {
    init_stdlib();
    let src = format!(
        "{FIXTURE}
class Probe extends Widget
  tint: Color
  fn init()
    self.super(1.0, 2.0)
  end
  fn go(w: Widget)
    self.go2(1)
    w.moveTo(3.0, 4.0)
    self.tint.apply(1.0)
    go2(1)
    Probe.stat(1)
  end
  fn go2(n: integer)
  end
  static fn stat(n: integer)
  end
end
"
    );
    for (case, needle, expected) in [
        ("self.super", "self.super(1.0", "Widget.init("),
        ("self.method", "self.go2(1", "Probe.go2("),
        ("method on parameter", "w.moveTo(3.0", "w.moveTo("),
        (
            "method on field",
            "self.tint.apply(1.0",
            "Probe.tint.apply(",
        ),
        ("bare sibling method", "\n    go2(1", "go2("),
        ("own static via class name", "Probe.stat(1", "Probe.stat("),
    ] {
        let got = label(help_at(&src, call_offset(&src, needle)), case);
        assert!(got.starts_with(expected), "{case}: got {got:?}");
    }
}

/// Mid-keystroke: the user has typed `(` (or `(arg, `) and the
/// buffer doesn't parse yet. This is the path that has to keep the
/// popup alive while the arguments are being typed.
#[test]
fn signature_help_survives_unclosed_call() {
    init_stdlib();
    for (case, snippet, expected, active) in [
        ("free fn", "fn probe()\n  local r = add(", "add(", 0),
        (
            "free fn second arg",
            "fn probe()\n  local r = add(1, ",
            "add(",
            1,
        ),
        (
            "constructor",
            "fn probe()\n  local w = Widget(",
            "Widget(",
            0,
        ),
        (
            "method on annotated local",
            "fn probe()\n  local w: Widget = Widget(1.0, 2.0)\n  w.moveTo(",
            "w.moveTo(",
            0,
        ),
        (
            "method on inferred local",
            "fn probe()\n  local w = Widget(1.0, 2.0)\n  w.moveTo(",
            "w.moveTo(",
            0,
        ),
        (
            "static method",
            "fn probe()\n  Widget.make(",
            "Widget.make(",
            0,
        ),
        (
            "method on constructor result",
            "fn probe()\n  Widget(1.0, 2.0).moveTo(",
            "Widget.moveTo(",
            0,
        ),
        (
            "stdlib module fn",
            "fn probe()\n  local n = Math.floor(",
            "Math.floor(",
            0,
        ),
        ("bare native", "fn probe()\n  println(", "println(", 0),
        (
            "enum tuple variant",
            "fn probe()\n  local s = Shape.Circle(",
            "Shape.Circle(",
            0,
        ),
        (
            // `self.` reads as the class it stands for.
            "self.method",
            "class Probe extends Widget\n  fn go()\n    self.moveTo(",
            "Probe.moveTo(",
            0,
        ),
        (
            "self.super",
            "class Probe extends Widget\n  fn init()\n    self.super(",
            "Widget.init(",
            0,
        ),
        (
            "method on field",
            "class Probe extends Widget\n  tint: Color\n  fn go()\n    self.tint.apply(",
            "Probe.tint.apply(",
            0,
        ),
        (
            "nested inner call",
            "fn probe()\n  local r = add(add(",
            "add(",
            0,
        ),
    ] {
        let src = format!("{FIXTURE}\n{snippet}");
        let offset = src.len();
        let h = help_mid_keystroke(&src, offset)
            .unwrap_or_else(|| panic!("no signature help for {case}"));
        assert_eq!(h.active_parameter, Some(active), "{case}");
        let got = active_label(&h);
        assert!(got.starts_with(expected), "{case}: got {got:?}");
    }
}

/// A `name: value` argument highlights the slot its key names,
/// not the positional slot it happens to sit in.
#[test]
fn named_argument_highlights_its_own_slot() {
    init_stdlib();
    let src = format!(
        "{FIXTURE}
fn probe()
  local w = Widget(y: 2.0)
end
"
    );
    let h = help_at(&src, call_offset(&src, "Widget(y: 2")).expect("help");
    assert_eq!(h.active_parameter, Some(1));
}

/// The realistic shape of the same thing: the half-typed call sits
/// in the *middle* of a file, with well-formed code after it. The
/// repair has to close the call at the cursor — appending past the
/// trailing `end`s closes nothing and the popup stays empty.
#[test]
fn signature_help_survives_unclosed_call_mid_file() {
    init_stdlib();
    for (case, head, tail, expected, active) in [
        (
            "method on local",
            "fn probe()\n  local w: Widget = Widget(1.0, 2.0)\n  w.moveTo(",
            "\nend\n",
            "w.moveTo(",
            0,
        ),
        (
            "constructor second arg",
            "fn probe()\n  local w = Widget(1.0, ",
            "\nend\n\nfn after()\n  println(1)\nend\n",
            "Widget(",
            1,
        ),
        (
            "self.super",
            "class Probe extends Widget\n  fn init()\n    self.super(",
            "\n  end\nend\n",
            "Widget.init(",
            0,
        ),
        (
            "nested call",
            "fn probe()\n  local r = add(add(",
            "\nend\n",
            "add(",
            0,
        ),
        (
            "method on field",
            "class Probe extends Widget\n  tint: Color\n  fn go()\n    self.tint.apply(",
            "\n  end\nend\n",
            "Probe.tint.apply(",
            0,
        ),
    ] {
        let src = format!("{FIXTURE}\n{head}{tail}");
        let offset = FIXTURE.len() + 1 + head.len();
        let h = help_mid_keystroke(&src, offset)
            .unwrap_or_else(|| panic!("no signature help for {case}"));
        assert_eq!(h.active_parameter, Some(active), "{case}");
        let got = active_label(&h);
        assert!(got.starts_with(expected), "{case}: got {got:?}");
    }
}

/// Every parameter range must be a valid slice of the label
/// measured in UTF-16 code units — that's what the client indexes
/// with. A default too complex to print renders `" = …"`, whose `…` is
/// three bytes but one code unit, so byte offsets would run past the
/// end of the label and the client would slice out of bounds.
#[test]
fn parameter_offsets_are_utf16_code_units() {
    init_stdlib();
    // `1.0 / 2.0` is deliberately an expression: simple literals now
    // print their value, and only an elided default puts the non-ASCII
    // `…` into the label that this test exists to measure.
    let src = "class Color
  fn init(r: float = 1.0 / 2.0, g: float = 1.0 / 2.0, b: float = 1.0 / 2.0)
  end
end

fn probe()
  local c = Color(1.0)
end
";
    let h = help_at(src, src.rfind("Color(1.0").unwrap() + "Color(".len()).expect("help");
    let sig = &h.signatures[0];
    let units: Vec<u16> = sig.label.encode_utf16().collect();
    assert!(sig.label.contains(" = …"), "label={}", sig.label);
    let params = sig.parameters.as_ref().expect("parameters");
    assert_eq!(params.len(), 3);
    for (i, p) in params.iter().enumerate() {
        let ParameterLabel::LabelOffsets([s, e]) = p.label else {
            panic!("expected offset labels");
        };
        assert!(
            s <= e && (e as usize) <= units.len(),
            "param {i} range [{s}, {e}) out of bounds for label of {} units: {}",
            units.len(),
            sig.label
        );
        let slice = String::from_utf16(&units[s as usize..e as usize]).expect("utf16");
        assert!(
            slice.starts_with(["r", "g", "b"][i]),
            "param {i} sliced to {slice:?}"
        );
    }
}

/// Nested calls: which signature is showing depends on the caret
/// position, and the boundaries have to be exact. `f(g())` has
/// four interesting spots — the caret is in `f`'s arg list
/// everywhere except strictly inside `g`'s parens.
#[test]
fn nested_call_switches_signature_at_the_paren_boundaries() {
    init_stdlib();
    let src = "class Color
  fn init(r: float = 1.0, g: float = 1.0)
  end
end

class View
  fn setBackground(color: Color)
  end
end

class PanelView extends View
  fn init()
    setBackground(Color())
  end
end
";
    let call = src.find("setBackground(Color())").expect("call site");
    // setBackground(Color())
    // ^0           ^13    ^20
    for (case, at, expected) in [
        (
            "before the argument",
            call + "setBackground(".len(),
            "setBackground(",
        ),
        (
            "on the callee name",
            call + "setBackground(Color".len(),
            "setBackground(",
        ),
        (
            "inside the inner parens",
            call + "setBackground(Color(".len(),
            "Color(",
        ),
        (
            "after the inner call",
            call + "setBackground(Color()".len(),
            "setBackground(",
        ),
    ] {
        let got = label(help_at(src, at), case);
        assert!(
            got.starts_with(expected),
            "{case}: expected {expected:?}, got {got:?}"
        );
    }
    // Past the outer `)` the popup belongs to nobody.
    assert!(
        help_at(src, call + "setBackground(Color())".len()).is_none(),
        "caret past the outer close paren should not report a signature"
    );
}

/// A nested call reports exactly one signature: the call the caret
/// is inside. Enclosing levels are not offered — a popup with a row
/// per level makes the reader pick their own function out of a list,
/// which in nested widget code is nearly every call.
#[test]
fn nested_call_reports_only_the_call_the_caret_is_in() {
    init_stdlib();
    let src = "class Color
  fn init(r: float = 1.0, g: float = 1.0)
  end
end

class View
  fn setBackground(color: Color?)
  end
end

class PanelView extends View
  fn init()
    self.setBackground(Color(10f, 10f))
  end
end
";
    let call = src.find("self.setBackground(Color").expect("call site");
    let outer = help_at(src, call + "self.setBackground(".len()).expect("help");
    let inner = help_at(src, call + "self.setBackground(Color(".len()).expect("help");

    // One row each, naming the call that position is inside — never
    // the other one.
    assert_eq!(outer.signatures.len(), 1, "{:?}", outer.signatures);
    assert_eq!(inner.signatures.len(), 1, "{:?}", inner.signatures);
    assert_eq!(outer.active_signature, Some(0));
    assert_eq!(inner.active_signature, Some(0));
    assert!(active_label(&outer).starts_with("PanelView.setBackground("));
    assert!(active_label(&inner).starts_with("Color("));
}

/// The response must never carry more signatures than the popup was
/// opened with: IntelliJ creates its rows once and LSP4IJ indexes
/// them by the position of every later response, so a longer list
/// throws ArrayIndexOutOfBounds inside the IDE.
#[test]
fn retrigger_never_grows_the_signature_list() {
    let sig = |label: &str| SignatureInformation {
        label: label.to_string(),
        documentation: None,
        parameters: Some(Vec::new()),
        active_parameter: Some(0),
    };
    let prev = SignatureHelp {
        signatures: vec![sig("setBackground(color: Color?)")],
        active_signature: Some(0),
        active_parameter: Some(0),
    };
    let fresh = SignatureHelp {
        signatures: vec![sig("setBackground(color: Color?)"), sig("Color(r: float)")],
        active_signature: Some(1),
        active_parameter: Some(0),
    };
    let out = reconcile_with_client(fresh, &prev);
    assert_eq!(
        out.signatures.len(),
        1,
        "must not exceed the client's row count"
    );

    // Shrinking or staying level is safe, so the fresh answer wins.
    let fresh = SignatureHelp {
        signatures: vec![sig("Color(r: float)")],
        active_signature: Some(0),
        active_parameter: Some(1),
    };
    let out = reconcile_with_client(fresh, &prev);
    assert_eq!(out.signatures.len(), 1);
    assert!(out.signatures[0].label.starts_with("Color("));
}

/// A call that takes no arguments is never worth a popup row —
/// IntelliJ spells it `<no parameters>`. It's dropped from the
/// chain, and if that empties the chain there's no popup at all.
#[test]
fn parameterless_calls_are_not_offered() {
    init_stdlib();
    let src = "class Timer
  fn getDelta() -> float
    return 0.0
  end
end

class Root
  fn update(dt: float)
  end
end

fn probe()
  local t = Timer()
  local root = Root()
  root.update(t.getDelta())
end
";
    let call = src.find("root.update(t.getDelta())").expect("call site");

    // Caret inside the parameterless `getDelta()`: that level is
    // dropped, leaving the enclosing `update` as the only row.
    let h = help_at(src, call + "root.update(t.getDelta(".len()).expect("help");
    let labels: Vec<&str> = h.signatures.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(
        labels.len(),
        1,
        "getDelta should be filtered out: {labels:?}"
    );
    assert!(labels[0].starts_with("root.update("), "{labels:?}");
    assert!(active_label(&h).starts_with("root.update("));

    // Caret inside the parameterless call with nothing enclosing it:
    // no popup at all.
    let src = "class Timer
  fn getDelta() -> float
    return 0.0
  end
end

fn probe()
  local t = Timer()
  local d = t.getDelta()
end
";
    let at = src.find("t.getDelta()").expect("call") + "t.getDelta(".len();
    assert!(
        help_at(src, at).is_none(),
        "parameterless call should not open a popup"
    );
}

/// The caret is not required to arrive by typing. It can be arrowed
/// backwards, arrowed forwards again, or clicked straight onto any
/// argument — every position has to resolve on its own terms.
///
/// The invariant checked at every offset in the expression: the one
/// signature returned always names the call the caret is really in.
#[test]
fn caret_can_land_anywhere_in_a_nested_call() {
    init_stdlib();
    let src = "class Color
  static fn rgb(r: integer, g: integer, b: integer) -> Color?
    return nil
  end
end

class Root
  fn setBackground(color: Color?)
  end
end

fn probe()
  local root = Root()
  root.setBackground(Color.rgb(38, 38, 38))
end
";
    let text = "root.setBackground(Color.rgb(38, 38, 38))";
    let call = src.find(text).expect("call site");
    let outer_open = "root.setBackground(".len();
    let inner_open = "root.setBackground(Color.rgb(".len();
    let inner_close = text.rfind("))").expect("closers");

    // 1. Every live position answers, with exactly one row.
    for i in outer_open..=inner_close + 1 {
        let h = help_at(src, call + i).unwrap_or_else(|| panic!("no help at +{i}"));
        assert_eq!(h.signatures.len(), 1, "at +{i}: {:?}", h.signatures);
    }

    // 2. That row tracks the caret, in both directions.
    for i in outer_open..inner_open {
        let h = help_at(src, call + i).expect("help");
        assert!(
            active_label(&h).starts_with("root.setBackground("),
            "at +{i}"
        );
    }
    for i in inner_open..=inner_close {
        let h = help_at(src, call + i).expect("help");
        assert!(active_label(&h).starts_with("Color.rgb("), "at +{i}");
    }
    // Between the two closing parens we're back in the outer call.
    let h = help_at(src, call + inner_close + 1).expect("help");
    assert!(active_label(&h).starts_with("root.setBackground("));
    // Past the whole expression there's nothing to show.
    assert!(help_at(src, call + text.len()).is_none());

    // 3. Clicking directly onto an argument selects that slot —
    //    including jumping backwards from the third to the first.
    for (slot, delta) in [(0u32, 0usize), (1, 4), (2, 8), (0, 0)] {
        let h = help_at(src, call + inner_open + delta).expect("help");
        assert!(active_label(&h).starts_with("Color.rgb("), "slot {slot}");
        assert_eq!(h.active_parameter, Some(slot), "clicked slot {slot}");
    }
}

/// The shape this rule exists for: three widgets nested on one line,
/// each with parameters. Standing in any one of them reports that
/// one — not a menu of all three, and not the sibling call earlier
/// on the same line.
#[test]
fn a_nested_widget_expression_reports_one_row_per_position() {
    init_stdlib();
    let src = "class Alignment
  static fn centerLeft() -> Alignment?
    return nil
  end
end

class ProgressBar
  fn init(value: float = 0.0)
  end
end

class Align
  fn init(alignment: Alignment? = nil, child: ProgressBar? = nil)
  end
end

class SizedBox
  fn init(width: float? = nil, child: Align? = nil)
  end
end

fn probe()
  local b = SizedBox(width: 120.0, child: Align(alignment: Alignment.centerLeft(), child: ProgressBar(value: 1.0)))
end
";
    let call = src.find("SizedBox(width:").expect("call site");
    for (case, prefix, expected) in [
        ("outer", "SizedBox(", "SizedBox("),
        ("middle", "SizedBox(width: 120.0, child: Align(", "Align("),
        (
            "innermost",
            "SizedBox(width: 120.0, child: Align(alignment: Alignment.centerLeft(), child: ProgressBar(",
            "ProgressBar(",
        ),
    ] {
        let h = help_at(src, call + prefix.len()).unwrap_or_else(|| panic!("no help: {case}"));
        assert_eq!(
            h.signatures.len(),
            1,
            "{case}: expected one row, got {:?}",
            h.signatures.iter().map(|s| &s.label).collect::<Vec<_>>()
        );
        assert!(
            active_label(&h).starts_with(expected),
            "{case}: expected {expected:?}, got {:?}",
            active_label(&h)
        );
    }
}

#[test]
fn unclosed_delimiters_ignores_strings_and_comments() {
    assert_eq!(unclosed_delimiters("add(1, 2)"), "");
    assert_eq!(unclosed_delimiters("add(f("), "))");
    assert_eq!(unclosed_delimiters("f(\"a )( b\""), ")");
    assert_eq!(unclosed_delimiters("f( -- a ) comment\n"), ")");
    assert_eq!(unclosed_delimiters("f( --[[ ) ]] "), ")");
    assert_eq!(unclosed_delimiters("t = {1, [2] = f("), ")}");
}

/// Like `help` but takes an absolute byte offset.
fn help_at(src: &str, offset: usize) -> Option<SignatureHelp> {
    init_stdlib();
    let tokens = saule_lexer::Lexer::new(src).tokenize().expect("lex");
    let module = saule_parser::parse(tokens).expect("parse");
    let _ = saule_semantic::analyze(&module);
    help_from_module(&module, src, offset)
}

/// Mirrors `signature_help_at`'s dispatch for a buffer that may not
/// parse: real AST first, repaired AST second, text scan last.
fn help_mid_keystroke(src: &str, offset: usize) -> Option<SignatureHelp> {
    init_stdlib();
    // Mirrors `signature_help_at`'s control flow exactly, including
    // the fall-through. An earlier version of this helper `return`ed
    // the parsed module's answer instead of falling through, so a
    // suppression that the real handler then handed to
    // `textual_fallback` — which re-derived the enclosing widget by
    // counting parens — looked correct here and wrong in the IDE.
    if let Ok(tokens) = saule_lexer::Lexer::new(src).tokenize()
        && let Ok(module) = saule_parser::parse(tokens)
    {
        let _ = saule_semantic::analyze(&module);
        match answer_from_module(&module, src, offset) {
            Answer::Help(h) => return Some(h),
            Answer::Suppressed => return None,
            Answer::Unresolved => {}
        }
    } else if let Some(module) = repair_parse(src, offset) {
        let _ = saule_semantic::analyze(&module);
        match answer_from_module(&module, src, offset) {
            Answer::Help(h) => return Some(h),
            Answer::Suppressed => return None,
            Answer::Unresolved => {}
        }
    }
    textual_fallback(src, offset)
}

/// The shape from the UI panel: a callback written inline as a named
/// argument, several lines of statements deep. Standing on one of
/// those statements, the popup for the enclosing widget must be
/// gone — the caret is writing a body, not filling in an argument.
#[test]
fn a_callback_body_ends_the_enclosing_calls_popup() {
    let src = "class TextField
  fn init(placeholder: string = \"\", onChanged: function? = nil)
  end
end

class Column
  fn init(children: table<Widget>? = nil, spacing: float = 0.0)
  end
end

fn build()
  local c = Column(spacing: 4.0, children: {
    TextField(placeholder: \"name\", onChanged: fn(text: string)
      local trimmed: string = text
      local n: integer = 1
    end)
  })
end
";
    // Inside the callback body: neither TextField nor Column.
    for needle in ["local trimmed: string = text", "local n: integer = 1"] {
        let at = src.find(needle).expect(needle) + 2;
        assert!(
            help_at(src, at).is_none(),
            "popup survived into the callback body at {needle:?}"
        );
    }
    // Still reported on the arguments themselves, either side of it.
    let at = src.find("placeholder: \"name\"").expect("arg") + 2;
    assert!(label_at(src, at).starts_with("TextField("));
    let at = src.find("spacing: 4.0").expect("arg") + 2;
    assert!(label_at(src, at).starts_with("Column("));
}

/// The suppression has to survive the *whole* handler, not just the
/// AST walk.
///
/// `help_at` stops at the walker, and the walker was right all along.
/// The handler then fell through to [`textual_fallback`], which
/// resolves by counting unmatched `(` in raw text: at a caret inside
/// the callback body, `Switch(`'s paren is still open, so it happily
/// re-reported the widget the walker had just ruled out. Only the
/// first body line showed it — one line up, the innermost unmatched
/// paren is the lambda's own `fn(`, which resolves to nothing.
#[test]
fn the_fallback_does_not_resurrect_a_suppressed_call() {
    let src = "class Switch
  fn init(value: boolean = false, label: string = \"\", onChanged: function? = nil)
  end
end

fn build()
  local s = Switch(value: true, label: \"Sound\", onChanged: fn(next: boolean)
    scratch.sound = next
    rebuild()
  end)
end
";
    for needle in ["scratch.sound = next", "rebuild()"] {
        let at = src.find(needle).expect(needle) + 3;
        assert!(
            help_mid_keystroke(src, at).is_none(),
            "textual fallback resurrected the popup at {needle:?}: {:?}",
            help_mid_keystroke(src, at).map(|h| h.signatures[0].label.clone())
        );
    }
    // The argument keys still answer through the same dispatch.
    let at = src.find("label: \"Sound\"").expect("arg") + 2;
    assert!(
        help_mid_keystroke(src, at)
            .expect("help on the key")
            .signatures[0]
            .label
            .starts_with("Switch(")
    );
}

/// The signature is laid out the way the call is: a widget written
/// one argument per line gets one parameter per line, rather than a
/// single line that runs off the side of the popup.
#[test]
fn a_multiline_call_gets_a_multiline_signature() {
    let src = "class Column
  fn init(children: table<Widget>? = nil, spacing: float = 0.0)
  end
end

fn build()
  local wide = Column(
    children: nil,
    spacing: 4.0
  )
  local tight = Column(children: nil, spacing: 4.0)
end
";
    let at = src.find("children: nil,\n").expect("multi-line call") + 2;
    assert_eq!(
        label_at(src, at),
        "Column(\n  children: table<Widget>? = nil,\n  spacing: float = 0.0\n)"
    );

    // The same call on one line stays on one line — it fits.
    let at = src.find("Column(children: nil").expect("single-line call") + "Column(".len();
    assert_eq!(
        label_at(src, at),
        "Column(children: table<Widget>? = nil, spacing: float = 0.0)"
    );
}

/// A signature wider than the popup breaks itself even when the call is
/// on one line. Mirroring the call's own layout is the right default,
/// but it can't be the whole rule: `Card(data: x)` fits on one line
/// while its signature does not, and the label then wrapped to column 0
/// as an unreadable run-on.
#[test]
fn a_wide_signature_breaks_even_for_a_one_line_call() {
    let src = "class Card
  fn init(data: ThemeData? = nil, child: View? = nil, key: string? = nil, content: (fn() -> nil)? = nil)
  end
end

fn build()
  local c = Card(data: nil)
end
";
    let at = src.find("Card(data: nil").expect("call") + "Card(".len();
    assert_eq!(
        label_at(src, at),
        "Card(\n  \
           data: ThemeData? = nil,\n  \
           child: View? = nil,\n  \
           key: string? = nil,\n  \
           content: (fn() -> nil)? = nil\n\
         )"
    );
}

/// A call the user has only just opened is not "multi-line" merely
/// because the rest of the file sits below it. `repair_parse` closes
/// such a call by appending at the end of the document, so its
/// argument region reaches there — the layout is therefore measured
/// over the arguments actually written.
#[test]
fn a_freshly_opened_call_is_not_multiline() {
    let src = "fn note(message: string, level: integer = 0)\nend\n\nfn build()\n  note(\nend\n";
    let at = src.find("note(\n").expect("call") + "note(".len();
    let h = help_mid_keystroke(src, at).expect("help");
    assert_eq!(
        h.signatures[0].label,
        "note(message: string, level: integer = 0)"
    );
}

/// The heading names the callee the way it was written, so a static
/// call reads `Theme.of` and a chain reads its whole path.
#[test]
fn the_heading_carries_the_dotted_callee_path() {
    let src = "class ThemeData
  fn init(dark: boolean = false)
  end
end

class Theme
  static fn of(context: integer) -> ThemeData?
    return nil
  end
end

class Inner
  fn three(x: integer)
  end
end

class Mid
  two: Inner
  fn init(two: Inner)
    self.two = two
  end
end

fn probe(one: Mid)
  Theme.of(1)
  one.two.three(1)
end
";
    let at = src.find("Theme.of(1)").expect("static call") + "Theme.of(".len();
    assert!(
        label_at(src, at).starts_with("Theme.of(context: integer)"),
        "{}",
        label_at(src, at)
    );

    let at = src.find("one.two.three(1)").expect("chain") + "one.two.three(".len();
    assert!(
        label_at(src, at).starts_with("one.two.three(x: integer)"),
        "{}",
        label_at(src, at)
    );
}

/// A table literal holds data, so the caret inside one is not at any
/// parameter. `children: {…}` with ten widgets in it kept the full
/// `Column(...)` list on screen for every one of them, describing a
/// slot the reader had already filled.
#[test]
fn a_table_literal_is_data_not_a_parameter_slot() {
    let src = "class Text
  fn init(data: string = \"\", size: float = 12.0)
  end
end

class Column
  fn init(children: table<Widget>? = nil, spacing: float = 0.0)
  end
end

fn build()
  local c = Column(spacing: 4.0, children: {
    Text(data: \"a\"),
    Text(data: \"b\"),
  })
end
";
    let open = src.find("children: {").expect("table at the call site");

    // Just inside the opening brace: data, so nothing.
    let at = open + "children: {".len();
    assert!(
        help_at(src, at).is_none(),
        "reported {:?} just inside the brace",
        help_at(src, at).map(|h| h.signatures[0].label.clone())
    );
    // Between the two entries, likewise.
    let at = src.find("Text(data: \"b\"").expect("second entry") - 1;
    assert!(
        help_at(src, at).is_none(),
        "reported {:?} between table entries",
        help_at(src, at).map(|h| h.signatures[0].label.clone())
    );

    // A call written inside the table resolves normally — that is
    // the whole point of stopping at the brace rather than earlier.
    let at = src.find("Text(data: \"a\"").expect("entry") + "Text(".len();
    assert!(
        label_at(src, at).starts_with("Text("),
        "{}",
        label_at(src, at)
    );

    // And the keys still answer, so you can see what `children`
    // wants before you open it.
    assert!(label_at(src, open + 2).starts_with("Column("));
    let at = src.find("spacing: 4.0").expect("key") + 2;
    assert!(label_at(src, at).starts_with("Column("));
}

/// A free function declared in another file and imported — through a
/// re-export barrel, as every UIKit helper is. `analyze_with_seed`
/// puts imported top-level functions in the semantic registry, so
/// this needs no import-graph walk of its own.
#[test]
fn an_imported_free_function_reports_its_signature() {
    init_stdlib();
    let dir = std::env::temp_dir().join(format!("saule-sighelp-barrel-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("kit")).unwrap();
    std::fs::write(
        dir.join("kit").join("overlay.sau"),
        "export fn showToast(context: integer, message: string = \"\") -> nothing\nend\n",
    )
    .unwrap();
    std::fs::write(dir.join("kit").join("init.sau"), "import * from overlay\n").unwrap();

    let src = "import * from kit\n\nfn build()\n  showToast(1, \"hi\")\nend\n";
    let tokens = saule_lexer::Lexer::new(src).tokenize().expect("lex");
    let module = saule_parser::parse(tokens).expect("parse");
    let seed = saule_interpreter::module::collect_import_seed(&module, &dir);
    let _ = saule_semantic::analyze_with_seed(&module, seed);

    let at = src.find("showToast(1").expect("call") + "showToast(".len();
    let h = help_from_module(&module, src, at).expect("help for imported fn");
    assert!(
        h.signatures[0]
            .label
            .starts_with("showToast(context: integer"),
        "got {:?}",
        h.signatures[0].label
    );
    assert_eq!(h.active_parameter, Some(0));

    let _ = std::fs::remove_dir_all(&dir);
}

/// Where the suppression starts decides whether it works at all.
///
/// `(` is a trigger character, so `fn(` re-queries the server and
/// re-opens the popup. A barrier that began only at the body would
/// answer `TextField` at that keystroke, leaving a freshly-opened
/// popup one line above the body — and from there the caret only
/// *moves*, which LSP4IJ services without re-labelling or closing.
/// The `None` has to land on the trigger, so the whole lambda is
/// covered, parameter list included.
#[test]
fn suppression_starts_at_the_lambda_not_at_its_body() {
    let src = "class TextField
  fn init(placeholder: string = \"\", onChanged: function? = nil)
  end
end

fn build()
  local t = TextField(placeholder: \"name\", onChanged: fn(text: string)
    local n: integer = 1
  end)
end
";
    let key = src.find("onChanged: fn(text").expect("key");
    // Choosing what to pass: the widget still answers.
    assert!(label_at(src, key + 2).starts_with("TextField("));
    assert!(label_at(src, key + "onChanged:".len()).starts_with("TextField("));

    // Writing the callback: silent from the parameter list onward,
    // and in particular at the `(` the client re-triggers on.
    for suffix in [
        "onChanged: fn(",
        "onChanged: fn(text",
        "onChanged: fn(text: string",
    ] {
        let at = key + suffix.len();
        assert!(
            help_at(src, at).is_none(),
            "still reporting at {suffix:?}: {:?}",
            help_at(src, at).map(|h| h.signatures[0].label.clone())
        );
    }
}

/// An empty body is the case that matters most while typing: the
/// caret sits on a blank line in a callback that has no statements
/// yet, which is precisely when a stale popup is in the way.
#[test]
fn an_empty_callback_body_also_ends_it() {
    let src = "class Button
  fn init(label: string = \"\", onPressed: function? = nil)
  end
end

fn build()
  local b = Button(label: \"Save\", onPressed: fn()

  end)
end
";
    let at = src.find("fn()\n").expect("lambda") + "fn()\n".len();
    assert!(help_at(src, at).is_none(), "empty body kept the popup");
}

/// The barrier stops at the body — a call *written inside* it opens
/// its own popup as normal.
#[test]
fn a_call_inside_the_body_still_reports() {
    let src = "class Button
  fn init(label: string = \"\", onPressed: function? = nil)
  end
end

fn note(message: string, level: integer = 0)
end

fn build()
  local b = Button(label: \"Save\", onPressed: fn()
    note(\"saved\", 1)
  end)
end
";
    let at = src.find("note(\"saved\"").expect("call") + "note(".len();
    assert!(
        label_at(src, at).starts_with("note("),
        "{}",
        label_at(src, at)
    );
}

/// A `=>` lambda's body is one expression, still visibly an argument
/// of the call — so it is not a barrier.
#[test]
fn an_expression_lambda_is_not_a_barrier() {
    let src = "fn apply(items: table<integer>, f: fn(integer) -> integer, tag: string = \"\")
end

fn build()
  local out = apply({1}, x => x + 1, tag: \"t\")
end
";
    let at = src.find("x + 1").expect("body") + 1;
    assert!(
        label_at(src, at).starts_with("apply("),
        "got {}",
        label_at(src, at)
    );
}

/// Nested callbacks resolve against the innermost body, not an outer
/// one that also contains the cursor.
#[test]
fn the_innermost_body_wins() {
    let src = "class Button
  fn init(label: string = \"\", onPressed: function? = nil)
  end
end

fn note(message: string, level: integer = 0)
end

fn build()
  local b = Button(label: \"a\", onPressed: fn()
    local inner = Button(label: \"b\", onPressed: fn()
      note(\"deep\", 2)
    end)
  end)
end
";
    // Inside the inner callback's body, on its own call.
    let at = src.find("note(\"deep\"").expect("call") + "note(".len();
    assert!(label_at(src, at).starts_with("note("));
    // On the inner Button's own argument, the inner Button answers.
    let at = src.find("label: \"b\"").expect("arg") + 2;
    assert!(label_at(src, at).starts_with("Button("));
    // A statement in the inner body reports nothing at all.
    let at = src.find("local inner = Button").expect("stmt") + 2;
    assert!(help_at(src, at).is_none());
}

// ── Trailing blocks ─────────────────────────────────────────────────────────

/// Signature help inside a call's parentheses is unaffected by a trailing
/// block hanging off the end of it.
#[test]
fn signature_inside_the_parens_of_a_call_with_a_trailing_block() {
    let src = "\
fn repeated(times: integer, body: fn() -> nil) -> nil
  body()
end

fn main() -> nil
  repeated(2) do
    print(1)
  end
end
";
    let h = help(src, "repeated(2)", 9).expect("help");
    let sig = &h.signatures[0];
    assert!(sig.label.starts_with("repeated("), "label={}", sig.label);
    assert!(sig.label.contains("times: integer"), "label={}", sig.label);
    assert_eq!(h.active_parameter, Some(0));
}

/// A nested call written inside a trailing block gets its own signature help
/// — the block body is an ordinary statement context.
#[test]
fn signature_for_a_call_nested_in_a_trailing_block() {
    let src = "\
fn scaled(value: integer, by: integer) -> integer
  return value * by
end

fn repeated(times: integer, body: fn() -> nil) -> nil
  body()
end

fn main() -> nil
  repeated(2) do
    print(scaled(3, 4))
  end
end
";
    let h = help(src, "scaled(3", 7).expect("help");
    let sig = &h.signatures[0];
    assert!(sig.label.starts_with("scaled("), "label={}", sig.label);
    assert_eq!(h.active_parameter, Some(0));
}
