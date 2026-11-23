//! One connection: whole messages, with handles alongside.
//!
//! Every platform frames the same way — an 8-byte header gives the payload
//! length and how many handles belong to the message — and the limits below
//! are the same everywhere. How the handles travel is the platform's: on
//! Unix as `SCM_RIGHTS` beside the bytes, on Windows duplicated into the
//! peer and named in the frame (see `sys`).
//!
//! Nothing here ever blocks. A message that does not fit in the pipe waits in
//! an outgoing queue and goes out as the peer reads; a peer that stops
//! reading altogether is cut off once the queue passes [`MAX_BACKLOG`],
//! rather than growing it forever — a frozen app must not be able to stall
//! the display server, or fill its memory.

pub(crate) use super::sys::Channel;

/// The largest message, payload included. Pixels travel in shared memory, so
/// real messages are small; this only bounds what a broken peer can claim.
pub const MAX_MESSAGE: usize = 16 << 20;

/// The most handles one message may carry.
pub const MAX_FDS: usize = 64;

/// How much may wait unsent for a peer that is not reading before it is
/// disconnected.
pub const MAX_BACKLOG: usize = 64 << 20;
