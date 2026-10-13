//! `saule disasm`, through the binary a user runs.
//!
//! The behaviour under test is that a module belonging to an import graph is
//! compiled *with* that graph. Before that, `disasm` used the single-module
//! route, and a file whose class extended one declared elsewhere was rejected
//! with `cannot compile a class extending one the compiler cannot see` —
//! which was never a fault in the code, since the same project type-checks
//! clean. `extending_a_class_from_another_module_disassembles` is the test
//! that pins the fix.
//!
//! Each test builds its own project under `CARGO_TARGET_TMPDIR`.

use std::path::{Path, PathBuf};
use std::process::Command;

struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Scratch {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join("disasm")
            .join(name);
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create the scratch root");
        Scratch { root }
    }

    fn write(&self, rel: &str, body: &str) -> PathBuf {
        let path = self.root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create a parent directory");
        }
        std::fs::write(&path, body).expect("write a fixture");
        path
    }

    /// A project whose entry extends a class declared in another module —
    /// the shape the single-module route could not compile.
    fn two_module_project(&self, kind: &str) {
        self.write(
            "saule.config",
            &format!(
                "name: \"probe\"\nversion: \"0.1.0\"\nkind: \"{kind}\"\nsrc_dirs: [\"src\"]\n"
            ),
        );
        self.write(
            "src/base.sau",
            "export class Base\n  fn greet() -> string\n    return \"base\"\n  end\nend\n",
        );
        self.write(
            "src/main.sau",
            "import * from \"base\"\n\n\
             class Derived extends Base\nend\n\n\
             class Main\n  static fn main()\n    \
             local d: Derived = Derived()\n    println(d.greet())\n  end\nend\n",
        );
    }
}

struct Run {
    stdout: String,
    stderr: String,
    status: Option<i32>,
}

impl Run {
    /// The source name on each chunk's first line, in emission order.
    fn chunks(&self) -> Vec<String> {
        self.stdout
            .lines()
            .filter_map(|l| l.strip_prefix("; chunk from "))
            .map(str::to_string)
            .collect()
    }

    /// Chunk labels reduced to their file name, so assertions do not depend
    /// on whether a project context was installed — a file inside a project
    /// is labelled `probe/src/main.sau`, and one outside it by absolute path.
    fn basenames(&self) -> Vec<String> {
        self.chunks()
            .iter()
            .map(|c| {
                c.rsplit(['/', '\\'])
                    .next()
                    .unwrap_or(c.as_str())
                    .to_string()
            })
            .collect()
    }
}

fn disasm(args: &[&str], cwd: Option<&Path>) -> Run {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_saule"));
    cmd.arg("disasm");
    for a in args {
        cmd.arg(a);
    }
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let out = cmd.output().expect("run saule disasm");
    Run {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        status: out.status.code(),
    }
}

/// The regression this change exists for.
#[test]
fn extending_a_class_from_another_module_disassembles() {
    let s = Scratch::new("extends");
    s.two_module_project("app");
    let entry = s.root.join("src/main.sau");

    let run = disasm(&[entry.to_str().unwrap()], None);

    assert_eq!(
        run.status,
        Some(0),
        "stderr: {}\nstdout: {}",
        run.stderr,
        run.stdout
    );
    assert_eq!(run.basenames(), vec!["main.sau"]);
    assert!(
        !run.stderr.contains("cannot compile a class extending"),
        "stderr: {}",
        run.stderr
    );
}

/// By default one chunk — the file asked for — so the output is what it
/// always was for a file that already worked.
#[test]
fn a_single_file_emits_only_its_own_chunk() {
    let s = Scratch::new("single");
    let f = s.write("solo.sau", "local n: integer = 1\nprintln(n)\n");

    let run = disasm(&[f.to_str().unwrap()], None);

    assert_eq!(run.status, Some(0), "stderr: {}", run.stderr);
    assert_eq!(run.basenames(), vec!["solo.sau"]);
}

/// `--deps` adds the imported modules, in post-order: every module after the
/// ones it imports, which is the order they were compiled in.
#[test]
fn deps_adds_the_imported_modules_in_post_order() {
    let s = Scratch::new("deps");
    s.two_module_project("app");
    let entry = s.root.join("src/main.sau");

    let run = disasm(&[entry.to_str().unwrap(), "--deps"], None);

    assert_eq!(run.status, Some(0), "stderr: {}", run.stderr);
    assert_eq!(
        run.basenames(),
        vec!["base.sau", "main.sau"],
        "the imported module must come first"
    );
}

