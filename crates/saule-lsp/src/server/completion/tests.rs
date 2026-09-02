use super::*;

/// Complete at the `@` marker. Mirrors the request handler minus the
/// cross-file seeding, which needs a real workspace.
fn complete(marked: &str) -> Vec<String> {
    let offset = marked.find('@').expect("no `@` caret marker");
    let src = marked.replace('@', "");
    let (patched, prefix) = splice_sentinel(&src, offset).expect("splice");
    let header = header_keywords(&src, offset);
    if !header.is_empty() {
        return filter(keyword_items(&header, "class header"), &prefix)
            .into_iter()
            .map(|i| i.label)
            .collect();
    }
    let Some(module) = parse_tolerant(&patched, None) else {
        return Vec::new();
    };
    let _ = saule_semantic::analyze(&module);
    let Some(found) = Walk::run(&module) else {
        if Walk::in_interface_body(&module, offset - prefix.len()) {
            return filter(
                keyword_items(INTERFACE_KEYWORDS, "interface member"),
                &prefix,
            )
            .into_iter()
            .map(|i| i.label)
            .collect();
        }
        return Vec::new();
    };
    let items = match &found.ctx {
        Ctx::BaseClass { exclude } => class_items(exclude),
        Ctx::Interfaces { exclude } => interface_items(exclude),
        Ctx::TypeName => type_items(),
        Ctx::Value { stmt_start } => value_items(&found, &module, *stmt_start),
        Ctx::AfterExport => export_items(),
        Ctx::ClassMember {
            is_static,
            is_private,
        } => class_member_items(*is_static, *is_private),
        _ => Vec::new(),
    };
    filter(items, &prefix)
        .into_iter()
        .map(|i| i.label)
        .collect()
}

/// Complete at `@` and return `(label, detail)` pairs, so tests can assert on
/// the type a binding is offered with and not merely that it is offered.
fn complete_detailed(marked: &str) -> Vec<(String, Option<String>)> {
    let offset = marked.find('@').expect("no `@` caret marker");
    let src = marked.replace('@', "");
    let (patched, prefix) = splice_sentinel(&src, offset).expect("splice");
    let Some(module) = parse_tolerant(&patched, None) else {
        return Vec::new();
    };
    let _ = saule_semantic::analyze(&module);
    let Some(found) = Walk::run(&module) else {
        return Vec::new();
    };
    let items = match &found.ctx {
        Ctx::Value { stmt_start } => value_items(&found, &module, *stmt_start),
        _ => Vec::new(),
    };
    filter(items, &prefix)
        .into_iter()
        .map(|i| (i.label, i.detail))
        .collect()
}

/// Complete at `@` and return the labels in the order a client that honours
/// `sortText` puts them in — which is the order the author actually sees.
fn complete_ranked(marked: &str) -> Vec<String> {
    let offset = marked.find('@').expect("no `@` caret marker");
    let src = marked.replace('@', "");
    let (patched, prefix) = splice_sentinel(&src, offset).expect("splice");
    let Some(module) = parse_tolerant(&patched, None) else {
        return Vec::new();
    };
    let _ = saule_semantic::analyze(&module);
    let Some(found) = Walk::run(&module) else {
        return Vec::new();
    };
    let Ctx::Value { stmt_start } = &found.ctx else {
        return Vec::new();
    };
    let mut items = filter(value_items(&found, &module, *stmt_start), &prefix);
    items.sort_by(|a, b| a.sort_text.cmp(&b.sort_text));
    items.into_iter().map(|i| i.label).collect()
}

/// The detail string a completion offers for `name`, or `None` when the name
/// isn't offered at all.
fn detail_of(marked: &str, name: &str) -> Option<Option<String>> {
    complete_detailed(marked)
        .into_iter()
        .find(|(l, _)| l == name)
        .map(|(_, d)| d)
}

const DECLS: &str = "\
class Entity end
class Actor extends Entity end
interface Drawable
    fn draw()
end
interface Sized
    fn size() -> integer
end
enum Colour Red, Green end
";

