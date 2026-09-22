//! The runtime string, and the hash it remembers.
//!
//! Saule strings used to be a bare `Rc<String>`, which meant every table
//! operation re-walked their bytes. `TableObject::get_str` hashed on every
//! lookup, `set` hashed on every insert, and `reserve_rehash` hashed *every
//! key again* on every growth — `benchmarks/sau/map.sau` spent a third of
//! its time between those three.
//!
//! Lua does not do this: a short string is interned and carries its hash in
//! the header, which is why `map` and `wordfreq` were close to begin with.
//! [`SauleStr`] is the same idea — the hash is computed at most once per
//! string and read from then on.
//!
//! **Lazily**, not eagerly. `benchmarks/sau/strings.sau` builds two hundred
//! thousand strings and uses none of them as a table key, so hashing at
//! construction would be a tax on every concatenation to speed up the
//! programs that index with one. The first hash pays; the rest read.
//!
//! **`Deref<Target = String>` on purpose.** This type replaced `Rc<String>`
//! across a hundred-odd call sites, and derefing to `String` rather than
//! `str` is what let almost all of them stay as they were — `**s` is still
//! a `str`, `(**s).clone()` is still a `String`, `s.len()` still works.

use std::cell::Cell;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use crate::fxhash::FxHasher;

/// A string and the hash it has computed so far.
#[derive(Debug)]
struct StrBox {
    /// The cached hash, or `UNHASHED` if it has not been asked for yet.
    hash: Cell<u32>,
    s: String,
}

/// The "not computed yet" marker.
///
/// A real hash that lands on this value is nudged to `UNHASHED + 1` by
/// [`SauleStr::hash32`], which costs one collision's worth of precision in
/// two billion and saves carrying an `Option`.
const UNHASHED: u32 = 0;

/// A Saule runtime string: shared by reference, hashed at most once.
#[derive(Debug, Clone)]
pub struct SauleStr(Rc<StrBox>);

impl SauleStr {
    pub fn new(s: String) -> SauleStr {
        SauleStr(Rc::new(StrBox { hash: Cell::new(UNHASHED), s }))
    }

    pub fn as_str(&self) -> &str {
        &self.0.s
    }

    /// Do these two name the same allocation?
    ///
    /// The replacement for `Rc::ptr_eq`, and the first thing string equality
    /// tries: two strings that came from the same literal *are* the same
    /// allocation, so the common comparison in a scanner — a character
    /// against a constant — answers in one instruction.
    pub fn ptr_eq(a: &SauleStr, b: &SauleStr) -> bool {
        Rc::ptr_eq(&a.0, &b.0)
    }

    /// This string's hash, computing it on the first call only.
    ///
    /// The value must agree with [`hash_str`] for the same bytes, because a
    /// table is probed both ways: with a `SauleStr` that has one of these,
    /// and with a bare `&str` that does not.
    pub fn hash32(&self) -> u32 {
        let cached = self.0.hash.get();
        if cached != UNHASHED {
            return cached;
        }
        let h = match hash_str(&self.0.s) {
            UNHASHED => UNHASHED + 1,
            h => h,
        };
        self.0.hash.set(h);
        h
    }
}

/// The hash of `s`, for callers holding bytes rather than a [`SauleStr`].
///
/// Sharing one function is what keeps the two probe paths agreeing; see
/// [`SauleStr::hash32`].
pub fn hash_str(s: &str) -> u32 {
    let mut h = FxHasher::default();
    s.hash(&mut h);
    // Folded rather than truncated: `FxHasher`'s low bits are the ones
    // `hashbrown` indexes with, and the high half is otherwise discarded.
    let full = h.finish();
    (full ^ (full >> 32)) as u32
}

impl std::ops::Deref for SauleStr {
    type Target = String;
    #[inline(always)]
    fn deref(&self) -> &String {
        &self.0.s
    }
}

