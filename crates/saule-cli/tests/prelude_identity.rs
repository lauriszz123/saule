//! The standard library's enums and classes are the same in every module.
//!
//! `Os.platform() == OsPlatform.Macos` was true in `main.sau` and false in a
//! module of a sub-folder, on the same Mac. The compiler builds a prelude
//! for each module it compiles, and each build made a new `OsPlatform` and
//! a new `FsKind`; what `Os.platform` and `Os.fsInfo` handed out belonged
//! to whichever was made last, and enum variants compare by identity. A
//! window manager's About window, asking in a `service/` module whether it
//! ran on a Mac, never found out. (`is FsInfo` held throughout; it is asked
//! here so that it stays so.)

use std::path::PathBuf;
use std::process::Command;

/// What a module asks, printed one answer a line: whether exactly one
/// `OsPlatform` variant is `==` to the platform, whether a folder's kind is
/// `FsKind.Dir`, and whether `Os.fsInfo` gives an `FsInfo`.
const ASK: &str = r#"
  static fn ask(who: string) -> nil
    local platform = Os.platform()
    local matching = 0
    for variant in {OsPlatform.Linux, OsPlatform.Macos, OsPlatform.Windows, OsPlatform.Other} do
      if platform == variant then
        matching += 1
      end
    end
    local info = Os.fsInfo(".")!
    printf("%s platform %s\n", who, matching == 1)
    printf("%s kind %s\n", who, info.kind == FsKind.Dir)
    printf("%s class %s\n", who, info is FsInfo)
  end
"#;

/// A project asking in `main.sau` and in `service/Probe.sau`, reached
/// through `service/init.sau` as a package of modules is.
fn project() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("prelude_identity");
    let _ = std::fs::remove_dir_all(&root);
    let main = format!(
        "from service import Probe\n\nclass Main\n  static fn main()\n    Main.ask(\"main\")\n    Probe.ask(\"service\")\n  end\n{ASK}end\n"
    );
    let probe = format!("export class Probe\n{ASK}end\n");
    let files = [
        (
            "saule.config",
            "name: \"probe\"\nversion: \"0.1.0\"\nentry: \"src/main.sau\"\nsrc_dirs: [\"src\"]\n"
                .to_string(),
        ),
        ("src/main.sau", main),
        (
            "src/service/init.sau",
            "export from Probe import *\n".to_string(),
        ),
        ("src/service/Probe.sau", probe),
    ];
    for (rel, body) in files {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("create a directory");
        std::fs::write(&path, body).expect("write a fixture");
    }
    root
}

#[test]
fn the_standard_enums_and_classes_are_the_same_in_every_module() {
    let root = project();
    let out = Command::new(env!("CARGO_BIN_EXE_saule"))
        .arg("run")
        .arg(&root)
        .current_dir(&root)
        .output()
        .expect("run saule");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "saule run failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let expected = [
        "main platform true",
        "main kind true",
        "main class true",
        "service platform true",
        "service kind true",
        "service class true",
    ];
    assert_eq!(stdout.lines().collect::<Vec<_>>(), expected);
}
