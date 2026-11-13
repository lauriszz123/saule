//! The Saule language server's analysis, as a library.
//!
//! With the default `server` feature this is the whole language server —
//! [`server::Backend`] and the stdio [`transport`] that `main.rs` drives.
//! Without it, it is only the editor-independent engines, which is what lets
//! the web playground (`saule-wasm`) offer the completions an editor gets
//! instead of a second, weaker implementation of its own.

// Without `server` most of the hover / reference machinery has no caller —
// it is still compiled because completion reaches into parts of it.
#![cfg_attr(not(feature = "server"), allow(dead_code, unused_imports))]

mod exprty;
mod hover;
mod line_index;
mod refs;
pub mod server;
mod syntax;
#[cfg(feature = "server")]
pub mod transport;

pub use server::complete;
