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
        _ => Vec::new(),
    };
    filter(items, &prefix)
        .into_iter()
        .map(|i| i.label)
        .collect()
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