/// Classes and nothing else — the interfaces and the enum declared above
/// are not base classes.
#[test]
fn extends_offers_classes_only() {
    let got = complete(&format!("{DECLS}class Player extends @"));
    assert!(got.contains(&"Entity".to_string()));
    assert!(got.contains(&"Actor".to_string()));
    for absent in ["Drawable", "Sized", "Colour", "string"] {
        assert!(!got.contains(&absent.to_string()), "{absent} offered");
    }
}

#[test]
fn extends_filters_by_prefix() {
    let got = complete(&format!("{DECLS}class Player extends En@"));
    assert_eq!(got, vec!["Entity"]);
}

/// A class can extend neither itself nor one of its own descendants.
#[test]
fn extends_excludes_cycles() {
    let got = complete(&format!("{DECLS}class Entity2 extends @\nend\n"));
    assert!(got.contains(&"Actor".to_string()));

    let got = complete(&format!("{DECLS}class Entity extends @"));
    assert!(!got.contains(&"Entity".to_string()));
    assert!(!got.contains(&"Actor".to_string()), "Actor extends Entity");
}

/// Interfaces — including the built-in ones — and nothing else. The
/// classes and the enum declared above must not show up.
#[test]
fn implements_offers_interfaces_only() {
    let got = complete(&format!("{DECLS}class Player implements @"));
    assert!(got.contains(&"Drawable".to_string()));
    assert!(got.contains(&"Sized".to_string()));
    assert!(got.contains(&"Iterable".to_string()), "built-in interface");
    for absent in ["Entity", "Actor", "Colour", "string"] {
        assert!(!got.contains(&absent.to_string()), "{absent} offered");
    }
}

/// Later entries in the list drop what's already been named.
#[test]
fn implements_drops_names_already_listed() {
    let got = complete(&format!("{DECLS}class Player implements Drawable, @"));
    assert!(!got.contains(&"Drawable".to_string()));
    assert!(got.contains(&"Sized".to_string()));
}

#[test]
fn interface_extends_offers_interfaces_minus_self() {
    let got = complete(&format!("{DECLS}interface Widget extends @"));
    assert!(got.contains(&"Drawable".to_string()));
    assert!(got.contains(&"Sized".to_string()));

    let got = complete(&format!("{DECLS}interface Drawable extends @"));
    assert!(!got.contains(&"Drawable".to_string()));
    assert!(got.contains(&"Sized".to_string()));
}

/// The header still parses once the body is there.
#[test]
fn works_with_a_complete_class_body() {
    let got = complete(&format!(
        "{DECLS}class Player extends @\n    name: string\n    fn go() end\nend\n"
    ));
    assert!(got.contains(&"Entity".to_string()));
    assert!(got.contains(&"Actor".to_string()));
}

/// The header keywords themselves, before either has been typed.
#[test]
fn a_class_header_offers_extends_and_implements() {
    assert_eq!(
        complete(&format!("{DECLS}class Player @")),
        vec!["extends", "implements"]
    );
    assert_eq!(
        complete(&format!("{DECLS}class Player ext@")),
        vec!["extends"]
    );
    assert_eq!(
        complete(&format!("{DECLS}export class Player impl@")),
        vec!["implements"]
    );
    // `extends` comes first in the header, so a parent still has a place.
    assert_eq!(
        complete(&format!("{DECLS}class Player extends Entity @")),
        vec!["implements"]
    );
    // …and once `implements` is written, it does not — the header is done,
    // and what follows is the body.
    let got = complete(&format!("{DECLS}class Player implements Drawable @"));
    assert!(!got.iter().any(|i| i == "extends" || i == "implements"), "{got:?}");
}

/// An interface has no `implements`.
#[test]
fn an_interface_header_offers_only_extends() {
    assert_eq!(
        complete(&format!("{DECLS}interface Shape ext@")),
        vec!["extends"]
    );
}

