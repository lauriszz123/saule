//! Pass 1: class layout (`VM_DESIGN.md` §8, §24.2).
//!
//! The prefix invariant is asserted directly, because the failure it
//! prevents is silent: a `GETF` compiled against a parent's slot reading a
//! different field on a subclass produces a wrong answer, not a crash.

use saule_lexer::Lexer;
use saule_parser::parse;
use saule_vm::compile::layout;

fn build(src: &str) -> (Vec<saule_vm::chunk::ClassProto>, layout::Layouts) {
    saule_interpreter::init();
    let toks = Lexer::new(src).tokenize().expect("lex");
    let module = parse(toks).expect("parse");
    layout::build(&module).expect("layout")
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

class Leaf extends Mid
  fn init()
    self.super()
    self.d = 4
  end
  d: integer
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
"#;

#[test]
fn a_subclass_layout_extends_its_parents() {
    let (protos, idx) = build(HIERARCHY);
    let base = &protos[idx.get("Base").unwrap() as usize];
    let mid = &protos[idx.get("Mid").unwrap() as usize];
    let leaf = &protos[idx.get("Leaf").unwrap() as usize];

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
}

#[test]
fn a_subclass_is_laid_out_even_when_declared_before_its_parent() {
    // `Leaf` appears above `Mid` in the source. Pass 1 orders by inheritance
    // depth, not by source order, or the parent's slots would not exist yet.
    let (protos, idx) = build(HIERARCHY);
    let leaf = &protos[idx.get("Leaf").unwrap() as usize];
    assert_eq!(leaf.layout.len(), 4, "Leaf was built before Mid");
}

#[test]
fn a_vtable_extends_its_parents_and_overrides_in_place() {
    let (protos, idx) = build(HIERARCHY);
    let base = &protos[idx.get("Base").unwrap() as usize];
    let mid = &protos[idx.get("Mid").unwrap() as usize];
    let leaf = &protos[idx.get("Leaf").unwrap() as usize];

    // The slot a `CALLM` compiled against `Base` would use.
    let describe = base.vindex["describe"];
    let shared = base.vindex["shared"];
    assert_ne!(describe, shared);

    // An override takes the *same* slot, which is what makes dynamic
    // dispatch work without a lookup.
    assert_eq!(mid.vindex["describe"], describe);
    assert_eq!(leaf.vindex["describe"], describe);
    // An inherited method keeps its slot too.
    assert_eq!(leaf.vindex["shared"], shared);
    assert_eq!(leaf.vtable.len(), base.vtable.len());
}

#[test]
fn init_is_found_through_the_chain() {
    let (protos, idx) = build(HIERARCHY);
    for name in ["Base", "Mid", "Leaf"] {
        let c = &protos[idx.get(name).unwrap() as usize];
        assert!(c.init.is_some(), "{name} has no init slot");
    }
}

#[test]
fn a_defaulted_field_becomes_a_static_when_there_is_no_init() {
    // Matching the tree-walker's rule in `eval/stmt/classes.rs`. If the two
    // engines disagreed here they would disagree about what `C.field` means.
    let (protos, idx) = build("class C\n  count: integer = 0\nend");
    let c = &protos[idx.get("C").unwrap() as usize];
    assert_eq!(c.layout.len(), 0, "the field should not be an instance slot");
    assert_eq!(c.n_statics, 1);
    assert!(c.sindex.contains_key("count"));
}

#[test]
fn a_defaulted_field_stays_an_instance_field_when_there_is_an_init() {
    let (protos, idx) = build(
        "class C\n  fn init()\n    self.count = 1\n  end\n  count: integer = 0\nend",
    );
    let c = &protos[idx.get("C").unwrap() as usize];
    assert_eq!(c.layout.slot("count"), Some(0));
    assert_eq!(c.n_statics, 0);
}

#[test]
fn static_methods_are_indexed_separately_from_instance_methods() {
    let (protos, idx) = build(
        "class C\n  static fn make() -> integer\n    return 1\n  end\n\
         \x20 fn use() -> integer\n    return 2\n  end\nend",
    );
    let c = &protos[idx.get("C").unwrap() as usize];
    assert!(c.smindex.contains_key("make"));
    assert!(!c.vindex.contains_key("make"));
    assert!(c.vindex.contains_key("use"));
}

#[test]
fn a_class_from_another_module_is_refused_rather_than_guessed() {
    // Guessing a parent's layout is precisely the §24.2 failure. Refusing is
    // the only safe answer until cross-module layouts land.
    saule_interpreter::init();
    let src = "class Child extends Missing\n  fn init()\n    self.x = 1\n  end\n  x: integer\nend";
    let toks = Lexer::new(src).tokenize().expect("lex");
    let module = parse(toks).expect("parse");
    match layout::build(&module) {
        Err(saule_vm::CompileError::Unsupported { thing, .. }) => {
            assert!(thing.contains("another module"), "{thing}");
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}