impl PartialEq for SauleStr {
    fn eq(&self, other: &SauleStr) -> bool {
        // Pointer, then hash, then bytes. The first answers for anything
        // that came from one literal; the second rejects most of the rest
        // without touching a byte.
        Rc::ptr_eq(&self.0, &other.0)
            || (self.0.s.len() == other.0.s.len()
                && self.hash32() == other.hash32()
                && self.0.s == other.0.s)
    }
}

impl Eq for SauleStr {}

// Comparisons against plain text, so a `SauleStr` field reads like the
// `String` it replaced at the sites that test it against a literal or an
// AST-held name.
impl PartialEq<str> for SauleStr {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}
impl PartialEq<&str> for SauleStr {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}
impl PartialEq<String> for SauleStr {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == other.as_str()
    }
}
impl PartialEq<SauleStr> for str {
    fn eq(&self, other: &SauleStr) -> bool {
        self == other.as_str()
    }
}
impl PartialEq<SauleStr> for String {
    fn eq(&self, other: &SauleStr) -> bool {
        self.as_str() == other.as_str()
    }
}

impl PartialOrd for SauleStr {
    fn partial_cmp(&self, other: &SauleStr) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SauleStr {
    /// Byte order, **not** hash order: this is what `Table.sort` and the
    /// table's own iteration order are built on, so it has to be the order
    /// a reader would predict.
    fn cmp(&self, other: &SauleStr) -> std::cmp::Ordering {
        self.0.s.cmp(&other.0.s)
    }
}

impl From<String> for SauleStr {
    fn from(s: String) -> SauleStr {
        SauleStr::new(s)
    }
}

impl From<&str> for SauleStr {
    fn from(s: &str) -> SauleStr {
        SauleStr::new(s.to_owned())
    }
}

impl std::fmt::Display for SauleStr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0.s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caches_and_agrees_with_the_borrowed_hash() {
        for text in ["", "a", "key1", "key123456", &"x".repeat(500)] {
            let s = SauleStr::from(text);
            // Uncomputed until asked.
            assert_eq!(s.0.hash.get(), UNHASHED, "for {text:?}");
            let first = s.hash32();
            assert_ne!(first, UNHASHED, "a computed hash is never the marker");
            // Cached, and stable across calls.
            assert_eq!(s.hash32(), first, "for {text:?}");
            // And the same answer a bare `&str` gets — the property the
            // table's two probe paths rest on.
            let bare = match hash_str(text) {
                UNHASHED => UNHASHED + 1,
                h => h,
            };
            assert_eq!(first, bare, "for {text:?}");
        }
    }

    #[test]
    fn equality_ignores_provenance() {
        let a = SauleStr::from("hello");
        let b = SauleStr::from("hello");
        let c = a.clone();
        assert!(!SauleStr::ptr_eq(&a, &b), "built separately");
        assert!(SauleStr::ptr_eq(&a, &c), "cloned shares the allocation");
        // All three compare equal regardless.
        assert_eq!(a, b);
        assert_eq!(a, c);
        assert_ne!(a, SauleStr::from("hell"));
        assert_ne!(a, SauleStr::from("hellp"));
    }

    #[test]
    fn equality_holds_when_only_one_side_is_hashed() {
        let a = SauleStr::from("same");
        let b = SauleStr::from("same");
        a.hash32(); // one side warm, the other cold
        assert_eq!(a, b);
        assert_eq!(b, a);
    }

    #[test]
    fn orders_by_bytes() {
        let mut v: Vec<SauleStr> = ["pear", "apple", "fig"].iter().map(|s| SauleStr::from(*s)).collect();
        v.sort();
        let got: Vec<&str> = v.iter().map(|s| s.as_str()).collect();
        assert_eq!(got, ["apple", "fig", "pear"]);
    }

    #[test]
    fn derefs_like_the_rc_string_it_replaced() {
        let s = SauleStr::from("abc");
        assert_eq!(s.len(), 3);
        assert_eq!(&**s, "abc");
        // Call sites hold a `&SauleStr`, exactly as they held a
        // `&Rc<String>` — so `(**r).clone()` is still a `String`.
        let r: &SauleStr = &s;
        let owned: String = (**r).clone();
        assert_eq!(owned, "abc");
    }
}
