//! `saule build --shared`: a program without the runtime in it.
//!
//! The output is the program alone behind a first line naming the saule
//! that runs it, so that many programs on one machine share one runtime.
//! `saule run FILE` is what that line asks for, and what follows the file
//! is the program's own, whatever it looks like.

use std::path::{Path, PathBuf};
use std::process::Command;

const PROGRAM: &str = "class Main\n  static fn main()\n    println(String.join(\"|\", Os.args()))\n    println(Project.name)\n    local file = Io.open(Project.root .. \"/assets/note.txt\", IoMode.Read)\n    println(file!.read(\"a\"))\n    file!.close()\n  end\nend\n";

/// A project with one bundled file, in a folder of its own under `name`.
fn project(name: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&root);
    let files = [
        (
            "saule.config",
            "name: \"shared-probe\"\nversion: \"0.1.0\"\nentry: \"src/main.sau\"\nsrc_dirs: [\"src\"]\n",
        ),
        ("src/main.sau", PROGRAM),
        ("assets/note.txt", "kept"),
    ];
    for (rel, body) in files {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("create a directory");
        std::fs::write(&path, body).expect("write a fixture");
    }
    root
}

fn saule() -> Command {
    Command::new(env!("CARGO_BIN_EXE_saule"))
}

/// Build `root` with `options` into `out`.
fn build(root: &Path, out: &Path, options: &[&str]) -> std::process::Output {
    saule()
        .arg("build")
        .arg(root)
        .arg("-o")
        .arg(out)
        .args(options)
        .output()
        .expect("run saule build")
}

fn said(out: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn a_shared_program_is_small_and_runs_on_the_saule_it_names() {
    let root = project("shared_build");
    let out = root.join("build/probe");
    let built = build(&root, &out, &["--shared"]);
    assert!(built.status.success(), "{}", said(&built));

    // The program and its one file, not seven megabytes of runtime.
    let bytes = std::fs::read(&out).expect("the output");
    assert!(bytes.len() < 64 * 1024, "{} bytes", bytes.len());
    let first = String::from_utf8_lossy(&bytes[..bytes.iter().position(|&b| b == b'\n').unwrap()])
        .into_owned();
    let runtime = Path::new(env!("CARGO_BIN_EXE_saule"))
        .canonicalize()
        .expect("this saule");
    assert_eq!(first, format!("#!{} run", runtime.display()));

    // Run by name, as the first line is read on any system: arguments
    // that look like saule's own are the program's.
    let cache = root.join("cache");
    let ran = saule()
        .arg("run")
        .arg(&out)
        .args(["one", "--help", "-o", "two words", "--"])
        .env("XDG_CACHE_HOME", &cache)
        .env("LOCALAPPDATA", &cache)
        .env("HOME", &cache)
        .output()
        .expect("run it");
    assert!(ran.status.success(), "{}", said(&ran));
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout).replace('\r', ""),
        "one|--help|-o|two words|--\nshared-probe\nkept\n"
    );

    // And as a program in its own right, where the system reads that line.
    #[cfg(unix)]
    {
        let ran = Command::new(&out)
            .args(["-v"])
            .env("XDG_CACHE_HOME", &cache)
            .env("HOME", &cache)
            .output()
            .expect("run it directly");
        assert!(ran.status.success(), "{}", said(&ran));
        assert_eq!(
            String::from_utf8_lossy(&ran.stdout),
            "-v\nshared-probe\nkept\n"
        );
    }
}

#[test]
fn the_saule_a_shared_program_names_is_a_whole_path_without_spaces() {
    let root = project("shared_build_paths");
    let out = root.join("build/probe");

    let named = build(&root, &out, &["--shared=/opt/saule/bin/saule"]);
    assert!(named.status.success(), "{}", said(&named));
    let bytes = std::fs::read(&out).expect("the output");
    assert!(bytes.starts_with(b"#!/opt/saule/bin/saule run\n"));

    for (bad, why) in [
        ("--shared=bin/saule", "not a whole path"),
        ("--shared=/my saule/saule", "space"),
    ] {
        let refused = build(&root, &root.join("build/refused"), &[bad]);
        assert!(!refused.status.success(), "{bad} was taken");
        assert!(said(&refused).contains(why), "{}", said(&refused));
        assert!(!root.join("build/refused").exists());
    }
}

#[test]
fn a_file_that_is_no_built_program_is_run_as_before() {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("shared_build_plain");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("a folder");
    let file = root.join("plain.sau");
    std::fs::write(&file, "println(String.join(\"|\", Os.args()))\n").expect("a file");
    let ran = saule()
        .arg("run")
        .arg(&file)
        .args(["a", "b"])
        .output()
        .expect("run it");
    assert!(ran.status.success(), "{}", said(&ran));
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout).replace('\r', ""),
        "a|b\n"
    );
}
