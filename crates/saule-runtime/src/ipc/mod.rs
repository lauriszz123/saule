//! Calls between processes, for `extern interface`.
//!
//! The transport — connections, the wire format, calls and objects, shared
//! memory, on Unix and Windows — is the `saule-ipc` crate, re-exported here
//! whole. What is the runtime's own is [`lang`]: the glue between Saule
//! values and that transport.

pub use saule_ipc::*;

pub mod lang;