/// Project mode emits every module, and names each one only once even though
/// each source file is used as a root in turn.
#[test]
fn project_mode_emits_every_module_exactly_once() {
    let s = Scratch::new("project");
    s.two_module_project("app");

    let run = disasm(&[s.root.to_str().unwrap()], None);

    assert_eq!(run.status, Some(0), "stderr: {}", run.stderr);
    let names = run.basenames();
    assert_eq!(names.len(), 2, "expected two chunks, got {names:?}");
    assert!(names.contains(&"base.sau".to_string()), "{names:?}");
    assert!(names.contains(&"main.sau".to_string()), "{names:?}");

    // Inside a project the labels are project-relative, which is what makes
    // the output reproducible across machines.
    assert!(
        run.chunks().iter().all(|c| c.starts_with("probe/")),
        "expected project-relative labels: {:?}",
        run.chunks()
    );
}

/// A module nothing imports is disassembled too — the same reason `check`
/// walks every file rather than following imports from the entry.
#[test]
fn project_mode_reaches_a_module_nothing_imports() {
    let s = Scratch::new("orphan");
    s.two_module_project("app");
    s.write(
        "src/orphan.sau",
        "export class Orphan\n  fn value() -> integer\n    return 7\n  end\nend\n",
    );

    let run = disasm(&[s.root.to_str().unwrap()], None);

    assert_eq!(run.status, Some(0), "stderr: {}", run.stderr);
    assert!(
        run.basenames().contains(&"orphan.sau".to_string()),
        "an unimported module must still be disassembled: {:?}",
        run.basenames()
    );
}

/// A library has no entry point at all, so project mode must not depend on
/// one. This is the case the entry-rooted route could not have handled.
#[test]
fn a_library_disassembles_without_an_entry_point() {
    let s = Scratch::new("library");
    s.two_module_project("library");
    // A library's `main.sau` carrying a `Main` class is odd but harmless;
    // what matters is that nothing asked for an entry point.
    let run = disasm(&[s.root.to_str().unwrap()], None);

    assert_eq!(run.status, Some(0), "stderr: {}", run.stderr);
    assert_eq!(run.basenames().len(), 2, "{:?}", run.basenames());
}

/// No argument means the project in the current directory, as with `run` and
/// `check`.
#[test]
fn no_argument_means_the_project_in_the_current_directory() {
    let s = Scratch::new("cwd");
    s.two_module_project("app");

    let run = disasm(&[], Some(&s.root));

    assert_eq!(run.status, Some(0), "stderr: {}", run.stderr);
    assert_eq!(run.basenames().len(), 2, "{:?}", run.basenames());
}

#[test]
fn a_missing_file_is_an_error() {
    let s = Scratch::new("missing");
    let absent = s.root.join("nope.sau");

    let run = disasm(&[absent.to_str().unwrap()], None);

    assert_eq!(run.status, Some(1));
    assert!(run.stderr.contains("does not exist"), "{}", run.stderr);
    assert!(run.stdout.is_empty());
}

/// An unresolvable import is reported at the `import`, and nothing is
/// written to stdout — a half-emitted chunk would be worse than none.
#[test]
fn an_unresolved_import_fails_without_emitting_a_chunk() {
    let s = Scratch::new("bad_import");
    let f = s.write("imp.sau", "import * from \"nowhere\"\n");

    let run = disasm(&[f.to_str().unwrap()], None);

    assert_eq!(run.status, Some(1));
    assert!(run.stdout.is_empty(), "stdout: {}", run.stdout);
    assert!(
        run.stderr.contains("could not find module"),
        "stderr: {}",
        run.stderr
    );
}

/// The listing is the point, so a chunk must carry instructions and not just
/// its header.
#[test]
fn the_listing_contains_instructions() {
    let s = Scratch::new("listing");
    let f = s.write("solo.sau", "local n: integer = 1\nprintln(n)\n");

    let run = disasm(&[f.to_str().unwrap()], None);

    assert!(run.stdout.contains("proto["), "stdout: {}", run.stdout);
    assert!(run.stdout.contains("constants:"), "stdout: {}", run.stdout);
}
