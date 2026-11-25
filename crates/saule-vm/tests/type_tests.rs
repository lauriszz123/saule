//! `v is T` at run time: what is emitted for it, and what the test answers
//! when the types involved come from several files.
//!
//! The fixtures under `tests/run/typetest/` cover the language. These pin
//! the two things only this crate can see: the instruction, and that a
//! class built from one module's layout answers for an interface declared
//! in another.

use std::path::{Path, PathBuf};

fn front_end(src: &str) -> saule_ast::Module {
    saule_runtime::init();
    let toks = saule_lexer::Lexer::new(src).tokenize().expect("lex");
    let mut module = saule_parser::parse(toks).expect("parse");
    let (errs, _) =
        saule_runtime::analyze_with_bindings(&module, saule_semantic::ModuleSeed::default());
    assert!(errs.is_empty(), "semantic errors in test source: {errs:?}");
    let terrs = saule_runtime::typeck::check_and_resolve(&mut module);
    assert!(terrs.is_empty(), "type errors in test source: {terrs:?}");
    module
}

fn listing(src: &str) -> String {
    let module = front_end(src);
    let chunk = saule_vm::compile(&module, "is.sau", src).expect("compiles");
    saule_vm::disasm::chunk(&chunk)
}

/// How many times `listing` emits `op`. Token-wise: every mnemonic is a
/// substring of some other one.
fn count(listing: &str, op: &str) -> usize {
    listing
        .lines()
        .filter(|l| l.split_whitespace().nth(1) == Some(op))
        .count()
}

#[test]
fn a_type_test_is_one_instruction() {
    let l = listing("fn f(v: any) -> boolean\n  return v is integer\nend\n");
    assert_eq!(count(&l, "ISTYPE"), 1, "{l}");
    // Not the cast and a compare against nil: `nil is string?` is true, and
    // a cast hands the nil back.
    assert_eq!(count(&l, "CASTCHK"), 0, "{l}");
}

#[test]
fn a_negated_type_test_is_the_test_and_a_not() {
    let l = listing("fn f(v: any) -> boolean\n  return v is not integer\nend\n");
    assert_eq!(count(&l, "ISTYPE"), 1, "{l}");
    assert_eq!(count(&l, "NOT"), 1, "{l}");
}

/// The tested type shares the table `as` interns its targets in, so the
/// same type tested and cast is one entry.
#[test]
fn a_test_and_a_cast_share_one_type_entry() {
    let src = "fn f(v: any) -> integer\n  if v is integer then\n    return 1\n  end\n  return (v as integer) ?? 0\nend\n";
    let module = front_end(src);
    let chunk = saule_vm::compile(&module, "is.sau", src).expect("compiles");
    assert_eq!(chunk.cast_types.len(), 1, "{:?}", chunk.cast_types);
}

/// An alias is gone before the compiler runs: the test is against the type.
#[test]
fn a_test_against_an_alias_is_a_test_against_its_type() {
    let src = "type Id = integer\nfn f(v: any) -> boolean\n  return v is Id\nend\n";
    let module = front_end(src);
    let chunk = saule_vm::compile(&module, "is.sau", src).expect("compiles");
    let tested: Vec<saule_ast::Type> = chunk.cast_types.iter().map(|t| (**t).clone()).collect();
    assert_eq!(tested, vec![saule_ast::Type::Named("integer".into())]);
}

// ── across files ──

fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("type_tests")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create project dir");
    for (file, body) in files {
        let path = dir.join(file);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create subdir");
        }
        std::fs::write(&path, body).expect("write module");
    }
    dir
}

fn run(name: &str, files: &[(&str, &str)]) -> String {
    saule_runtime::init();
    let dir = project(name, files);
    let entry: &Path = &dir.join("main.sau");
    let program = match saule_vm::program::compile(entry) {
        Ok(p) => p,
        Err(e) => panic!("expected `{name}` to compile: {e} | {e:?}"),
    };
    let (sink, ()) = saule_runtime::output::capture(|| {
        saule_vm::run_program(program).expect("the program must run");
    });
    sink.text().trim().to_string()
}

const CONTRACTS: &str = "\
export interface Named
  fn name() -> string
end

export interface Greeter extends Named
  fn greet() -> string
end

export interface Heavy
  fn weight() -> integer
end
";

const PEOPLE: &str = "\
from contracts import Greeter

export class Entity
  id: integer = 0
end

export class Person extends Entity implements Greeter
  fn name() -> string
    return \"Ann\"
  end

  fn greet() -> string
    return \"hello\"
  end
end

export class Student extends Person
end

export fn anyone() -> any
  return Student()
end
";

