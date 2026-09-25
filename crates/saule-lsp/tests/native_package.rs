//! The editor's view of a native package, end to end.
//!
//! Starts the real `saule-lsp` over stdio with `saule-native-fixture`
//! installed in a throwaway `SAULE_HOME`, opens a file that uses the
//! package's class, and asks what an editor asks: are there diagnostics,
//! what completes after `c.`, what does hovering show. Everything the server
//! knows about the package it read out of the library file — the
//! signatures, the class shape, the `///` comments.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::Duration;

use serde_json::{Value, json};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate is two levels below the workspace root")
        .to_path_buf()
}

/// Build the fixture package and install its library as the only file in a
/// fresh `SAULE_HOME` named for `test`. Returns the home.
fn fixture_home(test: &str) -> PathBuf {
    // A target directory of its own: this runs while `cargo test` may still
    // hold the workspace's.
    let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("native-fixture-target");
    let status = Command::new(env!("CARGO"))
        .args(["build", "--quiet", "-p", "saule-native-fixture", "--target-dir"])
        .arg(&target)
        .current_dir(workspace_root())
        .status()
        .expect("run cargo to build the fixture package");
    assert!(status.success(), "building saule-native-fixture failed");
    let file = format!(
        "{}saule_fixture{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    );

    let home = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("lsp-native-homes")
        .join(test);
    let _ = std::fs::remove_dir_all(&home);
    let packages = home.join("native_packages");
    std::fs::create_dir_all(&packages).expect("create the packages directory");
    std::fs::copy(target.join("debug").join(&file), packages.join(&file))
        .expect("install the fixture library");
    home
}

/// A running `saule-lsp` and the messages it has sent.
struct Server {
    child: Child,
    stdin: ChildStdin,
    inbox: Receiver<Value>,
    next_id: i64,
}

impl Server {
    fn start(home: &Path) -> Server {
        let mut child = Command::new(env!("CARGO_BIN_EXE_saule-lsp"))
            .env("SAULE_HOME", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start saule-lsp");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");

        // A reader thread, so a server that stops talking fails the test
        // with a timeout instead of hanging it.
        let (tx, inbox) = channel();
        std::thread::spawn(move || {
            let mut r = BufReader::new(stdout);
            loop {
                let mut len = None;
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    if let Some(v) = line.strip_prefix("Content-Length:") {
                        len = v.trim().parse::<usize>().ok();
                    }
                }
                let mut body = vec![0; len.expect("a Content-Length header")];
                if r.read_exact(&mut body).is_err() {
                    return;
                }
                let msg: Value = serde_json::from_slice(&body).expect("a JSON message");
                if tx.send(msg).is_err() {
                    return;
                }
            }
        });

        Server {
            child,
            stdin,
            inbox,
            next_id: 1,
        }
    }

    fn send(&mut self, msg: &Value) {
        let body = serde_json::to_vec(msg).expect("serialise");
        write!(self.stdin, "Content-Length: {}\r\n\r\n", body.len()).expect("write header");
        self.stdin.write_all(&body).expect("write body");
        self.stdin.flush().expect("flush");
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    /// Wait for a message matching `want`, answering any request the server
    /// makes of the client meanwhile so it never stalls waiting on us.
    fn wait_for(&mut self, what: &str, want: impl Fn(&Value) -> bool) -> Value {
        loop {
            let msg = match self.inbox.recv_timeout(Duration::from_secs(30)) {
                Ok(m) => m,
                Err(RecvTimeoutError::Timeout) => panic!("timed out waiting for {what}"),
                Err(RecvTimeoutError::Disconnected) => panic!("the server exited before {what}"),
            };
            if want(&msg) {
                return msg;
            }
            if msg.get("method").is_some()
                && let Some(id) = msg.get("id")
            {
                let id = id.clone();
                self.send(&json!({ "jsonrpc": "2.0", "id": id, "result": null }));
            }
        }
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        let reply = self.wait_for(method, |m| {
            m.get("id") == Some(&json!(id)) && m.get("method").is_none()
        });
        reply.get("result").cloned().unwrap_or(Value::Null)
    }

    fn diagnostics_for(&mut self, uri: &str) -> Vec<Value> {
        let note = self.wait_for("diagnostics", |m| {
            m.get("method") == Some(&json!("textDocument/publishDiagnostics"))
                && m["params"]["uri"] == json!(uri)
        });
        note["params"]["diagnostics"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn file_uri(path: &Path) -> String {
    format!("file://{}", path.display())
}

/// Open `source` as `main.sau` in `dir` with a fresh server, and return the
/// server, the document's URI and its first diagnostics.
fn open(home: &Path, source: &str) -> (Server, String, Vec<Value>) {
    let dir = home.join("project");
    std::fs::create_dir_all(&dir).expect("create the project");
    let path = dir.join("main.sau");
    std::fs::write(&path, source).expect("write the document");
    let uri = file_uri(&path);

    let mut server = Server::start(home);
    server.request(
        "initialize",
        json!({ "processId": null, "rootUri": file_uri(&dir), "capabilities": {} }),
    );
    server.notify("initialized", json!({}));
    server.notify(
        "textDocument/didOpen",
        json!({ "textDocument": {
            "uri": uri, "languageId": "saule", "version": 1, "text": source,
        }}),
    );
    let diags = server.diagnostics_for(&uri);
    (server, uri, diags)
}

/// The zero-based `(line, character)` of the `nth` occurrence of `needle`,
/// plus `offset` characters.
fn position(source: &str, needle: &str, nth: usize, offset: usize) -> Value {
    let at = source
        .match_indices(needle)
        .nth(nth)
        .map(|(i, _)| i + offset)
        .expect("needle in source");
    let line = source[..at].matches('\n').count();
    let character = at - source[..at].rfind('\n').map_or(0, |i| i + 1);
    json!({ "line": line, "character": character })
}

const PROGRAM: &str = "import * from \"fixture\"
local c: Counter = Counter(1)
local n: integer = c.bump()
local r: Rounding = Rounding.Up
";

#[test]
fn a_native_package_is_clean_completes_and_hovers() {
    let home = fixture_home("editor");
    let (mut server, uri, diags) = open(&home, PROGRAM);
    assert!(diags.is_empty(), "a correct program has no diagnostics: {diags:#?}");

    // Hover shows the Rust doc comment compiled into the library.
    let hover = server.request(
        "textDocument/hover",
        json!({
            "textDocument": { "uri": uri },
            "position": position(PROGRAM, "bump", 0, 1),
        }),
    );
    let text = hover.to_string();
    assert!(text.contains("Add the step, and return the new value."), "{text}");

    let hover = server.request(
        "textDocument/hover",
        json!({
            "textDocument": { "uri": uri },
            "position": position(PROGRAM, "Counter", 0, 1),
        }),
    );
    let text = hover.to_string();
    assert!(
        text.contains("A counter that starts somewhere and counts in steps."),
        "{text}"
    );

    // Typing `c.` offers the object's methods and properties — and none of
    // the class's static functions, which an object does not have.
    let typed = format!("{PROGRAM}c.");
    server.notify(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": uri, "version": 2 },
            "contentChanges": [{ "text": typed }],
        }),
    );
    let items = server.request(
        "textDocument/completion",
        json!({
            "textDocument": { "uri": uri },
            "position": position(&typed, "c.", 1, 2),
        }),
    );
    let items = items
        .get("items")
        .or(Some(&items))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let labels: Vec<&str> = items.iter().filter_map(|i| i["label"].as_str()).collect();
    for member in ["bump", "value", "step", "fork", "absorb", "doubled"] {
        assert!(labels.contains(&member), "`c.` should offer `{member}`: {labels:?}");
    }
    for static_fn in ["live", "parse"] {
        assert!(
            !labels.contains(&static_fn),
            "`c.` should not offer the static `{static_fn}`: {labels:?}"
        );
    }
}

#[test]
fn a_misused_native_class_is_flagged_in_the_editor() {
    let home = fixture_home("editor_errors");
    let source = "import * from \"fixture\"\nlocal c = Counter(1)\nc.bump(\"x\")\n";
    let (_server, _uri, diags) = open(&home, source);
    let messages: Vec<&str> = diags.iter().filter_map(|d| d["message"].as_str()).collect();
    assert!(
        messages
            .iter()
            .any(|m| m.contains("`Counter.bump` expects 0 argument(s), got 1")),
        "{messages:?}"
    );
}
