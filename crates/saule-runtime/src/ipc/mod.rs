//! Calls between processes: the transport under `extern interface`.
//!
//! A Saule program can publish an object as a named service and other
//! programs can call its methods as if it were local — the display server
//! is the first user: apps call `createWindow` on it, it calls `configure`
//! and `key` back on them. This module is the machinery under that, in
//! layers:
//!
//! - [`wire`]: the values that can cross, and their encoding;
//! - [`channel`]: one connection — whole messages and file descriptors over
//!   a Unix socket, never blocking;
//! - [`shm`]: shared memory, for pixels and other bulk data;
//! - [`services`]: publishing and finding services by name, and who a peer
//!   is;
//! - [`looper`]: calls and replies, objects by reference, and waiting that
//!   keeps serving.
//!
//! The language layer on top — `extern interface`, proxies, `async fn` —
//! turns Saule values into [`Wire`] and Saule objects into the keys the
//! looper works with.
//!
//! The platform under all of it is [`sys`]: Unix-domain sockets on Linux and
//! macOS, named pipes on Windows. Everything above it is the same everywhere.

mod channel;
mod sys;
pub mod lang;
pub mod looper;
pub mod services;
pub mod shm;
pub mod wire;

pub use looper::{CallError, CallId, ConnId, Handler, Looper, ObjectId, ROOT};
pub use services::{Peer, services_dir, valid_name};
pub use shm::SharedMemory;
pub use sys::Handle;
pub use wire::{ObjRef, Wire};

#[cfg(test)]
mod tests;