/// The class name itself is the author's to invent — nothing is offered
/// while it is being typed, and nothing inside a generic list either.
#[test]
fn nothing_before_the_keyword() {
    assert!(complete(&format!("{DECLS}class Play@")).is_empty());
    assert!(complete(&format!("{DECLS}class Player<T@")).is_empty());
    assert!(complete(&format!("{DECLS}-- class Player @")).is_empty());
}

/// A type position after the keyword still belongs to the tree, which knows
/// which names would close a cycle — the header check must not steal it.
#[test]
fn the_header_check_leaves_type_positions_alone() {
    let got = complete(&format!("{DECLS}class Player extends En@\nend\n"));
    assert_eq!(got, vec!["Entity"]);
    let got = complete(&format!("{DECLS}class Player implements Dr@\nend\n"));
    assert_eq!(got, vec!["Drawable"]);
}

/// A name the author is inventing gets no suggestions — offering the names
/// already in scope where a *new* one is being declared is pure noise. The
/// keyword positions added around these must not leak into them.
#[test]
fn a_declaration_name_is_never_completed() {
    // Each of these has something in scope that shares the typed prefix.
    for src in [
        "fn hop() end\nfn ho@\n",
        "class Hop end\nclass Ho@\n",
        "class Hop end\ninterface Ho@\n",
        "class Hop end\nenum Ho@\n",
        "fn hop() end\nfn f()\n    local ho@ = 1\nend\n",
        "fn hop() end\nexport ho@\n",
        "fn hop() end\nfn f(ho@)\nend\n",
        "fn hop() end\nfn f(a: integer, ho@)\nend\n",
        "fn hop() end\nfor ho@ in items do end\n",
        "fn hop() end\ntry\ncatch ho@\nend\n",
        "fn hop() end\nclass A\n    fn ho@()\n    end\nend\n",
        "fn hop() end\nclass A\n    ho@: integer\nend\n",
        "fn hop() end\ninterface A\n    fn ho@()\nend\n",
        "class Hop end\nenum Colour\n    Ho@\nend\n",
    ] {
        assert!(complete(src).is_empty(), "{src:?}: {:?}", complete(src));
    }
}

/// An interface body takes signatures and nothing else.
#[test]
fn an_interface_body_offers_fn() {
    assert_eq!(complete("interface Drawable\n    @\nend\n"), vec!["fn"]);
    assert_eq!(complete("interface Drawable\n    f@\nend\n"), vec!["fn"]);
    assert_eq!(
        complete("interface Drawable\n    fn draw()\n    @\nend\n"),
        vec!["fn"]
    );
    // Same line as the header, once the header itself is finished.
    assert_eq!(
        complete(&format!("{DECLS}interface Shape extends Drawable @\nend\n")),
        vec!["fn"]
    );
}

/// The interface fallback only answers where the tree found nothing — a
/// signature's own positions still resolve as themselves.
#[test]
fn an_interface_signature_is_not_a_member_position() {
    let got = complete(&format!(
        "{DECLS}interface Shape\n    fn draw(c: @)\nend\n"
    ));
    assert!(got.iter().any(|i| i == "string"), "{got:?}");
    let got = complete(&format!(
        "{DECLS}interface Shape\n    fn draw() -> @\nend\n"
    ));
    assert!(got.iter().any(|i| i == "Colour"), "{got:?}");
    // …and neither is the interface's own name.
    assert!(complete(&format!("{DECLS}interface Sh@")).is_empty());
}

/// A class body offers what can begin a member.
#[test]
fn a_class_body_offers_the_member_keywords() {
    assert_eq!(
        complete("class Player\n    @\nend\n"),
        vec!["static", "local", "fn"]
    );
    assert_eq!(
        complete("class Player\n    name: string\n    f@\nend\n"),
        vec!["fn"]
    );
}

/// A modifier already written is not offered a second time, and the ones it
/// rules out go with it.
#[test]
fn a_class_member_modifier_is_not_repeated() {
    assert_eq!(complete("class Player\n    static @\nend\n"), vec!["local", "fn"]);
    assert_eq!(complete("class Player\n    local @\nend\n"), vec!["static", "fn"]);
    assert_eq!(complete("class Player\n    static local @\nend\n"), vec!["fn"]);
    assert_eq!(complete("class Player\n    local static @\nend\n"), vec!["fn"]);
}

