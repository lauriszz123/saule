//! Compiled programs as bytes: what `saule build` ships instead of source
//! (`VM_DESIGN.md` §14).
//!
//! [`encode`] writes a whole [`Program`] — every module's chunk and the
//! class, enum and interface tables they share — and [`decode`] reads it
//! back into a program [`run_program`](crate::run_program) can run, in a
//! process that never saw the source.
//!
//! ## What makes this possible
//!
//! A chunk stores indices, not runtime objects (see [`crate::chunk`]), so
//! nearly everything in it is plain data. The exceptions:
//!
//! - **Folded constants** — `print`, `Math`, `Graphics.circle` are live
//!   runtime objects in the constant pool. They are written as a path to the
//!   value ([`origin`]) and looked up again when read.
//! - **Inline caches** and `cast_fast` are derived, and rebuilt on load.
//! - **Source text** is optional ([`Source`]). Without it, a runtime error
//!   still names the right file, line and column: the reader rebuilds a
//!   blank stand-in of the same shape, which is all the line arithmetic
//!   needs.
//!
//! ## Trust
//!
//! The bytes may have been tampered with or written by another version, so
//! the header pins the exact Saule build, and every decoded chunk is run
//! through the verifier the compiler uses on its own output — a malformed
//! chunk is refused rather than run.
//!
//! ## Layout
//!
//! ```text
//! "SAULEBC\0"  u32 format  str saule-version  u8 source-kind
//! classes[]  enums[]  interfaces[]            shared by every module
//! modules[]                                   post-order, as compiled
//! entry  via[]
//! ```
//!
//! Integers are LEB128 varints except `i64`/`f64` constants (8 bytes,
//! little-endian); a string is a length and UTF-8 bytes; a list is a count
//! and its items; maps are written sorted by key, so one program always
//! produces the same bytes.

pub mod origin;
mod read;
mod write;

pub use read::decode;
pub use write::{EncodeError, encode};

/// The first eight bytes of every bytecode file.
pub const MAGIC: &[u8; 8] = b"SAULEBC\0";

/// Bumped whenever the layout changes. The Saule version is checked too, so
/// this guards a development build against its own older files.
pub const FORMAT: u32 = 1;

/// How much of the source to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The full text: errors show the offending line, as under `saule run`.
    Full,
    /// Only the length of each line: errors name file, line and column, and
    /// the program's source is not in the file.
    Lines,
}

impl Source {
    fn tag(self) -> u8 {
        match self {
            Source::Full => 0,
            Source::Lines => 1,
        }
    }

    fn from_tag(tag: u8) -> Option<Source> {
        match tag {
            0 => Some(Source::Full),
            1 => Some(Source::Lines),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
