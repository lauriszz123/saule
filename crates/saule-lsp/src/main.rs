//! `saule-lsp` — Language Server Protocol implementation for Saule.
//!
//! Communicates over stdin/stdout (the conventional LSP transport).
//! Editors launch this binary as a child process and speak LSP to it.

mod line_index;
mod server;
mod workspace;

use server::Backend;
use tower_lsp::{LspService, Server};

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    // Seed the stdlib's prelude names and native typeck signatures so
    // references like `print`, `Math.sqrt`, `Iterable`, etc. don't get
    // flagged as undefined. Idempotent; safe to call once at startup.
    saule_interpreter::init();

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
