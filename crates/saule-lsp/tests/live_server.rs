//! The real `saule-lsp` binary, driven over stdio the way an editor drives it.
//!
//! The other suites call the analysis directly, which cannot see the failures
//! this one exists for: a file changing underneath the server, an importer
//! left showing yesterday's errors, a request answered from a stale buffer.
//! Those live in the handlers and the invalidation between them.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tower_lsp::lsp_types::Url;

const CREATED: i32 = 1;
const CHANGED: i32 = 2;
const DELETED: i32 = 3;

struct Server {
    child: Child,
    stdin: ChildStdin,
    incoming: Receiver<Value>,
    next_id: i64,
    versions: HashMap<String, i64>,
    diagnostics: HashMap<String, Vec<Value>>,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Built by the URL crate rather than by hand: a Windows path is not a URI
/// with `file://` in front of it, and `canonicalize` hands back `\\?\C:\…`.
fn uri(path: &Path) -> String {
    Url::from_file_path(path)
        .expect("test paths are absolute")
        .to_string()
}

impl Server {
    fn start(root: Option<&Path>) -> Server {
        let mut child = Command::new(env!("CARGO_BIN_EXE_saule-lsp"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start saule-lsp");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, incoming) = channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut length = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let line = line.trim();
                    if line.is_empty() {
                        break;
                    }
                    if let Some(n) = line.strip_prefix("Content-Length:") {
                        length = n.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; length];
                if reader.read_exact(&mut body).is_err() {
                    return;
                }
                if tx.send(serde_json::from_slice(&body).unwrap()).is_err() {
                    return;
                }
            }
        });
        let mut server = Server {
            child,
            stdin,
            incoming,
            next_id: 1,
            versions: HashMap::new(),
            diagnostics: HashMap::new(),
        };
        let root_uri = root.map(uri);
        server.request(
            "initialize",
            json!({
                "processId": null,
                "rootUri": root_uri,
                "capabilities": {
                    "workspace": {"didChangeWatchedFiles": {"dynamicRegistration": true}},
                },
            }),
        );
        server.notify("initialized", json!({}));
        server.settle();
        server
    }

