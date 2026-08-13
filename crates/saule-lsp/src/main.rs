//! `saule-lsp` — Language Server Protocol implementation for Saule.
//!
//! Communicates over stdin/stdout (the conventional LSP transport).
//! Editors launch this binary as a child process and speak LSP to it.

mod exprty;
mod hover;
mod line_index;
mod refs;
mod server;
mod syntax;
mod transport;

use server::Backend;
use tower_lsp::{LspService, Server};

/// Stack for every thread that runs analysis.
///
/// **This is what keeps the server alive on Windows.** Analysis is recursive
/// over a tree that mirrors the user's source — the parser, type checker,
/// hover walker and formatter all descend it — so stack depth is bounded by how
/// deeply the *input* nests, not by anything the server chooses. Windows gives
/// a process's main thread 1 MiB; macOS and Linux give 8. `tower_lsp::Server`
/// polls the service inline in the future it is driving, so on Windows every
/// handler ran on that 1 MiB with tower-lsp's own async state machines already
/// sitting on it.
///
/// The result was a `STATUS_STACK_OVERFLOW` (`0xC00000FD`) partway through the
/// workspace diagnostics pass, on Windows only. The process died mid-frame,
/// which truncated the message being written, and the editor then read a short
/// body — that is what surfaced in IntelliJ as "Error while handling LSP
/// message", because lsp4j turns an empty body into a `null` Message and
/// LSP4IJ's `handleLSPMessage` is `@NotNull`.
///
/// 64 MiB is reserved address space, not committed memory: pages are only
/// backed once touched, so a server that never recurses deeply pays nothing.
const SERVER_STACK_SIZE: usize = 64 * 1024 * 1024;

fn main() {
    // Answer `--version` / `--help` before touching stdin. Without this the
    // server starts and blocks waiting for an LSP handshake that a human at a
    // terminal is never going to send — so `saule-lsp --version`, which both
    // the installer and the docs tell people to run as their check that the
    // language server landed, would hang instead of printing anything.
    if handle_cli_args() {
        return;
    }

    // The main thread's stack size is fixed by the OS at process start and
    // cannot be raised from inside the process, so the only way to get a
    // bigger one is to do the work somewhere else.
    let server = std::thread::Builder::new()
        .name("saule-lsp".to_string())
        .stack_size(SERVER_STACK_SIZE)
        .spawn(serve)
        .expect("could not spawn the language server thread");

    // A panic here must not look like a clean shutdown: exiting 0 tells the
    // editor the server stopped on purpose and it stays stopped.
    if server.join().is_err() {
        std::process::exit(101);
    }
}

/// Serve LSP over stdin/stdout until the editor disconnects.
fn serve() {
    // Seed the stdlib's prelude names and native typeck signatures so
    // references like `print`, `Math.sqrt`, `Iterable`, etc. don't get
    // flagged as undefined. Idempotent; safe to call once at startup.
    saule_interpreter::init();

    // Built by hand rather than via `#[tokio::main]` so that worker and
    // blocking threads inherit the same large stack. Analysis runs inline on
    // whichever thread polls the service, and `tokio::spawn` puts that on a
    // worker — a worker left at the default size would reintroduce the crash
    // somewhere less obvious.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .thread_name("saule-lsp-worker")
        .thread_stack_size(SERVER_STACK_SIZE)
        .enable_all()
        .build()
        .expect("could not start the async runtime");

    runtime.block_on(async {
        // Everything from the editor goes through `transport::sanitize` first.
        // A malformed frame reaching `tower-lsp` ends the serve loop outright,
        // taking the session with it, and that is not catchable from a
        // `LanguageServer` impl. See `transport` for the details.
        let (sanitized, incoming) = tokio::io::duplex(64 * 1024);
        tokio::spawn(async move {
            if let Err(err) = transport::sanitize(tokio::io::stdin(), sanitized).await {
                eprintln!("saule-lsp: stdin transport error: {err}");
            }
            // Dropping `sanitized` here closes the pipe, which is what makes
            // the serve loop below see EOF and return once the editor goes away.
        });

        let stdout = tokio::io::stdout();
        let (service, socket) = LspService::new(Backend::new);
        Server::new(incoming, stdout, socket).serve(service).await;
    });
}

/// Handle the handful of flags a language server has any business accepting.
/// Returns `true` when the process should exit instead of serving.
///
/// Deliberately hand-rolled rather than pulled in via `clap`: the argument
/// surface is a few strings and will stay that way, because everything else a
/// language server is told arrives over the protocol.
fn handle_cli_args() -> bool {
    // Every argument is inspected, not just the first: `--stdio` arrives
    // *after* whatever extra args the editor was configured with, so
    // stopping at `nth(1)` would reject a perfectly ordinary launch.
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-v" | "-V" | "--version" => {
                println!("saule-lsp {}", saule_version::FULL);
                return true;
            }
            "-h" | "--help" => {
                println!(
                    "\
saule-lsp {} — the Saule language server

Speaks LSP over stdin/stdout. Editors launch it; you normally don't.

  -v, --version    print the version and exit
  -h, --help       print this message and exit
      --stdio      serve over stdin/stdout (the default; accepted for clients
                   that pass it explicitly)

Run with no arguments to serve. Editor setup: \
https://lauriszz123.github.io/saule/reference/editors/",
                    saule_version::FULL
                );
                return true;
            }
            // stdio is the only transport we speak, so this is a no-op — but
            // it has to be accepted, because clients pass it unconditionally.
            // `vscode-languageclient` appends `--stdio` to the command line
            // for any `TransportKind.stdio` server; rejecting it killed the
            // process before the `initialize` handshake, which the editor
            // surfaced only as "connection got disposed".
            "--stdio" => {}
            // Anything else is reported rather than ignored: silently serving
            // despite an unrecognised flag is how a typo'd editor config turns
            // into "the language server does nothing and I can't tell why".
            other => {
                eprintln!("saule-lsp: unrecognised argument `{other}` (try --help)");
                std::process::exit(2);
            }
        }
    }
    false
}
