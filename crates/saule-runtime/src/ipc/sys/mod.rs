//! The transport's platform layer — the one part that differs by system.
//!
//! Each platform provides the same few pieces, and everything above them
//! (framing limits, the wire format, calls and objects, the language glue)
//! is shared:
//!
//! - [`Handle`]: what carries an open file or shared memory between
//!   processes;
//! - `Channel`: one connection, sending and receiving whole framed messages
//!   with handles, never blocking (the interface `channel` describes);
//! - `Listener`: a published service, accepting connections;
//! - `connect`: reaching a published service;
//! - `Poller`: waiting on every listener and connection at once;
//! - `shm_create`, `shm_map`, `shm_unmap`: shared memory.

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::Handle;
#[cfg(unix)]
pub(crate) use unix::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::Handle;
#[cfg(windows)]
pub(crate) use windows::*;
