//! `saule install` / `remove` / `list`, through the binary a user runs.
//!
//! Every test gets its own `SAULE_HOME`, so nothing here can touch a real
//! install, and fetches are redirected to a git repository created on disk by
//! the test. The redirect is git's own `url.<base>.insteadOf`, passed through
//! `GIT_CONFIG_*` — so the code under test builds and clones the same
//! `https://github.com/...` URL it would in earnest, and nothing about the
//! production path is stubbed or made test-aware.
//!
//! The native-package path is not covered here: it runs `cargo build
//! --release` on a fetched crate, which needs a network fetch of the SDK and
//! minutes of build time. It is exercised by hand against Shine2D.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A scratch directory tree for one test: its own `SAULE_HOME`, a project,
/// and any package repositories it publishes.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Scratch {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join("packages")
            .join(name);
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create the scratch root");
        Scratch { root }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    fn write(&self, rel: &str, body: &str) {
        let path = self.path(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create a parent directory");
        }
        std::fs::write(path, body).expect("write a file");
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.path(rel)).expect("read a file")
    }

    /// A runnable project with `dependencies:` as given.
    fn project(&self, deps: &[&str]) -> PathBuf {
        let list: Vec<String> = deps.iter().map(|d| format!("\"{d}\"")).collect();
        self.write(
            "proj/saule.config",
            &format!(
                "name: \"demo\"\nversion: \"0.1.0\"\nentry: \"src/main.sau\"\n\
                 src_dirs: [\"src\"]\ndependencies: [{}]\n",
                list.join(", ")
            ),
        );
        self.write(
            "proj/src/main.sau",
            "class Main\n  static fn main()\n  end\nend\n",
        );
        self.path("proj")
    }

    /// Give the project a `main.sau` with `imports` at the top — where they
    /// belong — and `body` inside `Main.main`.
    ///
    /// Run through project mode (`saule run` with no target), because that is
    /// the mode that has a project: a single-file run deliberately has none,
    /// so it reads no `dependencies:` and would find no package.
    fn main_is(&self, imports: &str, body: &str) {
        self.write(
            "proj/src/main.sau",
            &format!("{imports}\nclass Main\n  static fn main()\n    {body}\n  end\nend\n"),
        );
    }

    /// Publish `files` as a git repository with a `v1` tag, and return the
    /// `owner/repo` it can be installed as.
    fn publish(&self, repo: &str, files: &[(&str, &str)]) -> String {
        for (rel, body) in files {
            self.write(&format!("repos/{repo}/{rel}"), body);
        }
        let dir = self.path(&format!("repos/{repo}"));
        git(&dir, &["init", "-q"]);
        git(&dir, &["add", "-A"]);
        git(
            &dir,
            &[
                "-c",
                "user.email=test@example.com",
                "-c",
                "user.name=test",
                "commit",
                "-qm",
                "published",
            ],
        );
        git(&dir, &["tag", "v1"]);
        format!("acme/{repo}")
    }

    /// Run `saule <args>` in `dir` with this scratch's `SAULE_HOME`, with
    /// GitHub redirected to the repositories [`publish`](Self::publish) made.
    fn saule(&self, dir: &Path, args: &[&str]) -> Output {
        let repos = self.path("repos");
        Command::new(env!("CARGO_BIN_EXE_saule"))
            .args(args)
            .current_dir(dir)
            .env("SAULE_HOME", self.path("home"))
            // `insteadOf` rewrites the base, so every `acme/<repo>` URL the
            // code builds lands on `repos/<repo>`.
            .env("GIT_CONFIG_COUNT", "1")
            .env(
                "GIT_CONFIG_KEY_0",
                format!("url.{}/.insteadOf", repos.display()),
            )
            .env("GIT_CONFIG_VALUE_0", "https://github.com/acme/")
            .output()
            .expect("run saule")
    }
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// A minimal Saule library package.
fn library(name: &str) -> Vec<(&'static str, String)> {
    vec![
        (
            "saule.config",
            format!("name: \"{name}\"\nkind: \"library\"\nsrc_dirs: [\"src\"]\n"),
        ),
        (
            "src/init.sau",
            format!("export fn who() -> string\n  return \"{name}\"\nend\n"),
        ),
    ]
}

