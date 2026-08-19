//! Whole-program compilation across modules (`VM_DESIGN.md` §14, §24.2).
//!
//! The property these tests exist for is the one §24.2 calls the worst bug
//! this project could ship: a class laid out twice, once where it is declared
//! and once where it is extended, so a `GETF` compiled against one reads a
//! field of the other. A program-global class table makes that
//! unrepresentable — these assert that it really is one table.

use std::path::{Path, PathBuf};

/// Write a throwaway project and hand back its directory.
///
/// Files are written under the crate's `target/` so a failing test leaves
/// them behind for inspection rather than in a system temp directory.
fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
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

fn compile(entry: &Path) -> saule_vm::program::Program {
    saule_interpreter::init();
    match saule_vm::program::compile(entry) {
        Ok(p) => p,
        Err(e) => panic!("expected `{}` to compile: {e}", entry.display()),
    }
}

#[test]
fn a_subclass_extends_a_parent_from_another_module() {
    // The 24-file cause, end to end. `Derived`'s field slots must extend
    // `Base`'s *real* ones, which is only true if both were laid out into
    // the same table.
    let dir = project(
        "cross_module_inherit",
        &[
            (
                "shapes.sau",
                "export class Base\n\
                 \x20 fn init()\n\
                 \x20   self.a = 1\n\
                 \x20 end\n\
                 \x20 a: integer\n\
                 \x20 fn describe() -> string\n\
                 \x20   return \"base\"\n\
                 \x20 end\n\
                 end\n",
            ),
            (
                "main.sau",
                "import Base from shapes\n\
                 class Derived extends Base\n\
                 \x20 fn init()\n\
                 \x20   self.super()\n\
                 \x20   self.b = 2\n\
                 \x20 end\n\
                 \x20 b: integer\n\
                 end\n\
                 class Main\n\
                 \x20 static fn main()\n\
                 \x20   local d: Derived = Derived()\n\
                 \x20   println(d.describe())\n\
                 \x20 end\n\
                 end\n",
            ),
        ],
    );

    let program = compile(&dir.join("main.sau"));

    // Post-order: the imported module is compiled — and will be run —
    // before the module that imports it.
    assert_eq!(program.modules.len(), 2);
    assert_eq!(program.entry, 1, "the entry module comes last in post-order");

    // One table, seen identically through either chunk. `Rc::ptr_eq` is the
    // assertion that matters: two equal-looking tables would still be the
    // §24.2 bug waiting to happen.
    let a = &program.modules[0];
    let b = &program.modules[1];
    assert!(
        std::rc::Rc::ptr_eq(&a.classes, &b.classes),
        "every module of a program must share one class table"
    );

    let base = a.classes.iter().position(|c| c.name.as_ref() == "Base").expect("Base");
    let derived = a
        .classes
        .iter()
        .position(|c| c.name.as_ref() == "Derived")
        .expect("Derived");
    assert_eq!(
        a.classes[derived].parent,
        Some(base as u32),
        "the subclass must point at the parent's program-global index"
    );
    // The prefix invariant across a module boundary.
    assert_eq!(a.classes[derived].layout.slot("a"), Some(0));
    assert_eq!(a.classes[derived].layout.slot("b"), Some(1));
    // And the inherited method is reachable through the subclass's vtable.
    let slot = a.classes[derived].vindex.get("describe").copied().expect("describe slot");
    assert_ne!(
        a.classes[derived].vtable[slot as usize],
        u32::MAX,
        "an inherited method must be filled in, not left as a placeholder"
    );
}

#[test]
fn an_import_cycle_is_refused_rather_than_looping() {
    let dir = project(
        "cyclic_modules",
        &[
            ("a.sau", "import B from b\nexport class A\nend\n"),
            ("b.sau", "import A from a\nexport class B\nend\n"),
        ],
    );
    saule_interpreter::init();
    match saule_vm::program::compile(&dir.join("a.sau")) {
        Err(e @ saule_vm::program::ProgramError::Circular { .. }) => {
            assert!(e.is_fallback(), "a cycle must fall back, not fail the run");
        }
        other => panic!("expected a circular-import refusal, got {other:?}"),
    }
}

#[test]
fn an_imported_function_is_copied_into_the_importing_module_slot() {
    // An imported name gets a module slot, and the prologue copies the
    // exporter's value into it. Getting this wrong reads `nil` silently,
    // so it is asserted rather than assumed.
    let dir = project(
        "import_value",
        &[
            ("lib.sau", "export fn double(n: integer) -> integer\n  return n * 2\nend\n"),
            (
                "main.sau",
                "import double from lib\n\
                 class Main\n\
                 \x20 static fn main()\n\
                 \x20   println(double(21))\n\
                 \x20 end\n\
                 end\n",
            ),
        ],
    );
    let program = compile(&dir.join("main.sau"));
    assert_eq!(program.modules.len(), 2);

    // Slot spaces are laid end to end, so the importer's slots start where
    // the exporter's stop. That is what lets the copy be a plain `GETMOD` +
    // `SETMOD` — two indices into one vector, no cross-module opcode.
    let lib = &program.modules[0];
    let main = &program.modules[1];
    assert_eq!(lib.module_slot_base, 0);
    assert_eq!(main.module_slot_base, lib.module_slots);

    // And it runs: the value really arrives, rather than the slot reading
    // `nil` and the call failing somewhere else.
    let out = run_capturing(program);
    assert_eq!(out.trim(), "42");
}

/// Run a program and capture what it printed.
fn run_capturing(program: saule_vm::program::Program) -> String {
    let (sink, ()) = saule_interpreter::output::capture(|| {
        saule_vm::run_program(program).expect("the program must run");
    });
    sink.text()
}