/// The member keywords stay inside the class body — a method body is
/// ordinary statement territory.
#[test]
fn a_method_body_is_not_a_member_position() {
    let got = complete("class Player\n    fn go()\n        @\n    end\nend\n");
    assert!(got.iter().any(|i| i == "local"), "{got:?}");
    assert!(got.iter().any(|i| i == "self"), "{got:?}");
    assert!(got.iter().any(|i| i == "if"), "{got:?}");
}

/// A field being named on the line *below* the header is not a header.
#[test]
fn a_field_name_is_not_a_header_position() {
    let got = complete(&format!("{DECLS}class Player\n    ext@\nend\n"));
    assert!(!got.iter().any(|i| i == "extends"), "{got:?}");
}

/// Type positions are unaffected — they still see enums and primitives.
#[test]
fn type_position_still_offers_everything() {
    let got = complete(&format!("{DECLS}class Player\n    hue: @\nend\n"));
    assert!(got.contains(&"Colour".to_string()));
    assert!(got.contains(&"string".to_string()));
    assert!(got.contains(&"Drawable".to_string()));
}

// ── Casts ───────────────────────────────────────────────────────────────────

/// A cast target is a type position: `v as <caret>` offers the names that
/// can stand there, not the values in scope.
#[test]
fn a_cast_target_offers_type_names() {
    let got = complete(&format!(
        "{DECLS}fn probe(v: any)\n    local x = v as @\nend\n"
    ));
    assert!(got.contains(&"integer".to_string()), "{got:?}");
    assert!(got.contains(&"Entity".to_string()), "{got:?}");
    assert!(got.contains(&"Colour".to_string()), "{got:?}");
    // The operand is a value, not a candidate for its own cast target.
    assert!(!got.contains(&"v".to_string()), "{got:?}");
}

/// Inside `table<...>` too — the target is walked as a whole type.
#[test]
fn a_nested_cast_target_offers_type_names() {
    let got = complete(&format!(
        "{DECLS}fn probe(v: any)\n    local x = v as table<@>\nend\n"
    ));
    assert!(got.contains(&"string".to_string()), "{got:?}");
}

/// The operand half stays a value position — a caret there used to be
/// invisible, because the walker had no arm for `Expr::Cast` at all.
#[test]
fn a_cast_operand_still_completes_values() {
    let src = "\
fn build()
  local speed = 10
  local n = spe@ as string
end
";
    let got = complete(src);
    assert!(got.iter().any(|i| i == "speed"), "{got:?}");
}

// ── Trailing blocks ─────────────────────────────────────────────────────────

const BLOCK_DECLS: &str = "\
fn each(items: table<integer>, body: fn(integer) -> nil) -> nil
    body(items[1])
end

fn repeated(times: integer, body: fn() -> nil) -> nil
    body()
end
";

/// A trailing block's own parameter is in scope inside it.
#[test]
fn completes_a_trailing_block_parameter_inside_the_block() {
    let src = format!(
        "{BLOCK_DECLS}
fn main() -> nil
    each({{1}}) do (item)
        @
    end
end
"
    );
    let got = complete(&src);
    assert!(got.contains(&"item".to_string()), "{got:?}");
}

/// The parameter is offered with the type the callee declared, not the `any`
/// an omitted annotation parses to.
#[test]
fn a_trailing_block_parameter_completes_with_its_inferred_type() {
    let src = format!(
        "{BLOCK_DECLS}
fn main() -> nil
    each({{1}}) do (item)
        @
    end
end
"
    );
    let detail = detail_of(&src, "item").expect("`item` should be offered");
    assert_eq!(
        detail.as_deref(),
        Some("parameter: integer"),
        "detail={detail:?}"
    );
}

/// Locals declared before the call are visible inside the block — a trailing
/// block body is an ordinary nested scope.
#[test]
fn completes_enclosing_locals_inside_a_trailing_block() {
    let src = format!(
        "{BLOCK_DECLS}
fn main() -> nil
    local label: string = \"hi\"
    repeated(times: 2) do
        @
    end
end
"
    );
    let got = complete(&src);
    assert!(got.contains(&"label".to_string()), "{got:?}");
}