    fn send(&mut self, message: &Value) {
        let body = message.to_string();
        write!(self.stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        self.stdin.flush().unwrap();
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    /// Take in one message, answering anything the server asks of us.
    fn handle(&mut self, message: Value) -> Option<Value> {
        if message.get("method").is_some() && message.get("id").is_some() {
            let id = message["id"].clone();
            self.send(&json!({"jsonrpc": "2.0", "id": id, "result": null}));
            return None;
        }
        if message["method"] == "textDocument/publishDiagnostics" {
            let params = &message["params"];
            self.diagnostics.insert(
                params["uri"].as_str().unwrap().to_string(),
                params["diagnostics"].as_array().unwrap().clone(),
            );
            return None;
        }
        message.get("id").is_some().then_some(message)
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            let Ok(message) = self.incoming.recv_timeout(Duration::from_millis(200)) else {
                continue;
            };
            if let Some(response) = self.handle(message)
                && response["id"] == id
            {
                return response["result"].clone();
            }
        }
        panic!("no response to {method}");
    }

    /// Everything already sent has been handled once this much silence has
    /// passed. Notifications carry no acknowledgement to wait on instead.
    fn settle(&mut self) {
        while let Ok(message) = self.incoming.recv_timeout(Duration::from_millis(400)) {
            self.handle(message);
        }
    }

    fn open(&mut self, path: &Path) {
        let text = std::fs::read_to_string(path).unwrap();
        self.open_text(path, &text);
    }

    fn open_text(&mut self, path: &Path, text: &str) {
        self.versions.insert(uri(path), 1);
        self.notify(
            "textDocument/didOpen",
            json!({"textDocument": {
                "uri": uri(path), "languageId": "saule", "version": 1, "text": text,
            }}),
        );
        self.settle();
    }

    /// [`Self::open`], waiting for this file's answer rather than for silence,
    /// which is the difference between seconds and minutes over a whole corpus.
    fn errors_on_opening(&mut self, path: &Path) -> Vec<String> {
        let text = std::fs::read_to_string(path).unwrap();
        self.diagnostics.remove(&uri(path));
        self.versions.insert(uri(path), 1);
        self.notify(
            "textDocument/didOpen",
            json!({"textDocument": {
                "uri": uri(path), "languageId": "saule", "version": 1, "text": text,
            }}),
        );
        let deadline = Instant::now() + Duration::from_secs(30);
        while !self.diagnostics.contains_key(&uri(path)) {
            assert!(Instant::now() < deadline, "no diagnostics for {path:?}");
            if let Ok(message) = self.incoming.recv_timeout(Duration::from_millis(100)) {
                self.handle(message);
            }
        }
        let errors = self.errors(path);
        self.notify(
            "textDocument/didClose",
            json!({"textDocument": {"uri": uri(path)}}),
        );
        errors
    }

    fn change(&mut self, path: &Path, text: &str) {
        let version = self.versions.entry(uri(path)).or_insert(1);
        *version += 1;
        let version = *version;
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": uri(path), "version": version},
                "contentChanges": [{"text": text}],
            }),
        );
        self.settle();
    }

    fn close(&mut self, path: &Path) {
        self.notify(
            "textDocument/didClose",
            json!({"textDocument": {"uri": uri(path)}}),
        );
        self.settle();
    }

    fn disk_event(&mut self, path: &Path, kind: i32) {
        self.notify(
            "workspace/didChangeWatchedFiles",
            json!({"changes": [{"uri": uri(path), "type": kind}]}),
        );
        self.settle();
    }

    /// What the editor is showing for `path`, as messages.
    fn errors(&self, path: &Path) -> Vec<String> {
        self.diagnostics
            .get(&uri(path))
            .map(|all| {
                all.iter()
                    .map(|d| d["message"].as_str().unwrap().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn hover(&mut self, path: &Path, line: u32, character: u32) -> String {
        let result = self.request(
            "textDocument/hover",
            json!({
                "textDocument": {"uri": uri(path)},
                "position": {"line": line, "character": character},
            }),
        );
        result["contents"]["value"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    fn definition_exists(&mut self, path: &Path, line: u32, character: u32) -> bool {
        let result = self.request(
            "textDocument/definition",
            json!({
                "textDocument": {"uri": uri(path)},
                "position": {"line": line, "character": character},
            }),
        );
        !result.is_null()
    }
}

/// A throwaway project on disk.
struct Project(PathBuf);

impl Project {
    fn new(files: &[(&str, &str)]) -> Project {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "saule-lsp-live-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        std::fs::write(
            dir.join("saule.config"),
            "name: \"t\"\nversion: \"0.1.0\"\nsrc_dirs: [\".\"]\n",
        )
        .unwrap();
        for (name, text) in files {
            std::fs::write(dir.join(name), text).unwrap();
        }
        Project(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn write(&self, name: &str, text: &str) {
        std::fs::write(self.path(name), text).unwrap();
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const LIB_INT: &str = "class Lib\n  static fn value() -> integer\n    return 1\n  end\nend\n";
const LIB_STR: &str = "class Lib\n  static fn value() -> string\n    return \"x\"\n  end\nend\n";
const MAIN: &str = "import Lib from \"lib\"\nlocal n: integer = Lib.value()\nprintln(n)\n";

#[test]
fn an_edit_shows_and_clears_an_error() {
    let project = Project::new(&[("a.sau", "local a: integer = 1\n")]);
    let file = project.path("a.sau");
    let mut server = Server::start(Some(&project.0));
    server.open(&file);
    assert!(server.errors(&file).is_empty());
    server.change(&file, "local a: integer = \"x\"\n");
    assert!(!server.errors(&file).is_empty());
    server.change(&file, "local a: integer = 2\n");
    assert!(server.errors(&file).is_empty());
}

#[test]
fn an_unsaved_edit_reaches_importers_and_closing_takes_it_back() {
    let project = Project::new(&[("lib.sau", LIB_INT), ("main.sau", MAIN)]);
    let (lib, main) = (project.path("lib.sau"), project.path("main.sau"));
    let mut server = Server::start(Some(&project.0));
    server.open(&main);
    server.open(&lib);
    assert!(server.errors(&main).is_empty());

    server.change(&lib, LIB_STR);
    assert!(
        !server.errors(&main).is_empty(),
        "edit should reach importer"
    );

    server.close(&lib);
    assert!(
        server.errors(&main).is_empty(),
        "closing without saving puts the disk's text back"
    );
}

#[test]
fn a_change_on_disk_reaches_importers_open_or_closed() {
    let project = Project::new(&[("lib.sau", LIB_INT), ("main.sau", MAIN)]);
    let (lib, main) = (project.path("lib.sau"), project.path("main.sau"));
    let mut server = Server::start(Some(&project.0));
    assert!(server.errors(&main).is_empty());

    project.write("lib.sau", LIB_STR);
    server.disk_event(&lib, CHANGED);
    assert!(!server.errors(&main).is_empty(), "closed importer");

    server.open(&main);
    project.write("lib.sau", LIB_INT);
    server.disk_event(&lib, CHANGED);
    assert!(server.errors(&main).is_empty(), "open importer");
}

#[test]
fn a_module_closed_after_editing_still_follows_the_disk() {
    let project = Project::new(&[("lib.sau", LIB_INT), ("main.sau", MAIN)]);
    let (lib, main) = (project.path("lib.sau"), project.path("main.sau"));
    let mut server = Server::start(Some(&project.0));
    server.open(&main);
    server.open(&lib);
    server.close(&lib);

    project.write("lib.sau", LIB_STR);
    server.disk_event(&lib, CHANGED);
    assert!(!server.errors(&main).is_empty());
}

#[test]
fn creating_and_deleting_a_module_updates_its_importers() {
    let project = Project::new(&[("main.sau", MAIN)]);
    let (lib, main) = (project.path("lib.sau"), project.path("main.sau"));
    let mut server = Server::start(Some(&project.0));
    server.open(&main);
    assert!(!server.errors(&main).is_empty(), "nothing to import yet");

    project.write("lib.sau", LIB_INT);
    server.disk_event(&lib, CREATED);
    assert!(
        server.errors(&main).is_empty(),
        "{:?}",
        server.errors(&main)
    );

    std::fs::remove_file(&lib).unwrap();
    server.disk_event(&lib, DELETED);
    assert!(!server.errors(&main).is_empty());
    assert!(server.errors(&lib).is_empty(), "nothing left to report on");
}

#[test]
fn a_saule_config_change_rechecks_open_files() {
    let project = Project::new(&[("main.sau", MAIN)]);
    std::fs::create_dir(project.path("vendor")).unwrap();
    project.write("vendor/lib.sau", LIB_INT);
    let main = project.path("main.sau");
    let mut server = Server::start(Some(&project.0));
    server.open(&main);
    assert!(!server.errors(&main).is_empty());

    project.write(
        "saule.config",
        "name: \"t\"\nversion: \"0.1.0\"\nsrc_dirs: [\".\", \"vendor\"]\n",
    );
    server.disk_event(&project.path("saule.config"), CHANGED);
    assert!(
        server.errors(&main).is_empty(),
        "{:?}",
        server.errors(&main)
    );
}

#[test]
fn a_change_reaches_files_that_only_import_it_indirectly() {
    let base = |ret: &str, value: &str| {
        format!("class Base\n  fn value() -> {ret}\n    return {value}\n  end\nend\n")
    };
    let project = Project::new(&[
        ("base.sau", &base("integer", "1")),
        (
            "mid.sau",
            "import Base from \"base\"\nclass Mid\n  static fn make() -> Base\n    return Base()\n  end\nend\n",
        ),
        (
            "top.sau",
            "import Mid from \"mid\"\nimport Base from \"base\"\nlocal a: integer = Mid.make().value()\nprintln(a)\n",
        ),
    ]);
    let (base_file, top) = (project.path("base.sau"), project.path("top.sau"));
    let mut server = Server::start(Some(&project.0));
    server.open(&top);
    server.open(&project.path("mid.sau"));
    server.open(&base_file);
    assert!(server.errors(&top).is_empty(), "{:?}", server.errors(&top));

    server.change(&base_file, &base("string", "\"x\""));
    assert!(!server.errors(&top).is_empty());
}

/// VS Code on Windows writes `file:///c%3A/...`, where the server would write
/// `file:///C:/...`. Any client's spelling has to be the one that gets answered,
/// or an open file is taken for a closed one: analysed from the disk instead of
/// its buffer, with the result published under a URI the editor never opened.
#[test]
fn an_open_importer_is_recognised_however_the_client_spells_its_uri() {
    let project = Project::new(&[("lib.sau", LIB_INT), ("main.sau", MAIN)]);
    let (lib, main) = (project.path("lib.sau"), project.path("main.sau"));
    let spelled = uri(&main).replace("main.sau", "m%61in.sau");
    let mut server = Server::start(Some(&project.0));
    assert!(server.errors(&main).is_empty());

    // Unsaved: the disk has no such line.
    let buffer = format!("{MAIN}local unsaved: integer = \"x\"\n");
    server.notify(
        "textDocument/didOpen",
        json!({"textDocument": {
            "uri": spelled, "languageId": "saule", "version": 1, "text": buffer,
        }}),
    );
    server.settle();
    assert_eq!(
        server.diagnostics[&spelled].len(),
        1,
        "the buffer's own error"
    );

    server.open(&lib);
    server.change(&lib, LIB_STR);
    assert_eq!(
        server.diagnostics[&spelled].len(),
        2,
        "the buffer's error and the import's: {:?}",
        server.diagnostics[&spelled]
    );
    assert!(
        server.errors(&main).is_empty(),
        "nothing published under a spelling the editor never opened: {:?}",
        server.errors(&main)
    );
}

/// Each `+` used to double the work, so a sum of 18 terms never came back and
/// took every other request in the file with it.
#[test]
fn a_long_expression_does_not_hang_the_server() {
    let terms = " + x".repeat(200);
    let source = format!(
        "local x: integer = 1\nlocal sum = x{terms}\nlocal neg = {}x\nlocal y: integer = \"s\"\n",
        "-".repeat(100)
    );
    let project = Project::new(&[("a.sau", &source)]);
    let file = project.path("a.sau");
    let mut server = Server::start(Some(&project.0));
    server.open(&file);
    assert!(
        !server.errors(&file).is_empty(),
        "the file was still checked"
    );
    assert!(server.hover(&file, 1, 6).contains("sum"));
}

#[test]
fn saving_rereads_the_disk_for_clients_that_cannot_watch_files() {
    let project = Project::new(&[("lib.sau", LIB_INT), ("main.sau", MAIN)]);
    let main = project.path("main.sau");
    let mut server = Server::start(Some(&project.0));
    server.open(&main);

    project.write("lib.sau", LIB_STR);
    server.notify(
        "textDocument/didSave",
        json!({"textDocument": {"uri": uri(&main)}}),
    );
    server.settle();
    assert!(!server.errors(&main).is_empty());
}

#[test]
fn hover_and_navigation_survive_a_syntax_error_elsewhere() {
    let source = "fn add(a: integer) -> integer\n  return a\nend\nlocal total: integer = add(1)\nlocal broken = (\n";
    let project = Project::new(&[("a.sau", source)]);
    let file = project.path("a.sau");
    let mut server = Server::start(Some(&project.0));
    server.open(&file);
    assert!(!server.errors(&file).is_empty());
    assert!(server.hover(&file, 3, 8).contains("total"));
    assert!(server.definition_exists(&file, 3, 24));
}

#[test]
fn members_of_a_generic_class_resolve_through_every_receiver() {
    let source = "class Box<T>\n  value: T\n\n  fn init(value: T)\n    self.value = value\n  end\nend\n\nlocal b: Box<integer> = Box(1)\nlocal maybe: Box<integer>? = nil\nprintln(b.value)\nprintln(maybe?.value)\n";
    let project = Project::new(&[("a.sau", source)]);
    let file = project.path("a.sau");
    let mut server = Server::start(Some(&project.0));
    server.open(&file);
    assert!(
        server.errors(&file).is_empty(),
        "{:?}",
        server.errors(&file)
    );

    assert!(server.hover(&file, 10, 11).contains("value"), "`b.value`");
    assert!(server.definition_exists(&file, 10, 11), "`b.value`");
    assert!(
        server.hover(&file, 11, 16).contains("value"),
        "`maybe?.value`"
    );
    assert!(server.definition_exists(&file, 11, 16), "`maybe?.value`");

    server.change(&file, &format!("{source}b."));
    let completion = server.request(
        "textDocument/completion",
        json!({
            "textDocument": {"uri": uri(&file)},
            "position": {"line": 12, "character": 2},
        }),
    );
    let items = completion
        .get("items")
        .unwrap_or(&completion)
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        items.iter().any(|i| i["label"] == "value"),
        "completion on `b.`: {items:?}"
    );
}

/// The errors `saule run` reports while running rather than before: they
/// depend on values the checker cannot know. Every other fixture in
/// `tests/errors/` fails before anything runs, so the editor has to show it.
const RUNTIME_ONLY: &[&str] = &[
    "exceptions/catch_wrong_type",
    "exceptions/stack_overflow_recursion",
    "exceptions/stack_overflow_reentrant",
    "exceptions/throw_uncaught",
    "nullability/force_unwrap_chained_nil",
    "nullability/force_unwrap_field_nil",
    "nullability/force_unwrap_nil",
    "nullability/force_unwrap_return_nil",
    "stdlib/io_use_after_close",
    "syntax/pow_negative_exponent",
    "tables/table_insert_oob",
];

fn fixtures(tree: &str) -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests")
        .join(tree)
        .canonicalize()
        .unwrap();
    let mut found = Vec::new();
    for area in std::fs::read_dir(&root).unwrap().flatten() {
        if !area.path().is_dir() {
            continue;
        }
        for file in std::fs::read_dir(area.path()).unwrap().flatten() {
            if file.path().extension().is_some_and(|e| e == "sau") {
                found.push(file.path());
            }
        }
    }
    found.sort();
    found
}

#[test]
fn the_editor_flags_everything_that_cannot_compile_and_nothing_that_runs() {
    let mut server = Server::start(None);

    let mut silent = Vec::new();
    let mut stale_exemptions: Vec<&str> = RUNTIME_ONLY.to_vec();
    for path in fixtures("errors") {
        let area = path
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy();
        let stem = path.file_stem().unwrap().to_string_lossy();
        let name = format!("{area}/{stem}");
        let shown = !server.errors_on_opening(&path).is_empty();
        match RUNTIME_ONLY.contains(&name.as_str()) {
            true if shown => panic!("{name} is flagged now: take it out of RUNTIME_ONLY"),
            true => stale_exemptions.retain(|n| *n != name),
            false if !shown => silent.push(name),
            false => {}
        }
    }
    assert!(
        silent.is_empty(),
        "fails before running, but not flagged: {silent:?}"
    );
    assert!(
        stale_exemptions.is_empty(),
        "no such fixture: {stale_exemptions:?}"
    );

    for path in fixtures("run") {
        let errors = server.errors_on_opening(&path);
        assert!(
            errors.is_empty(),
            "{path:?} runs, but is flagged: {errors:?}"
        );
    }
}
