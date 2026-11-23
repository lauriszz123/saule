//! `extern interface` between real processes: a server and a client, each a
//! `saule run` of its own, talking through a service in a private services
//! directory.
//!
//! The fixtures in `tests/extern_interface/` are a small display-server
//! shape: `api.sau` is the contract both sides import, `server.sau`
//! implements it, `client.sau` exercises it. The client's output is the
//! assertion — every line is a property: calls with replies and several
//! results, objects passed both ways, one-way callbacks arriving during a
//! call, a call back into the client while it waits, a data class copied
//! across, tables both ways, a remote `throw` caught, and one-way calls kept
//! in order.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/extern_interface")
}

/// A services directory of the test's own, short enough for a socket path.
/// (On Windows it names the space the test's pipes live in.)
fn services_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sx-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn saule(services: &Path, args: &[&str], dir: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_saule"));
    c.args(args)
        .current_dir(dir)
        .env("SAULE_SERVICES_DIR", services)
        .env_remove("RUST_BACKTRACE");
    c
}

/// A server process, and what it has printed so far.
struct Server {
    child: Child,
    stdout: JoinHandle<String>,
}

/// Start `server` and wait until it says it is `ready` — the line it prints
/// once it has published. Waiting on the output rather than on a socket file
/// is what lets this run on Windows, where a service is a pipe.
fn start_server(services: &Path, dir: &Path, file: &str) -> Server {
    let mut child = saule(services, &["run", file], dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start the server");
    let out = child.stdout.take().expect("piped");
    let (ready_tx, ready_rx) = mpsc::channel();
    let stdout = std::thread::spawn(move || {
        let mut all = String::new();
        let mut lines = BufReader::new(out).lines();
        while let Some(Ok(line)) = lines.next() {
            if line.contains("ready") {
                let _ = ready_tx.send(());
            }
            all.push_str(&line);
            all.push('\n');
        }
        all
    });
    if ready_rx.recv_timeout(Duration::from_secs(30)).is_err() {
        let _ = child.kill();
        let mut err = String::new();
        if let Some(mut e) = child.stderr.take() {
            let _ = e.read_to_string(&mut err);
        }
        panic!("the server never became ready:\n{err}");
    }
    Server { child, stdout }
}

fn run(services: &Path, dir: &Path, file: &str) -> Output {
    saule(services, &["run", file], dir).output().expect("run the client")
}

/// Diagnostics with miette's line wrapping undone, for matching on words.
fn flat(out: &Output) -> String {
    text(out)
        .replace('│', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// Wait for the server to exit by itself, killing it if it will not, and
/// return everything it printed.
fn finish(mut server: Server) -> String {
    let start = Instant::now();
    while server.child.try_wait().unwrap().is_none() {
        if start.elapsed() > Duration::from_secs(10) {
            let _ = server.child.kill();
            panic!("the server did not stop");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut out = server.stdout.join().expect("the reader thread");
    if let Some(mut e) = server.child.stderr.take() {
        let _ = e.read_to_string(&mut out);
    }
    out
}

#[test]
fn a_client_and_a_server_call_each_other() {
    let services = services_dir("calls");
    let dir = fixtures();
    let server = start_server(&services, &dir, "server.sau");
    let out = run(&services, &dir, "client.sau");
    let client = text(&out);
    let server = finish(server);
    assert!(out.status.success(), "client failed:\n{client}\nserver:\n{server}");
    assert_eq!(
        client,
        "client: screen 1024x600\n\
         client: got window 1\n\
         client: callbacks ran: configure 640x480, key a/true\n\
         client: echo a,b,c\n\
         client: caught: the server refused\n\
         client: windows 1, pings 55\n\
         client: done\n"
    );
    assert_eq!(
        server,
        "server: ready\n\
         server: the client calls its window Terminal (asked while it waits)\n\
         server: caller pid known: true\n\
         server: window 1 is now titled ~/projects\n\
         server: stopped\n"
    );
}

/// Write a scratch project and hand back its directory.
fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (file, body) in files {
        std::fs::write(dir.join(file), body).unwrap();
    }
    dir
}

const QUIT_API: &str = "export extern interface Quitter\n\
  fn quit() -> string\n\
  fn hello() -> string\n\
  async fn poke()\n\
end\n";

#[test]
fn a_server_that_has_gone_is_a_failure_the_client_can_catch() {
    let dir = project(
        "ext_gone",
        &[
            ("api.sau", QUIT_API),
            (
                "server.sau",
                "from api import *\n\
                 class Q implements Quitter\n\
                 \x20 fn quit() -> string\n\
                 \x20   Looper.stop()\n\
                 \x20   return \"bye\"\n\
                 \x20 end\n\
                 \x20 fn hello() -> string\n\
                 \x20   return \"hi\"\n\
                 \x20 end\n\
                 \x20 fn poke()\n\
                 \x20 end\n\
                 end\n\
                 Quitter.publish(\"test.quitter\", Q())\n\
                 println(\"ready\")\n\
                 Looper.run()\n",
            ),
            (
                "client.sau",
                "from api import *\n\
                 local q = Quitter.connect(\"test.quitter\")\n\
                 println(q.quit())\n\
                 Os.sleep(0.3)\n\
                 -- One-way calls to a process that has gone are dropped.\n\
                 q.poke()\n\
                 try\n\
                 \x20 println(q.hello())\n\
                 catch e: string\n\
                 \x20 println(\"caught: \" .. e)\n\
                 end\n\
                 try\n\
                 \x20 local again = Quitter.connect(\"test.quitter\")\n\
                 catch e: string\n\
                 \x20 println(\"caught: \" .. e)\n\
                 end\n",
            ),
        ],
    );
    let services = services_dir("gone");
    let server = start_server(&services, &dir, "server.sau");
    let out = run(&services, &dir, "client.sau");
    finish(server);
    let client = text(&out);
    assert!(out.status.success(), "{client}");
    assert_eq!(
        client,
        "bye\n\
         caught: the other process has gone away (it ended, crashed, or closed the connection)\n\
         caught: no service `test.quitter` is published\n"
    );
}

#[test]
fn the_checker_explains_what_cannot_cross_a_process_boundary() {
    let dir = project(
        "ext_check",
        &[(
            "main.sau",
            "export enum Mode\n\
             \x20 A\n\
             end\n\
             export interface Local\n\
             \x20 fn x()\n\
             end\n\
             export extern interface Bad\n\
             \x20 fn callback(f: fn() -> nil)\n\
             \x20 async fn answer() -> integer\n\
             \x20 fn defaulted(x: integer = 3)\n\
             \x20 fn mode(m: Mode)\n\
             \x20 fn useLocal(l: Local)\n\
             end\n\
             export extern interface Empty\n\
             end\n",
        )],
    );
    let out = saule(&services_dir("check"), &["check", "main.sau"], &dir)
        .output()
        .unwrap();
    let all = flat(&out);
    assert!(!out.status.success());
    for expected in [
        "parameter `f` of `Bad.callback` cannot be sent to another process: a function",
        "`Bad.answer` is `async`, so it cannot return a value yet",
        "parameter `x` of `Bad.defaulted` cannot have a default",
        "`Mode` is an enum",
        "`Local` is a local interface",
        "`extern interface Empty` declares no methods",
    ] {
        assert!(
            all.contains(expected),
            "missing {expected:?} in:\n{}",
            text(&out)
        );
    }
}

#[test]
fn connect_and_publish_are_typed() {
    let dir = project(
        "ext_typed",
        &[(
            "main.sau",
            "export extern interface Svc\n\
             \x20 fn n() -> integer\n\
             end\n\
             local s = Svc.connect(\"x\")\n\
             local wrong: string = s\n\
             Svc.publish(42, s)\n",
        )],
    );
    let out = saule(&services_dir("typed"), &["check", "main.sau"], &dir)
        .output()
        .unwrap();
    let all = flat(&out);
    assert!(all.contains("cannot assign value of type `Svc` to variable of type `string`"), "{all}");
    assert!(all.contains("argument 1 of `Svc.publish` expects `string`, got `integer`"), "{all}");
}