/// Top-level functions stay reachable from inside a block, including the one
/// whose call the block is attached to.
#[test]
fn completes_top_level_functions_inside_a_trailing_block() {
    let src = format!(
        "{BLOCK_DECLS}
fn main() -> nil
    repeated(times: 2) do
        @
    end
end
"
    );
    let got = complete(&src);
    assert!(got.contains(&"each".to_string()), "{got:?}");
    assert!(got.contains(&"repeated".to_string()), "{got:?}");
}

/// A block's parameter is scoped to the block: it must not leak out past the
/// `end`, where the enclosing scope is what matters.
#[test]
fn a_trailing_block_parameter_does_not_leak_past_the_block() {
    let src = format!(
        "{BLOCK_DECLS}
fn main() -> nil
    each({{1}}) do (item)
        print(item)
    end
    @
end
"
    );
    let got = complete(&src);
    assert!(!got.contains(&"item".to_string()), "{got:?}");
}

/// Nested blocks stack: the inner block sees its own parameter and the outer
/// one's.
#[test]
fn completes_both_parameters_inside_nested_trailing_blocks() {
    let src = format!(
        "{BLOCK_DECLS}
fn main() -> nil
    each({{1}}) do (outer)
        each({{2}}) do (inner)
            @
        end
    end
end
"
    );
    let got = complete(&src);
    assert!(got.contains(&"outer".to_string()), "{got:?}");
    assert!(got.contains(&"inner".to_string()), "{got:?}");
}

