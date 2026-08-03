//! `saule-lsp` — Language Server Protocol implementation for Saule.
//!
//! Communicates over stdin/stdout (the conventional LSP transport).
//! Editors launch this binary as a child process and speak LSP to it.

mod exprty;
mod hover;
mod line_index;
mod refs;
mod server;
mod workspace;

use server::Backend;
use tower_lsp::{LspService, Server};

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    // Answer `--version` / `--help` before touching stdin. Without this the
    // server starts and blocks waiting for an LSP handshake that a human at a
    // terminal is never going to send — so `saule-lsp --version`, which both
    // the installer and the docs tell people to run as their check that the
    // language server landed, would hang instead of printing anything.
    if handle_cli_args() {
        return;
    }

    // Seed the stdlib's prelude names and native typeck signatures so
    // references like `print`, `Math.sqrt`, `Iterable`, etc. don't get
    // flagged as undefined. Idempotent; safe to call once at startup.
    saule_interpreter::init();

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}

/// Handle the only two flags a language server has any business accepting.
/// Returns `true` when the process should exit instead of serving.
///
/// Deliberately hand-rolled rather than pulled in via `clap`: the argument
/// surface is two strings and will stay that way, because everything else a
/// language server is told arrives over the protocol.
fn handle_cli_args() -> bool {
    // Only the first argument is ever consulted: every branch below either
    // prints and stops or exits, so a second argument could never be
    // reached. Written as `nth(1)` rather than a loop to say so outright.
    if let Some(arg) = std::env::args().nth(1) {
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

Run with no arguments to serve. Editor setup: \
https://lauriszz123.github.io/saule/reference/editors/",
                    saule_version::FULL
                );
                return true;
            }
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
