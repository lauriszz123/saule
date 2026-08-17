//! Phase 0 structural invariants: slot-based instance fields (0.2),
//! flattened method tables (0.3), and dense enum tags (0.4).
//!
//! These are properties of the runtime's *shape*, not of any program's
//! output, so no `.sau` fixture can observe them — a fixture would still
//! pass if `p.health` went back to a per-instance hash map. They are
//! asserted here because the bytecode compiler will depend on them:
//! `GETF` compiles a field name to a slot, and `SWITCH` compiles a `match`
//! to a jump indexed by tag (`VM_DESIGN.md` §8.2, §9.2).

use std::rc::Rc;

use saule_interpreter::value::{ClassObject, EnumObject, Value};
use saule_interpreter::{Environment, Value as V, run_in};
use saule_lexer::Lexer;
use saule_parser::parse;

/// Run a program and hand back the environment it ran in, so a test can
/// inspect the class and enum objects it declared.
fn run_and_env(src: &str) -> Rc<std::cell::RefCell<Environment>> {
    let toks = Lexer::new(src).tokenize().expect("lex");
    let module = parse(toks).expect("parse");
    let env = Environment::with_prelude();
    run_in(&module, &env).expect("run");
    env
}

fn class_named(env: &Rc<std::cell::RefCell<Environment>>, name: &str) -> Rc<ClassObject> {
    match env.borrow().get(name) {
        Some(V::Class(c)) => c,
        other => panic!("expected class `{name}`, got {other:?}"),
    }
}

fn enum_named(env: &Rc<std::cell::RefCell<Environment>>, name: &str) -> Rc<EnumObject> {
    match env.borrow().get(name) {
        Some(V::Enum(e)) => e,
        other => panic!("expected enum `{name}`, got {other:?}"),
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
    let env = run_and_env(HIERARCHY);
    let base = class_named(&env, "Base");
    let mid = class_named(&env, "Mid");
    let leaf = class_named(&env, "Leaf");

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
fn method_tables_are_flattened_and_overrides_win() {
    let env = run_and_env(HIERARCHY);
    let base = class_named(&env, "Base");
    let mid = class_named(&env, "Mid");
    let leaf = class_named(&env, "Leaf");

    // An inherited method is present in the subclass's own table, so a
    // lookup is one probe rather than a walk up the chain.
    assert!(leaf.methods.contains_key("shared"), "not flattened");
    assert!(leaf.methods.contains_key("describe"));

    // Two levels down, the nearest override wins.
    assert!(Rc::ptr_eq(
        &tree_method(&leaf, "describe"),
        &tree_method(&mid, "describe")
    ));
    assert!(!Rc::ptr_eq(
        &tree_method(&leaf, "describe"),
        &tree_method(&base, "describe")
    ));

    // Inherited entries are shared, not copied.
    assert!(Rc::ptr_eq(
        &tree_method(&leaf, "shared"),
        &tree_method(&base, "shared")
    ));
}

/// The `FunctionObject` behind a method of a tree-walker-built class.
///
/// `lookup_method` returns a `MethodRef` because a class the *bytecode*
/// engine built holds compiled closures instead. These classes come from
/// `exec_class_decl`, so the `Tree` arm always fires.
fn tree_method(
    class: &Rc<saule_interpreter::value::ClassObject>,
    name: &str,
) -> Rc<saule_interpreter::value::FunctionObject> {
    Rc::clone(
        class
            .lookup_method(name)
            .expect("method present")
            .as_tree()
            .expect("declared by the tree-walker"),
    )
}

#[test]
fn flattening_does_not_steal_the_parents_owner_class() {
    // The trap: an inherited entry is the *same* `Rc<FunctionObject>` the
    // parent holds, so re-pointing its owner when the subclass is built
    // would rewrite the parent's method to belong to the subclass.
    let env = run_and_env(HIERARCHY);
    let base = class_named(&env, "Base");
    let owner = tree_method(&base, "shared")
        .resolved_owner()
        .expect("owner set");
    assert!(
        Rc::ptr_eq(&owner, &base),
        "`Base.shared` now claims to belong to `{}`",
        owner.name
    );
}

#[test]
fn overridden_methods_still_dispatch_dynamically() {
    // The behaviour flattening must not change, checked through the language
    // rather than the data structure.
    let env = run_and_env(&format!("{HIERARCHY}\nlocal out = Leaf().describe()"));
    match env.borrow().get("out") {
        Some(V::Str(s)) => assert_eq!(s.as_str(), "mid"),
        other => panic!("expected \"mid\", got {other:?}"),
    }
}

#[test]
fn enum_tags_are_dense_and_follow_declaration_order() {
    let env = run_and_env(
        r#"
enum Event
  Quit
  Code = 7
  Click(x: integer, y: integer)
  Key(code: integer)
end
"#,
    );
    let e = enum_named(&env, "Event");

    assert_eq!(e.variant_count(), 4);
    assert_eq!(e.tag_of("Quit"), Some(0));
    assert_eq!(e.tag_of("Code"), Some(1));
    assert_eq!(e.tag_of("Click"), Some(2));
    assert_eq!(e.tag_of("Key"), Some(3));
    assert_eq!(e.tag_of("Nope"), None);

    // Singletons carry their tag and are reachable by it.
    assert_eq!(e.variants["Quit"].tag, 0);
    assert_eq!(e.variants["Code"].tag, 1);
    assert!(Rc::ptr_eq(e.variant_by_tag(0).unwrap(), &e.variants["Quit"]));

    // A tuple variant has a tag but no singleton — each call builds a fresh
    // object, so there is nothing to hand back by tag.
    assert!(e.by_tag[2].is_none());
    assert!(e.variant_by_tag(2).is_none());
}

#[test]
fn constructed_tuple_variants_carry_their_declaration_tag() {
    let env = run_and_env(
        r#"
enum Event
  Quit
  Click(x: integer, y: integer)
end
local c = Event.Click(3, 4)
"#,
    );
    match env.borrow().get("c") {
        Some(V::EnumVariant(v)) => {
            assert_eq!(v.variant_name, "Click");
            assert_eq!(v.tag, 1, "a constructed tuple variant lost its tag");
        }
        other => panic!("expected an enum variant, got {other:?}"),
    }
}

#[test]
fn writing_an_undeclared_field_is_reported() {
    // Instances have a fixed shape now. The typechecker rejects this first;
    // this is the unchecked `run_in` path, where the old code would have
    // silently created a field nothing could later find by slot.
    let toks = Lexer::new(
        r#"
class P
  fn init()
    self.x = 1
  end
  x: integer
end
local p = P()
p.nope = 2
"#,
    )
    .tokenize()
    .expect("lex");
    let module = parse(toks).expect("parse");
    let env = Environment::with_prelude();
    let err = run_in(&module, &env).expect_err("writing an undeclared field must fail");
    let msg = format!("{err}");
    assert!(msg.contains("nope"), "unhelpful message: {msg}");
}

// Keep the unused-import lint quiet about the alias used only in signatures.
const _: Option<Value> = None;
