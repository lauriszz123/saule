//! The checker knows the types a program's imports lead to, not only the
//! ones a file imports by name.
//!
//! `Outer` has a field `inner: Inner`, declared in `Outer.sau` with `Inner`
//! imported there. A file that imports only `Outer` used to know nothing of
//! `Inner`: `o.inner.weight` had no type, an untyped value goes unchecked,
//! and `1.0 * o.inner.weight` passed `saule check` and failed at run time.
//! The same gap made an inherited method uncallable without importing the
//! base class.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// A scratch project: `Inner`, `Outer` holding one, `Base` and `Child`.
fn project(name: &str, main: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("reachable_types")
        .join(name);
    let _ = std::fs::remove_dir_all(&root);
    let files = [
        (
            "saule.config",
            "name: \"probe\"\nversion: \"0.1.0\"\nentry: \"src/main.sau\"\nsrc_dirs: [\"src\"]\n",
        ),
        (
            "src/Inner.sau",
            "export class Inner\n  weight: integer\n\n  fn init()\n    self.weight = 100\n  end\nend\n",
        ),
        (
            "src/Outer.sau",
            "from Inner import Inner\n\nexport class Outer\n  inner: Inner\n\n  fn init()\n    self.inner = Inner()\n  end\n\n  fn get() -> Inner\n    return self.inner\n  end\nend\n",
        ),
        (
            "src/Base.sau",
            "export class Base\n  fn init()\n  end\n\n  fn hello() -> string\n    return \"hello\"\n  end\nend\n",
        ),
        (
            "src/Child.sau",
            "from Base import Base\n\nexport class Child extends Base\n  fn init()\n    self.super()\n  end\nend\n",
        ),
        ("src/main.sau", main),
    ];
    for (rel, body) in files {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("create a directory");
        std::fs::write(&path, body).expect("write a fixture");
    }
    root
}

/// The messages `saule check --json` reports for `main.sau`.
fn messages(root: &Path) -> Vec<String> {
    let out = Command::new(env!("CARGO_BIN_EXE_saule"))
        .arg("check")
        .arg(root.join("src/main.sau"))
        .arg("--json")
        .output()
        .expect("run saule check");
    let doc: Value = serde_json::from_slice(&out.stdout).expect("JSON on stdout");
    doc["files"]
        .as_array()
        .expect("files")
        .iter()
        .flat_map(|f| f["diagnostics"].as_array().expect("diagnostics").clone())
        .filter_map(|d| d["message"].as_str().map(str::to_string))
        .collect()
}

#[test]
fn a_field_of_a_class_not_imported_by_name_is_typed() {
    let root = project(
        "field",
        "from Outer import Outer\n\nlocal o = Outer()\nlocal a = 1.0 * o.inner.weight\nlocal b = 1.0 * o.get().weight\nlocal s: string = o.inner.weight\n",
    );
    let found = messages(&root);
    let mixed = found
        .iter()
        .filter(|m| m.contains("cannot mix `integer` and `float`"))
        .count();
    assert_eq!(mixed, 2, "both mixed products are caught: {found:?}");
    assert!(
        found
            .iter()
            .any(|m| m
                .contains("cannot assign value of type `integer` to variable of type `string`")),
        "the field's type is known exactly: {found:?}"
    );
}

#[test]
fn an_inherited_method_is_callable_without_importing_the_base() {
    let root = project(
        "inherited",
        "from Child import Child\n\nlocal s: string = Child().hello()\n",
    );
    let found = messages(&root);
    assert!(found.is_empty(), "{found:?}");
}

/// Knowing a type is not the same as being able to name it: a class the
/// file never imported is still not in scope.
#[test]
fn a_class_reached_but_not_imported_cannot_be_named() {
    let root = project("named", "from Outer import Outer\n\nlocal i = Inner()\n");
    let found = messages(&root);
    assert!(
        found.iter().any(|m| m.contains("undefined name `Inner`")),
        "{found:?}"
    );
}

/// A file's own class of the same name is what the name means there.
#[test]
fn a_files_own_class_wins_over_one_it_reaches() {
    let root = project(
        "own",
        "from Outer import Outer\n\nclass Inner\n  label: string\n\n  fn init()\n    self.label = \"mine\"\n  end\nend\n\nlocal mine: string = Inner().label\n",
    );
    let found = messages(&root);
    assert!(found.is_empty(), "{found:?}");
}