/// The interface is declared in one module, implemented in a second, and
/// asked about in a third.
#[test]
fn a_class_answers_for_an_interface_from_another_module() {
    let out = run(
        "interfaces",
        &[
            ("contracts.sau", CONTRACTS),
            ("people.sau", PEOPLE),
            (
                "main.sau",
                "from contracts import Named, Greeter, Heavy\n\
                 from people import Entity, Person, anyone\n\
                 local p: Entity = Person()\n\
                 local s: any = anyone()\n\
                 println(p is Person, p is Greeter, p is Named, p is Heavy)\n\
                 println(s is Person, s is Entity, s is Greeter, s is Named, s is Heavy)\n\
                 local plain: any = Entity()\n\
                 println(plain is Entity, plain is Named, plain is Person)\n",
            ),
        ],
    );
    assert_eq!(
        out,
        "true\ttrue\ttrue\tfalse\ntrue\ttrue\ttrue\ttrue\tfalse\ntrue\tfalse\tfalse"
    );
}

/// Narrowing to the interface makes its methods callable, through the
/// class's own method table for it.
#[test]
fn narrowing_to_an_imported_interface_dispatches() {
    let out = run(
        "dispatch",
        &[
            ("contracts.sau", CONTRACTS),
            ("people.sau", PEOPLE),
            (
                "main.sau",
                "from contracts import Greeter, Named\n\
                 from people import anyone\n\
                 local who = anyone()\n\
                 if who is Greeter then\n\
                 \x20 println(who.greet(), who.name())\n\
                 end\n\
                 if who is Named then\n\
                 \x20 println(who.name())\n\
                 end\n",
            ),
        ],
    );
    assert_eq!(out, "hello\tAnn\nAnn");
}

/// The checked `as` and a `catch` clause make the same test, so they now
/// answer for interfaces too — both used to say no to every instance.
#[test]
fn a_cast_and_a_catch_follow_interfaces_as_is_does() {
    let out = run(
        "cast_and_catch",
        &[
            ("contracts.sau", CONTRACTS),
            ("people.sau", PEOPLE),
            (
                "main.sau",
                "from contracts import Greeter, Heavy\n\
                 from people import anyone\n\
                 local who = anyone()\n\
                 local greeter = who as Greeter\n\
                 local heavy = who as Heavy\n\
                 println(greeter != nil, heavy == nil)\n\
                 try\n\
                 \x20 throw who\n\
                 catch h: Heavy\n\
                 \x20 println(\"heavy\")\n\
                 catch g: Greeter\n\
                 \x20 println(\"greeter \" .. g.greet())\n\
                 end\n",
            ),
        ],
    );
    assert_eq!(out, "true\ttrue\ngreeter hello");
}

/// A class imported under another name is still that class when tested.
#[test]
fn a_test_against_a_class_from_another_module() {
    let out = run(
        "classes",
        &[
            ("contracts.sau", CONTRACTS),
            ("people.sau", PEOPLE),
            (
                "main.sau",
                "from people import Entity, Person, Student, anyone\n\
                 local who = anyone()\n\
                 local kind = \"other\"\n\
                 if who is Student then\n\
                 \x20 kind = \"student\"\n\
                 elseif who is Person then\n\
                 \x20 kind = \"person\"\n\
                 end\n\
                 println(kind, who is Entity, Entity() is Student)\n",
            ),
        ],
    );
    assert_eq!(out, "student\ttrue\tfalse");
}

/// A value is asked what it is by the name its class was declared with, so
/// a test written with the name an import gave the class has to be turned
/// back into that one. `v is P` and `v as P` used to say no to everything.
#[test]
fn a_test_against_a_renamed_import_is_a_test_against_the_class() {
    let out = run(
        "renamed",
        &[
            ("contracts.sau", CONTRACTS),
            ("people.sau", PEOPLE),
            (
                "main.sau",
                "from people import Person as P, Entity as Base, anyone\n\
                 from contracts import Greeter as Hello\n\
                 local who = anyone()\n\
                 local list: any = {who}\n\
                 local three: any = 3\n\
                 println(who is P, who is Base, who is Hello, (who as P) != nil)\n\
                 println(list is table<P>, list is table<Base>, three is P?)\n",
            ),
        ],
    );
    assert_eq!(out, "true\ttrue\ttrue\ttrue\ntrue\ttrue\tfalse");
}

#[test]
fn a_test_against_a_namespaced_class_is_a_test_against_the_class() {
    let out = run(
        "namespaced",
        &[
            ("contracts.sau", CONTRACTS),
            ("people.sau", PEOPLE),
            (
                "main.sau",
                "import people\n\
                 local who = people.anyone()\n\
                 println(who is people.Person, who is people.Entity, (who as people.Student) != nil)\n",
            ),
        ],
    );
    assert_eq!(out, "true\ttrue\ttrue");
}