/// The block binds to the parameter the slot rule gives it, so a named
/// argument ahead of it must not shift the type its parameter completes with.
/// Reading the block as slot 0 would type `n` as `integer`.
#[test]
fn a_trailing_block_after_a_named_argument_completes_from_the_right_slot() {
    let src = "\
fn view(spacing: integer, body: fn(string) -> nil) -> nil
    body(\"x\")
end

fn main() -> nil
    view(spacing: 10) do (n)
        @
    end
end
";
    let detail = detail_of(src, "n").expect("`n` should be offered");
    assert_eq!(
        detail.as_deref(),
        Some("parameter: string"),
        "detail={detail:?}"
    );
}

/// An explicit annotation on the block's parameter wins over the callee's.
///
/// Only a non-`any` annotation is observable: an omitted type parses to the
/// same `any` an explicit one does, so `do (item: any)` is indistinguishable
/// from `do (item)` and gets refined. That is the same rule the typechecker
/// and hover apply.
#[test]
fn an_explicit_trailing_block_param_type_is_not_overridden() {
    let src = "\
fn each(items: table<integer>, body: fn(integer) -> nil) -> nil
    body(items[1])
end

fn main() -> nil
    each({1}) do (item: string)
        @
    end
end
";
    let detail = detail_of(src, "item").expect("`item` should be offered");
    assert_eq!(
        detail.as_deref(),
        Some("parameter: string"),
        "detail={detail:?}"
    );
}

/// Refinement is not specific to the trailing spelling — the same call written
/// with an inline lambda argument completes identically.
#[test]
fn an_inline_lambda_argument_completes_with_its_inferred_type() {
    let src = "\
fn each(items: table<integer>, body: fn(integer) -> nil) -> nil
    body(items[1])
end

fn main() -> nil
    each({1}, fn(item)
        @
    end)
end
";
    let detail = detail_of(src, "item").expect("`item` should be offered");
    assert_eq!(
        detail.as_deref(),
        Some("parameter: integer"),
        "detail={detail:?}"
    );
}

/// Typing an argument offers the callee's parameter names, so
/// `Widget(back…)` completes to the `background:` keyword rather than
/// only to whatever locals happen to share the prefix.
#[test]
fn an_argument_position_offers_parameter_keywords() {
    let src = "\
class Color
end

class Widget
  fn init(background: Color?, keyRepeat: boolean, title: string)
  end
end

fn build()
  local w = Widget(@)
end
";
    let items = complete(src);
    for want in ["background:", "keyRepeat:", "title:"] {
        assert!(
            items.iter().any(|i| i == want),
            "missing {want:?}: {items:?}"
        );
    }
}

/// A parameter already supplied by name drops off the list — offering
/// it again would produce a duplicate keyword the parser rejects.
#[test]
fn parameters_already_named_are_not_offered_again() {
    let src = "\
class Widget
  fn init(alpha: integer, beta: integer, gamma: integer)
  end
end

fn build()
  local w = Widget(beta: 1, @)
end
";
    let items = complete(src);
    assert!(items.iter().any(|i| i == "alpha:"), "{items:?}");
    assert!(items.iter().any(|i| i == "gamma:"), "{items:?}");
    assert!(!items.iter().any(|i| i == "beta:"), "{items:?}");
}

/// Positional arguments consume their slots too, so only what's left
/// is offered.
#[test]
fn positional_arguments_consume_their_slots() {
    let src = "\
class Widget
  fn init(alpha: integer, beta: integer, gamma: integer)
  end
end

fn build()
  local w = Widget(1, @)
end
";
    let items = complete(src);
    assert!(!items.iter().any(|i| i == "alpha:"), "{items:?}");
    assert!(items.iter().any(|i| i == "beta:"), "{items:?}");
    assert!(items.iter().any(|i| i == "gamma:"), "{items:?}");
}

/// Past a `name:` the caret is writing a *value*, and parameter
/// keywords are the wrong suggestion there.
#[test]
fn a_named_arguments_value_is_not_a_keyword_position() {
    let src = "\
class Widget
  fn init(alpha: integer, beta: integer)
  end
end

fn build()
  local alphabet = 1
  local w = Widget(beta: al@)
end
";
    let items = complete(src);
    assert!(items.iter().any(|i| i == "alphabet"), "{items:?}");
    assert!(!items.iter().any(|i| i == "alpha:"), "{items:?}");
}

/// The innermost call wins — a nested call's parameters, not the
/// enclosing one's.
#[test]
fn a_nested_call_offers_its_own_parameters() {
    let src = "\
class Inner
  fn init(innerOne: integer)
  end
end

class Outer
  fn init(outerOne: integer)
  end
end

fn build()
  local w = Outer(Inner(@))
end
";
    let items = complete(src);
    assert!(items.iter().any(|i| i == "innerOne:"), "{items:?}");
    assert!(!items.iter().any(|i| i == "outerOne:"), "{items:?}");
}

/// Method calls get keywords too, including further down a chain —
/// `receiver_class` resolves `.font(28.0)` to `Text` so the next link
/// knows its parameters.
#[test]
fn a_method_call_in_a_chain_offers_parameter_keywords() {
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
end

fn build()
  local t = Text().font(28.0).foregroundStyle(@)
end
";
    let items = complete(src);
    assert!(items.iter().any(|i| i == "color:"), "{items:?}");
}

// ─── Completion survives a broken file ───────────────────────────────────────
//
// Before parser error recovery, every test below returned an empty list: one
// bad line anywhere in the buffer meant no tree, and no tree meant no
// suggestions. That is the state a file spends most of its editing life in,
// which is what made this the gap between "the plugin exists" and "the editor
// feels good".

/// A mistake *above* the caret used to take the whole file with it.
#[test]
fn a_broken_line_earlier_in_the_file_does_not_silence_completion() {
    let src = "\
fn setup()
  local half =
end

fn build()
  local speed = 10
  local total = spe@
end
";
    let items = complete(src);
    assert!(items.iter().any(|i| i == "speed"), "{items:?}");
}

/// …and so did one below it.
#[test]
fn a_broken_line_later_in_the_file_does_not_silence_completion() {
    let src = "\
fn build()
  local speed = 10
  local total = spe@
end

fn teardown()
  ) ] } =
end
";
    let items = complete(src);
    assert!(items.iter().any(|i| i == "speed"), "{items:?}");
}