#[test]
fn a_module_top_level_runs_before_the_module_that_imports_it() {
    // Post-order is observable: the tree-walker runs an imported module's
    // top level on first import, so a module that prints at the top level
    // prints first. Any other order is a visible divergence.
    let dir = project(
        "module_init_order",
        &[
            ("first.sau", "println(\"first\")\nexport class Marker\nend\n"),
            (
                "main.sau",
                "import Marker from first\n\
                 println(\"second\")\n\
                 class Main\n\
                 \x20 static fn main()\n\
                 \x20   println(\"main\")\n\
                 \x20 end\n\
                 end\n",
            ),
        ],
    );
    let out = run_capturing(compile(&dir.join("main.sau")));
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines, ["first", "second", "main"]);
}

#[test]
fn a_named_argument_binds_to_an_imported_classs_constructor() {
    // §19 argument binding across a module boundary.
    //
    // `layouts` has been program-global since the imports slice, so `Box`
    // itself resolved fine — but `callee_params` was rebuilt from scratch per
    // module, so the *parameter list* needed to turn `label:` into a position
    // did not exist for a class declared elsewhere. The call refused with
    // `a named argument to a callee the compiler cannot identify`, which is
    // what made `ui-blocks` and `todo-app` fall back on their first
    // `Panel(title: …)`.
    //
    // Both orders are asserted: `width:` before `label:` is the one that
    // proves the reorder ran, since a compiler that merely dropped the names
    // and kept the written order would still pass the first case.
    let dir = project(
        "named_arg_imported_ctor",
        &[
            (
                "boxes.sau",
                "export class Box\n\
                 \x20 fn init(label: string, width: integer = 10)\n\
                 \x20   self.label = label\n\
                 \x20   self.width = width\n\
                 \x20 end\n\
                 \x20 label: string\n\
                 \x20 width: integer\n\
                 \x20 fn show() -> string\n\
                 \x20   return self.label .. \":\" .. tostring(self.width)\n\
                 \x20 end\n\
                 end\n",
            ),
            (
                "main.sau",
                "import Box from boxes\n\
                 class Main\n\
                 \x20 static fn main()\n\
                 \x20   println(Box(label: \"a\").show())\n\
                 \x20   println(Box(width: 3, label: \"b\").show())\n\
                 \x20 end\n\
                 end\n",
            ),
        ],
    );

    // The assertion is as much that this *compiles* as that it prints: before
    // the fix `compile` returned `Unsupported` and the CLI fell back.
    let out = run_capturing(compile(&dir.join("main.sau")));
    assert_eq!(out.trim(), "a:10\nb:3");
}

#[test]
fn a_top_level_fns_parameters_do_not_leak_across_modules() {
    // The boundary the fix deliberately does not cross. `CalleeKey::Method`
    // is keyed on a program-global `ClassIdx` and accumulates safely; a
    // top-level `fn` is keyed on a bare **name**, so it is published by
    // *slot* and seeded only through the importer's own `ImportBinding`.
    //
    // Both modules declare `fn tag`, with the two parameters in opposite
    // order, and `main` does **not** import lib's. A name-keyed accumulation
    // would let the module compiled first answer for the one compiled second
    // and silently swap the arguments — a wrong answer, not a fallback, and
    // the shadowing family of trap 1. `main` imports `seed` only to force the
    // dependency edge, so lib is guaranteed to be compiled first.
    let dir = project(
        "fn_params_no_leak",
        &[
            (
                "lib.sau",
                "export fn seed() -> integer\n\
                 \x20 return 1\n\
                 end\n\
                 fn tag(head: string, tail: string) -> string\n\
                 \x20 return head .. \"|\" .. tail\n\
                 end\n",
            ),
            (
                "main.sau",
                "import seed from lib\n\
                 fn tag(tail: string, head: string) -> string\n\
                 \x20 return head .. \"/\" .. tail\n\
                 end\n\
                 class Main\n\
                 \x20 static fn main()\n\
                 \x20   println(seed())\n\
                 \x20   println(tag(head: \"h\", tail: \"t\"))\n\
                 \x20 end\n\
                 end\n",
            ),
        ],
    );

    // `h/t`, from *main's* `tag`. Lib's would print `h|t`.
    let out = run_capturing(compile(&dir.join("main.sau")));
    assert_eq!(out.trim(), "1\nh/t");
}

#[test]
fn a_named_argument_binds_to_an_imported_fn() {
    // The gap the leak test above turned up on its first run: a class's
    // methods were reachable across a module boundary but a plain exported
    // `fn`'s parameters were not, so `tag(head: …)` on an imported `tag`
    // refused with the same message the imported constructor used to.
    //
    // The alias is the point of the second call: an importer binds the
    // exporter's parameter list under *its own* name for it, so seeding by
    // name at the exporter would bind nothing here.
    let dir = project(
        "named_arg_imported_fn",
        &[
            (
                "lib.sau",
                "export fn tag(head: string, tail: string) -> string\n\
                 \x20 return head .. \"|\" .. tail\n\
                 end\n",
            ),
            (
                "main.sau",
                "import tag from lib\n\
                 import tag as mark from lib\n\
                 class Main\n\
                 \x20 static fn main()\n\
                 \x20   println(tag(tail: \"t\", head: \"h\"))\n\
                 \x20   println(mark(tail: \"y\", head: \"x\"))\n\
                 \x20 end\n\
                 end\n",
            ),
        ],
    );

    let out = run_capturing(compile(&dir.join("main.sau")));
    assert_eq!(out.trim(), "h|t\nx|y");
}
