//! The runtime byte string.
//!
//! [`SauleStr`](crate::value::SauleStr) wraps a Rust `String`, so it is
//! UTF-8 by construction and cannot carry arbitrary bytes. That is the right
//! default — `String.len` counts codepoints and `String.sub` indexes by them,
//! which is what application code wants — but it leaves no way to hold the
//! contents of a PNG, a hash digest or a msgpack frame. `Os.capture` shows
//! the consequence: it runs the bytes through `from_utf8_lossy`, so anything
//! non-textual comes back with U+FFFD where its high bytes were.
//!
//! Lua solves this by having no text type at all: a Lua string is a counted
//! byte array, and `utf8` is a library layered on top. Saule has a
//! typechecker, so it can do better than asking the reader to remember which
//! of their strings are really blobs — `bytes` is a separate type, and
//! nothing converts between the two without saying so.
//!
//! Deliberately **not** a table key. Keying a table by a byte buffer is rare
//! enough that it is not worth a fourth [`TableKey`](crate::value::TableKey)
//! variant on a path this hot; `Bytes.toHex` gives anyone who needs one a
//! string to key by.

use std::cell::Cell;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use crate::fxhash::FxHasher;

/// A byte buffer and the hash it has computed so far.
#[derive(Debug)]
struct BytesBox {
    /// The cached hash, or `UNHASHED` if it has not been asked for yet.
    hash: Cell<u32>,
    b: Vec<u8>,
}

/// The "not computed yet" marker, as in [`crate::value::str`].
const UNHASHED: u32 = 0;

/// A Saule runtime byte string: shared by reference, hashed at most once.
///
/// Immutable, like every other Saule value. `Bytes.sub` and `Bytes.concat`
/// build new buffers rather than editing one in place, so handing the same
/// `bytes` to two places cannot surprise either of them.
#[derive(Debug, Clone)]
pub struct SauleBytes(Rc<BytesBox>);

impl SauleBytes {
    pub fn new(b: Vec<u8>) -> SauleBytes {
        SauleBytes(Rc::new(BytesBox {
            hash: Cell::new(UNHASHED),
            b,
        }))
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.0.b
    }

    pub fn len(&self) -> usize {
        self.0.b.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.b.is_empty()
    }

    /// Do these two name the same allocation?
    pub fn ptr_eq(a: &SauleBytes, b: &SauleBytes) -> bool {
        Rc::ptr_eq(&a.0, &b.0)
    }

    /// This buffer's hash, computing it on the first call only.
    ///
    /// Only equality reads this today, as a fast reject before the memcmp —
    /// which is worth having when the buffers are frame-sized. It agrees
    /// with [`hash_bytes`] so that the two can never diverge if a later
    /// change does want to probe with a bare slice.
    pub fn hash32(&self) -> u32 {
        let cached = self.0.hash.get();
        if cached != UNHASHED {
            return cached;
        }
        let h = match hash_bytes(&self.0.b) {
            UNHASHED => UNHASHED + 1,
            h => h,
        };
        self.0.hash.set(h);
        h
    }

    /// The buffer as text, or `None` when it is not valid UTF-8.
    ///
    /// The nullable return is the whole point of the type: there is no
    /// honest total function from bytes to string, and `from_utf8_lossy`
    /// pretending otherwise is the bug this exists to prevent.
    pub fn to_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.0.b).ok()
    }
}

/// The hash of `b`, for callers holding a slice rather than a [`SauleBytes`].
pub fn hash_bytes(b: &[u8]) -> u32 {
    let mut h = FxHasher::default();
    b.hash(&mut h);
    // Folded rather than truncated, as in `hash_str`: `FxHasher`'s low bits
    // are the ones `hashbrown` indexes with.
    let full = h.finish();
    (full ^ (full >> 32)) as u32
}

impl std::ops::Deref for SauleBytes {
    type Target = [u8];
    #[inline(always)]
    fn deref(&self) -> &[u8] {
        &self.0.b
    }
}

impl PartialEq for SauleBytes {
    fn eq(&self, other: &SauleBytes) -> bool {
        // Pointer, then length, then hash, then bytes — the same ladder
        // `SauleStr` uses, and it matters more here: these buffers are the
        // ones big enough for a memcmp to show up.
        Rc::ptr_eq(&self.0, &other.0)
            || (self.0.b.len() == other.0.b.len()
                && self.hash32() == other.hash32()
                && self.0.b == other.0.b)
    }
}

impl Eq for SauleBytes {}

impl PartialEq<[u8]> for SauleBytes {
    fn eq(&self, other: &[u8]) -> bool {
        self.as_slice() == other
    }
}

impl PartialOrd for SauleBytes {
    fn partial_cmp(&self, other: &SauleBytes) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SauleBytes {
    /// Byte order, so sorting is the order a reader would predict.
    fn cmp(&self, other: &SauleBytes) -> std::cmp::Ordering {
        self.0.b.cmp(&other.0.b)
    }
}

impl From<Vec<u8>> for SauleBytes {
    fn from(b: Vec<u8>) -> SauleBytes {
        SauleBytes::new(b)
    }
}

impl From<&[u8]> for SauleBytes {
    fn from(b: &[u8]) -> SauleBytes {
        SauleBytes::new(b.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_buffers_are_equal_without_sharing_an_allocation() {
        let a = SauleBytes::new(vec![0x80, 0x00, 0xff]);
        let b = SauleBytes::new(vec![0x80, 0x00, 0xff]);

        assert!(!SauleBytes::ptr_eq(&a, &b));
        assert_eq!(a, b);
    }

    #[test]
    fn differing_buffers_are_not_equal() {
        assert_ne!(
            SauleBytes::new(vec![1, 2, 3]),
            SauleBytes::new(vec![1, 2, 4])
        );
        assert_ne!(SauleBytes::new(vec![1, 2]), SauleBytes::new(vec![1, 2, 3]));
    }

    /// The cached hash must not change the answer, so equality has to hold
    /// before and after it is populated.
    #[test]
    fn hashing_is_stable_and_cached() {
        let a = SauleBytes::new(vec![9; 64]);
        let first = a.hash32();

        assert_eq!(first, a.hash32());
        assert_eq!(first, hash_bytes(&[9; 64]));
    }

    /// A buffer holding a NUL and a high byte is exactly what `string`
    /// cannot carry, and the reason this type exists.
    #[test]
    fn arbitrary_bytes_survive_a_round_trip() {
        let raw: Vec<u8> = vec![0x00, 0x7f, 0x80, 0xc3, 0x28, 0xff];
        let b = SauleBytes::new(raw.clone());

        assert_eq!(b.as_slice(), &raw[..]);
        // 0xc3 0x28 is an invalid two-byte sequence, so this is not text.
        assert_eq!(b.to_str(), None);
    }

    #[test]
    fn valid_utf8_converts_back() {
        let b = SauleBytes::new("saulė".as_bytes().to_vec());

        assert_eq!(b.to_str(), Some("saulė"));
        // Six bytes, five codepoints — the distinction `string` erases.
        assert_eq!(b.len(), 6);
    }
}