/// Borrow a built file list as [`Scratch::publish`] wants it.
fn as_files<'a>(files: &'a [(&'static str, String)]) -> Vec<(&'static str, &'a str)> {
    files.iter().map(|(a, b)| (*a, b.as_str())).collect()
}

// ─── install ────────────────────────────────────────────────────────────────

#[test]
fn installing_a_source_package_records_it_and_makes_it_importable() {
    let s = Scratch::new("source-install");
    let files = library("uikit");
    let slug = s.publish("uikit", &as_files(&files));
    let proj = s.project(&[]);

    let out = s.saule(&proj, &["install", &format!("gh:{slug}@v1")]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("import * from \"uikit\""),
        "{}",
        stdout(&out)
    );

    // Recorded, so a fresh clone of the project can reinstall it.
    assert!(
        s.read("proj/saule.config").contains("gh:acme/uikit@v1"),
        "{}",
        s.read("proj/saule.config")
    );
    // Installed globally, keyed by ref.
    assert!(
        s.path("home/packages/github.com/acme/uikit/v1/src/init.sau")
            .is_file()
    );

    // And a program can import it.
    s.main_is("import who from \"uikit\"", "println(who())");
    let run = s.saule(&proj, &["run"]);
    assert!(run.status.success(), "{}", stderr(&run));
    assert_eq!(stdout(&run), "uikit\n");
}

/// The prefix is optional where it is unambiguous, because typing it every
/// time is friction with no payoff.
#[test]
fn the_gh_prefix_may_be_left_off() {
    let s = Scratch::new("bare-slug");
    let files = library("uikit");
    let slug = s.publish("uikit", &as_files(&files));
    let proj = s.project(&[]);

    let out = s.saule(&proj, &["install", &format!("{slug}@v1")]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(s.read("proj/saule.config").contains("gh:acme/uikit@v1"));
}

/// The fresh-clone case: a project that records packages installs them all.
#[test]
fn install_with_no_argument_installs_what_the_project_records() {
    let s = Scratch::new("install-all");
    let a = library("alpha");
    let b = library("beta");
    s.publish("alpha", &as_files(&a));
    s.publish("beta", &as_files(&b));
    let proj = s.project(&["gh:acme/alpha@v1", "gh:acme/beta@v1"]);

    let out = s.saule(&proj, &["install"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("2 installed"), "{}", stdout(&out));
    assert!(s.path("home/packages/github.com/acme/alpha/v1").is_dir());
    assert!(s.path("home/packages/github.com/acme/beta/v1").is_dir());

    // Again, and it does the work twice only if it has to.
    let again = s.saule(&proj, &["install"]);
    assert!(
        stdout(&again).contains("0 installed, 2 already present"),
        "{}",
        stdout(&again)
    );
}

/// Installs are global and shared: a second project gets the package without
/// fetching it again, and says so.
#[test]
fn a_second_project_reuses_the_global_install() {
    let s = Scratch::new("shared-install");
    let files = library("uikit");
    s.publish("uikit", &as_files(&files));
    let first = s.project(&[]);
    assert!(
        s.saule(&first, &["install", "gh:acme/uikit@v1"])
            .status
            .success()
    );

    s.write(
        "second/saule.config",
        "name: \"second\"\nkind: \"library\"\ndependencies: [\"gh:acme/uikit@v1\"]\n",
    );
    let out = s.saule(&s.path("second"), &["install"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("already present"), "{}", stdout(&out));
}

/// Relative paths keep working exactly as before, beside a package in the
/// same list. This is the promise that made one list acceptable.
#[test]
fn paths_and_packages_coexist_in_one_list() {
    let s = Scratch::new("paths-and-packages");
    let files = library("uikit");
    s.publish("uikit", &as_files(&files));
    s.write(
        "shared/saule.config",
        "name: \"shared\"\nkind: \"library\"\n",
    );
    s.write(
        "shared/src/init.sau",
        "export fn hello() -> string\n  return \"hi\"\nend\n",
    );
    let proj = s.project(&["../shared"]);
    assert!(
        s.saule(&proj, &["install", "gh:acme/uikit@v1"])
            .status
            .success()
    );

    s.main_is(
        "import who from \"uikit\"\nimport hello from \"shared\"",
        "println(hello(), who())",
    );
    let run = s.saule(&proj, &["run"]);
    assert!(run.status.success(), "{}", stderr(&run));
    assert_eq!(stdout(&run), "hi\tuikit\n");
}

#[test]
fn installing_at_another_ref_replaces_the_entry() {
    let s = Scratch::new("reinstall-ref");
    let files = library("uikit");
    s.publish("uikit", &as_files(&files));
    let dir = s.path("repos/uikit");
    git(&dir, &["tag", "v2"]);
    let proj = s.project(&[]);

    assert!(
        s.saule(&proj, &["install", "gh:acme/uikit@v1"])
            .status
            .success()
    );
    assert!(
        s.saule(&proj, &["install", "gh:acme/uikit@v2"])
            .status
            .success()
    );

    let config = s.read("proj/saule.config");
    assert!(config.contains("gh:acme/uikit@v2"), "{config}");
    assert!(
        !config.contains("@v1"),
        "the old ref should be gone: {config}"
    );
}

// ─── failures ───────────────────────────────────────────────────────────────

#[test]
fn a_repository_that_is_not_a_package_is_refused() {
    let s = Scratch::new("not-a-package");
    s.publish("plain", &[("README.md", "nothing here")]);
    let proj = s.project(&[]);

    let out = s.saule(&proj, &["install", "gh:acme/plain@v1"]);
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(err.contains("not a Saule package"), "{err}");
    assert!(err.contains("Cargo.toml"), "{err}");
    // Nothing recorded, because nothing was installed.
    assert!(!s.read("proj/saule.config").contains("plain"));
}

#[test]
fn a_missing_ref_says_so() {
    let s = Scratch::new("missing-ref");
    let files = library("uikit");
    s.publish("uikit", &as_files(&files));
    let proj = s.project(&[]);

    let out = s.saule(&proj, &["install", "gh:acme/uikit@v9"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("no tag or branch `v9`"),
        "{}",
        stderr(&out)
    );
}

/// A path is used where it lies; there is nothing to fetch. Saying that is
/// more use than a parse error about a missing owner.
#[test]
fn installing_a_path_explains_itself() {
    let s = Scratch::new("install-path");
    let proj = s.project(&[]);
    let out = s.saule(&proj, &["install", "../shared"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("is a path, not a package"),
        "{}",
        stderr(&out)
    );
}

/// A project naming a package nobody installed fails at the import, naming
/// the command that fixes it.
#[test]
fn running_without_installing_first_says_what_to_run() {
    let s = Scratch::new("not-installed");
    let proj = s.project(&["gh:acme/uikit@v1"]);
    s.main_is("import who from \"uikit\"", "println(who())");

    let out = s.saule(&proj, &["run"]);
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(err.contains("not installed"), "{err}");
    assert!(err.contains("saule install"), "{err}");
}

// ─── remove and list ────────────────────────────────────────────────────────

#[test]
fn remove_drops_the_entry_but_keeps_the_installed_copy() {
    let s = Scratch::new("remove");
    let files = library("uikit");
    s.publish("uikit", &as_files(&files));
    let proj = s.project(&[]);
    assert!(
        s.saule(&proj, &["install", "gh:acme/uikit@v1"])
            .status
            .success()
    );

    let out = s.saule(&proj, &["remove", "uikit"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!s.read("proj/saule.config").contains("uikit"));
    // Shared with every other project, so it stays until asked otherwise.
    assert!(s.path("home/packages/github.com/acme/uikit/v1").is_dir());
    assert!(stdout(&out).contains("--purge"), "{}", stdout(&out));
}

#[test]
fn remove_purge_deletes_the_installed_copy() {
    let s = Scratch::new("purge");
    let files = library("uikit");
    s.publish("uikit", &as_files(&files));
    let proj = s.project(&[]);
    assert!(
        s.saule(&proj, &["install", "gh:acme/uikit@v1"])
            .status
            .success()
    );

    let out = s.saule(&proj, &["remove", "uikit", "--purge"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!s.path("home/packages/github.com/acme/uikit/v1").exists());
    assert!(stdout(&s.saule(&proj, &["list", "--global"])).contains("nothing installed"));
}

#[test]
fn removing_something_absent_fails_and_says_where_to_look() {
    let s = Scratch::new("remove-absent");
    let proj = s.project(&[]);
    let out = s.saule(&proj, &["remove", "nope"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("nothing matched"), "{}", stderr(&out));
}

#[test]
fn list_shows_paths_packages_and_what_is_missing() {
    let s = Scratch::new("list");
    let files = library("uikit");
    s.publish("uikit", &as_files(&files));
    s.write(
        "shared/saule.config",
        "name: \"shared\"\nkind: \"library\"\n",
    );
    let proj = s.project(&["../shared", "gh:acme/absent@v1"]);
    assert!(
        s.saule(&proj, &["install", "gh:acme/uikit@v1"])
            .status
            .success()
    );

    let out = s.saule(&proj, &["list"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("path       ../shared"), "{text}");
    assert!(
        text.contains("source     gh:acme/uikit@v1 (import \"uikit\")"),
        "{text}"
    );
    assert!(text.contains("missing    gh:acme/absent@v1"), "{text}");
}

#[test]
fn list_global_shows_every_install_whichever_project_asked() {
    let s = Scratch::new("list-global");
    let a = library("alpha");
    s.publish("alpha", &as_files(&a));
    let proj = s.project(&[]);
    assert!(
        s.saule(&proj, &["install", "gh:acme/alpha@v1"])
            .status
            .success()
    );

    // From a directory that is not a project at all.
    let out = s.saule(&s.root.clone(), &["list", "--global"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("acme/alpha@v1"), "{}", stdout(&out));
}

#[test]
fn list_outside_a_project_says_so() {
    let s = Scratch::new("list-no-project");
    let out = s.saule(&s.root.clone(), &["list"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("not inside a Saule project"),
        "{}",
        stderr(&out)
    );
}