/// The binding on the broken line itself stays in scope — that's what the
/// `Expr::Error` hole buys over dropping the statement.
#[test]
fn a_binding_whose_value_is_missing_is_still_offered() {
    let src = "\
fn build()
  local velocity =
  local n = velo@
end
";
    let items = complete(src);
    assert!(items.iter().any(|i| i == "velocity"), "{items:?}");
}

/// A class whose members are being typed still contributes to type contexts.
#[test]
fn a_class_with_one_broken_member_still_completes() {
    let src = "\
class Sprite
  fn = = =
  fn update(self) -> nil
  end
end

class Player extends Spr@
";
    let items = complete(src);
    assert!(items.iter().any(|i| i == "Sprite"), "{items:?}");
}

/// A lexical error is now survivable too: an unclosed quote costs its own
/// line and nothing else, where before it cost the whole file.
#[test]
fn an_unterminated_string_does_not_silence_completion() {
    let src = "\
fn build()
  local label = \"Play
  local speed = 10
  local total = spe@
end
";
    let items = complete(src);
    assert!(items.iter().any(|i| i == "speed"), "{items:?}");
}

/// And a stray character is simply not there. (`$` is the junk; `@` is this
/// harness's caret marker.)
#[test]
fn an_unexpected_character_does_not_silence_completion() {
    let src = "\
fn build()
  local speed = 10 $$
  local total = spe@
end
";
    let items = complete(src);
    assert!(items.iter().any(|i| i == "speed"), "{items:?}");
}

/// A half-typed declaration keyword at the start of a statement completes to
/// the keyword — the very first thing you write in an empty file.
#[test]
fn a_statement_start_completes_declaration_keywords() {
    for (src, want) in [
        ("en@", "enum"),
        ("cl@", "class"),
        ("class A\nend\n\nen@\n", "enum"),
        ("fn main()\n  cl@\nend\n", "class"),
    ] {
        let items = complete(src);
        assert!(items.iter().any(|i| i == want), "{src:?}: {items:?}");
    }
}

/// `export en…` is still a declaration keyword position, even though a bare
/// identifier there parses as an exported variable being named.
#[test]
fn export_completes_the_declaration_keywords() {
    assert_eq!(complete("export en@\n"), vec!["enum"]);
    assert_eq!(complete("export cl@\n"), vec!["class"]);
    assert_eq!(
        complete("export @\n"),
        vec!["fn", "class", "interface", "enum"]
    );
}

/// …but only while nothing follows it. Once the variable has a type or a
/// value, the author is naming it and keywords are the wrong suggestion.
#[test]
fn a_named_export_is_not_a_keyword_position() {
    assert!(complete("export cl@: integer = 1\n").is_empty());
}

// ── ranking by the slot being filled ────────────────────────────────────────

/// The corpus for argument ranking: a call whose parameters have real class
/// and enum types, and same-prefixed names of the wrong type to lose to them.
const SLOTS: &str = "\
class Alignment
    static fn center() -> Alignment
        return Alignment()
    end
end
class Align end
enum Axis Horizontal, Vertical end
class View
    fn aligned(alignment: Alignment) -> View
        return self
    end
end
fn stack(alignment: Alignment, axis: Axis, title: string) end
";

/// A named argument's value is ranked by the type that parameter holds — the
/// slot is the strongest signal there is, stronger than how close a name
/// looks to what has been typed.
#[test]
fn a_named_argument_ranks_the_expected_type_first() {
    let got = complete_ranked(&format!("{SLOTS}fn go()\n    stack(alignment: Ali@)\nend\n"));
    assert_eq!(got.first().map(String::as_str), Some("Alignment"), "{got:?}");
    // `Align` shares the prefix and is a class, but no `Align` fits the slot.
    let alignment = got.iter().position(|i| i == "Alignment");
    let align = got.iter().position(|i| i == "Align");
    assert!(alignment < align, "{got:?}");
}

