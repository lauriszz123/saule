//! Phase 0 structural invariants: slot-based instance fields (0.2),
//! flattened method tables (0.3), and dense enum tags (0.4).
//!
//! These are properties of the runtime's *shape*, not of any program's
//! output, so no `.sau` fixture can observe them — a fixture would still
//! pass if `p.health` went back to a per-instance hash map. They are
//! asserted on the objects the VM builds, because compiled code depends on
//! them: `GETF` compiles a field name to a slot, and `SWITCH` compiles a
//! `match` to a jump indexed by tag (`VM_DESIGN.md` §8.2, §9.2).

use std::rc::Rc;

use saule_lexer::Lexer;
use saule_parser::parse;
use saule_runtime::Value;
use saule_runtime::value::{ClassObject, EnumObject};

/// Run a program and hand back the value of its last expression.
fn run(src: &str) -> Value {
    let toks = Lexer::new(src).tokenize().expect("lex");
    let mut module = parse(toks).expect("parse");
    saule_vm::check_and_run(&mut module, "shape.sau", src).expect("run")
}

/// The classes `names` denotes after running `src`, from one run — so a
/// parent and its subclass are the very objects the program used.
fn classes(src: &str, names: &[&str]) -> Vec<Rc<ClassObject>> {
    let Value::Table(t) = run(&format!(
        "{src}\nlocal all: table<any> = {{{}}}\nall",
        names.join(", ")
    )) else {
        panic!("expected a table");
    };
    let t = t.borrow();
    t.array
        .iter()
        .map(|v| match v {
            Value::Class(c) => Rc::clone(c),
            other => panic!("expected a class, got {other:?}"),
        })
        .collect()
}

/// The enum behind `variant` (`"Event.Quit"`) after running `src`. Reached
/// through a variant because an enum's own name does not typecheck as a
/// value.
fn enum_of(src: &str, variant: &str) -> Rc<EnumObject> {
    match run(&format!("{src}\n{variant}")) {
        Value::EnumVariant(v) => v.enum_obj.borrow().clone().expect("a declared enum"),
        other => panic!("expected an enum variant, got {other:?}"),
    }
}

const HIERARCHY: &str = r#"
class Base
  fn init()
    self.a = 1
    self.b = 2
  end
  a: integer
  b: integer

  fn describe() -> string
    return "base"
  end
  fn shared() -> string
    return "from base"
  end
end

class Mid extends Base
  fn init()
    self.super()
    self.c = 3
  end
  c: integer

  fn describe() -> string
    return "mid"
  end
end

class Leaf extends Mid
  fn init()
    self.super()
    self.d = 4
  end
  d: integer
end
"#;

#[test]
fn subclass_layout_is_a_prefix_extension_of_its_parent() {
    let [base, mid, leaf] = classes(HIERARCHY, &["Base", "Mid", "Leaf"])
        .try_into()
        .expect("three classes");

    // The invariant a compiled `GETF` depends on: a slot resolved against a
    // parent's layout means the same field in every subclass.
    for (name, slot) in [("a", 0u16), ("b", 1)] {
        assert_eq!(base.layout.slot(name), Some(slot));
        assert_eq!(mid.layout.slot(name), Some(slot), "`{name}` moved in Mid");
        assert_eq!(leaf.layout.slot(name), Some(slot), "`{name}` moved in Leaf");
    }
    assert_eq!(mid.layout.slot("c"), Some(2));
    assert_eq!(leaf.layout.slot("c"), Some(2));
    assert_eq!(leaf.layout.slot("d"), Some(3));

    assert_eq!(base.layout.len(), 2);
    assert_eq!(leaf.layout.len(), 4);
    assert_eq!(base.layout.slot("d"), None);
}

#[test]
fn method_tables_are_flattened() {
    let [leaf] = classes(HIERARCHY, &["Leaf"]).try_into().expect("one class");
    // An inherited method is present in the subclass's own table, so a
    // lookup is one probe rather than a walk up the chain.
    assert!(leaf.methods.contains_key("shared"), "not flattened");
    assert!(leaf.methods.contains_key("describe"));
}

#[test]
fn overridden_methods_still_dispatch_dynamically() {
    // The behaviour flattening must not change, checked through the
    // language rather than the data structure — the nearest override wins.
    match run(&format!(
        "{HIERARCHY}\nLeaf().describe() .. \"/\" .. Leaf().shared()"
    )) {
        Value::Str(s) => assert_eq!(s.as_str(), "mid/from base"),
        other => panic!("expected a string, got {other:?}"),
    }
}

#[test]
fn enum_tags_are_dense_and_follow_declaration_order() {
    let e = enum_of(
        r#"
enum Event
  Quit
  Code = 7
  Click(x: integer, y: integer)
  Key(code: integer)
end
"#,
        "Event.Quit",
    );

    assert_eq!(e.variant_count(), 4);
    assert_eq!(e.tag_of("Quit"), Some(0));
    assert_eq!(e.tag_of("Code"), Some(1));
    assert_eq!(e.tag_of("Click"), Some(2));
    assert_eq!(e.tag_of("Key"), Some(3));
    assert_eq!(e.tag_of("Nope"), None);

    // Singletons carry their tag and are reachable by it.
    assert_eq!(e.variants["Quit"].tag, 0);
    assert_eq!(e.variants["Code"].tag, 1);
    assert!(Rc::ptr_eq(
        e.variant_by_tag(0).unwrap(),
        &e.variants["Quit"]
    ));

    // A tuple variant has a tag but no singleton — each call builds a fresh
    // object, so there is nothing to hand back by tag.
    assert!(e.by_tag[2].is_none());
    assert!(e.variant_by_tag(2).is_none());
}

#[test]
fn constructed_tuple_variants_carry_their_declaration_tag() {
    let src = r#"
enum Event
  Quit
  Click(x: integer, y: integer)
end
local c = Event.Click(3, 4)
c
"#;
    match run(src) {
        Value::EnumVariant(v) => {
            assert_eq!(v.variant_name, "Click");
            assert_eq!(v.tag, 1, "a constructed tuple variant lost its tag");
        }
        other => panic!("expected an enum variant, got {other:?}"),
    }
}
