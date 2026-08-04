use super::*;

/// Complete at the `@` marker. Mirrors the request handler minus the
/// cross-file seeding, which needs a real workspace.
fn complete(marked: &str) -> Vec<String> {
    let offset = marked.find('@').expect("no `@` caret marker");
    let src = marked.replace('@', "");
    let (patched, prefix) = splice_sentinel(&src, offset).expect("splice");
    let Some(module) = parse_tolerant(&patched) else {
        return Vec::new();
    };
    let _ = saule_semantic::analyze(&module);
    let Some(found) = Walk::run(&module) else {
        return Vec::new();
    };
    let items = match &found.ctx {
        Ctx::BaseClass { exclude } => class_items(exclude),
        Ctx::Interfaces { exclude } => interface_items(exclude),
        Ctx::TypeName => type_items(),
        Ctx::Value { stmt_start } => value_items(&found, &module, *stmt_start),
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
    let Some(module) = parse_tolerant(&patched) else {
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

/// The header keywords themselves are never suggested.
#[test]
fn nothing_before_the_keyword() {
    assert!(complete(&format!("{DECLS}class Player @")).is_empty());
}

/// Type positions are unaffected — they still see enums and primitives.
#[test]
fn type_position_still_offers_everything() {
    let got = complete(&format!("{DECLS}class Player\n    hue: @\nend\n"));
    assert!(got.contains(&"Colour".to_string()));
    assert!(got.contains(&"string".to_string()));
    assert!(got.contains(&"Drawable".to_string()));
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