/// …and an enum slot ranks the enum, not a same-prefixed anything else.
#[test]
fn an_enum_slot_ranks_the_enum_first() {
    let got = complete_ranked(&format!("{SLOTS}fn go()\n    stack(axis: A@)\nend\n"));
    assert_eq!(got.first().map(String::as_str), Some("Axis"), "{got:?}");
}

/// A local of the right type beats every global of the wrong one.
#[test]
fn a_local_of_the_expected_type_ranks_first() {
    let got = complete_ranked(&format!(
        "{SLOTS}fn go()\n    local a: Align = Align()\n    local al: Alignment = Alignment()\n    stack(alignment: a@)\nend\n"
    ));
    assert_eq!(got.first().map(String::as_str), Some("al"), "{got:?}");
}

/// Inside a method, a member ranks on what it yields: `aligned()` returns a
/// `View`, so it wins a `View` slot and loses an `Alignment` one.
#[test]
fn a_member_ranks_on_what_it_yields() {
    let src = "\
class Alignment end
class View
    fn aligned(a: Alignment) -> View
        return self
    end
    fn alignment() -> Alignment
        return Alignment()
    end
    fn go()
        self.aligned(a: ali@)
    end
end
";
    let got = complete_ranked(src);
    let alignment = got.iter().position(|i| i == "alignment");
    let aligned = got.iter().position(|i| i == "aligned");
    assert!(alignment.is_some() && alignment < aligned, "{got:?}");
}

/// A positional argument still offers the parameter *names* ahead of
/// everything — that is what is most likely being written there.
#[test]
fn parameter_names_outrank_values_at_a_positional_argument() {
    let got = complete_ranked(&format!("{SLOTS}fn go()\n    stack(al@)\nend\n"));
    assert_eq!(got.first().map(String::as_str), Some("alignment:"), "{got:?}");
}

/// Outside any argument there is no slot, so the order is untouched: a local
/// still beats a class, whatever the types are.
#[test]
fn no_slot_leaves_the_order_alone() {
    let got = complete_ranked(&format!(
        "{SLOTS}fn go()\n    local align: Align = Align()\n    local x = Al@\nend\n"
    ));
    assert_eq!(got.first().map(String::as_str), Some("align"), "{got:?}");
}

/// A slot declared `any` accepts everything, so it is no signal at all and
/// must not reshuffle the list.
#[test]
fn an_any_slot_is_not_a_signal() {
    let src = "\
class Alignment end
fn log(value: any) end
fn go()
    local always: Alignment = Alignment()
    log(al@)
end
";
    // The local first, then the class — the ordinary order. Were `any`
    // treated as a signal, `Alignment` would fit it and jump the queue.
    let got = complete_ranked(src);
    assert_eq!(got, vec!["always", "Alignment"], "{got:?}");
}

// ──────────────────────────────────────────────────────────────────────────────
// Enum variant payloads
// ──────────────────────────────────────────────────────────────────────────────

/// A tuple variant's payload is a type position like any other. The walker
/// used to descend into an enum's `methods` and nothing else, so the whole
/// variant list offered no suggestions at all.
#[test]
fn an_enum_variant_payload_offers_type_names() {
    let got = complete("enum Inline\n    Text(value: str@)\nend\n");
    assert!(got.contains(&"string".to_string()), "{got:?}");
}

/// …including the classes and enums declared alongside it, not just the
/// primitives.
#[test]
fn an_enum_variant_payload_offers_declared_types() {
    let src = "\
class Span end

enum Inline
    Text(value: string),
    Emph(children: Sp@)
end
";
    assert!(complete(src).contains(&"Span".to_string()));
}

/// A payload beyond the first is reached the same way.
#[test]
fn a_later_enum_variant_payload_field_offers_types() {
    let got = complete("enum Block\n    Heading(level: integer, slug: str@)\nend\n");
    assert!(got.contains(&"string".to_string()), "{got:?}");
}

/// A discriminant is an ordinary expression, so the caret in one completes
/// values rather than types.
#[test]
fn an_enum_discriminant_offers_values() {
    let src = "\
class Marker end

enum Status
    Alive = Mark@
end
";
    assert!(complete(src).contains(&"Marker".to_string()));
}
