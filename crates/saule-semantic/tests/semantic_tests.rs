//! Integration tests for `saule_semantic::analyze`.
//!
//! Semantic analysis is the pass between parsing and typechecking: it
//! resolves names, populates the class / interface / enum registries, and
//! enforces the rules that are about *structure* rather than types —
//! every field initialised, every path returning, `self` only inside a
//! method, `break` only inside a loop.
//!
//! Everything here drives the public [`analyze`] entry point over real
//! source text, so the tests describe the language rather than the
//! implementation and survive refactors of the internals.

use saule_ast::Module;
use saule_semantic::{SemanticError, analyze};

fn parse(src: &str) -> Module {
    let tokens = saule_lexer::Lexer::new(src)
        .tokenize()
        .unwrap_or_else(|e| panic!("source should lex: {e:?}\n{src}"));
    saule_parser::parse(tokens).unwrap_or_else(|e| panic!("source should parse: {e:?}\n{src}"))
}

/// Analyse `src` and return the errors.
fn errors(src: &str) -> Vec<SemanticError> {
    analyze(&parse(src))
}

/// Assert `src` analyses cleanly.
fn accepts(src: &str) {
    let errs = errors(src);
    assert!(
        errs.is_empty(),
        "expected no errors, got {errs:?}\n--- source ---\n{src}"
    );
}

/// Assert `src` produces at least one error whose rendering mentions
/// `needle`. Matching on the message keeps these readable while still
/// pinning the specific rule that fired.
fn rejects(src: &str, needle: &str) {
    let errs = errors(src);
    assert!(
        !errs.is_empty(),
        "expected an error mentioning {needle:?}, got none\n--- source ---\n{src}"
    );
    let joined = errs
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(
        joined.contains(needle),
        "expected an error mentioning {needle:?}, got: {joined}\n--- source ---\n{src}"
    );
}

// ─── name resolution ────────────────────────────────────────────────────

#[test]
fn undefined_name_is_reported() {
    rejects(
        "fn main() -> nil\n  println(nope)\n  return\nend\n",
        "nope",
    );
}

#[test]
fn locals_are_visible_after_declaration() {
    // No prelude here: `saule_semantic` learns names like `println` from
    // an embedder-installed provider, which only the interpreter sets.
    // Prelude resolution is covered by the `.sau` suite instead.
    accepts("fn f() -> integer\n  local x: integer = 1\n  return x\nend\n");
}

#[test]
fn parameters_are_in_scope_in_the_body() {
    accepts("fn f(a: integer) -> integer\n  return a\nend\n");
}

#[test]
fn a_local_does_not_escape_its_function() {
    rejects(
        "\
fn a() -> nil
  local secret: integer = 1
  return
end
fn b() -> nil
  println(secret)
  return
end
",
        "secret",
    );
}

#[test]
fn static_members_are_reachable_by_bare_name_inside_methods() {
    // Static fields, static methods and instance methods resolve
    // unqualified from inside any method of the same class.
    accepts(
        "\
class C
  static cap: integer = 10
  fn limit() -> integer
    return cap
  end
  fn again() -> integer
    return self.limit()
  end
end
",
    );
}

#[test]
fn instance_fields_are_not_reachable_by_bare_name() {
    // Instance *fields* are deliberately excluded from bare-name lookup —
    // they need `self.`. (The README's `Counter` example gets this wrong
    // and does not compile.)
    rejects(
        "\
class C
  count: integer = 0
  fn bump() -> integer
    return count + 1
  end
end
",
        "count",
    );
}

// ─── self / super ───────────────────────────────────────────────────────

#[test]
fn self_outside_a_class_is_rejected() {
    rejects(
        "fn f() -> nil\n  println(self)\n  return\nend\n",
        "self",
    );
}

#[test]
fn self_inside_a_method_is_fine() {
    accepts(
        "\
class C
  x: integer = 0
  fn get() -> integer
    return self.x
  end
end
",
    );
}

// ─── field initialisation ───────────────────────────────────────────────

#[test]
fn non_nullable_field_never_assigned_is_reported() {
    rejects(
        "\
class P
  name: string
  fn init()
  end
end
",
        "name",
    );
}

#[test]
fn field_with_a_default_is_initialised() {
    accepts("class P\n  name: string = \"anon\"\nend\n");
}

#[test]
fn field_assigned_in_init_is_initialised() {
    accepts(
        "\
class P
  name: string
  fn init(n: string)
    self.name = n
  end
end
",
    );
}

#[test]
fn nullable_field_needs_no_initialiser() {
    accepts("class P\n  nickname: string?\nend\n");
}

// ─── return paths ───────────────────────────────────────────────────────

#[test]
fn missing_return_on_some_path_is_reported() {
    rejects(
        "\
fn f(flag: boolean) -> integer
  if flag then
    return 1
  end
end
",
        "return",
    );
}

#[test]
fn return_on_every_branch_is_accepted() {
    accepts(
        "\
fn f(flag: boolean) -> integer
  if flag then
    return 1
  else
    return 2
  end
end
",
    );
}

// ─── loop control ───────────────────────────────────────────────────────

#[test]
fn break_outside_a_loop_is_rejected() {
    rejects("fn f() -> nil\n  break\n  return\nend\n", "loop");
}

#[test]
fn break_inside_a_loop_is_fine() {
    accepts(
        "\
fn f() -> nil
  while true do
    break
  end
  return
end
",
    );
}

// ─── parameter lists ────────────────────────────────────────────────────

#[test]
fn variadic_must_come_last() {
    rejects(
        "fn f(...rest: integer, tail: integer) -> nil\n  return\nend\n",
        "variadic",
    );
}

#[test]
fn only_one_variadic_parameter_is_allowed() {
    rejects(
        "fn f(...a: integer, ...b: integer) -> nil\n  return\nend\n",
        "variadic",
    );
}

// ─── declaration shapes that must keep analysing cleanly ────────────────

#[test]
fn inheritance_and_interfaces_analyse() {
    accepts(
        "\
interface Drawable
  fn draw() -> nil
end

class Shape
  sides: integer = 0
end

class Square extends Shape implements Drawable
  fn init()
    self.sides = 4
  end
  fn draw() -> nil
    return
  end
end
",
    );
}

#[test]
fn enums_with_variants_and_methods_analyse() {
    accepts(
        "\
enum Direction
  North
  South
  fn flip() -> Direction
    return self
  end
end
",
    );
}

// ── Compound assignment ──────────────────────────────────────────────────

#[test]
fn compound_assignment_requires_a_declared_target() {
    rejects("zzz += 1", "zzz");
    accepts("local n: integer = 0\nn += 1");
}

#[test]
fn compound_assignment_resolves_names_in_the_value() {
    rejects("local n: integer = 0\nn += nope", "nope");
}

#[test]
fn compound_assignment_does_not_initialise_a_field() {
    // `self.n += 1` reads `self.n` before writing it, so it cannot be what
    // brings the field into existence — the definite-initialisation check
    // must still fire.
    rejects(
        r#"
        class C
            n: integer
            fn init()
                self.n += 1
            end
        end
        "#,
        "n",
    );
    accepts(
        r#"
        class C
            n: integer
            fn init()
                self.n = 0
                self.n += 1
            end
        end
        "#,
    );
}
