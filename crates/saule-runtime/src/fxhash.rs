//! A fast, non-cryptographic hasher for the interpreter's hot maps.
//!
//! Every scope lookup, instance-field read, and table access goes through a
//! `HashMap`. `std`'s default `RandomState` uses SipHash, which is
//! DoS-resistant but costs roughly 25% of runtime on method-call-heavy
//! programs. The keys in these maps are program identifiers (variable,
//! field, and method names) chosen by the source file, not by an attacker,
//! so the resistance buys nothing and the speed matters.
//!
//! This is the FxHash algorithm used by rustc: multiply-and-rotate over
//! machine words. Inlined here rather than pulled in as a dependency
//! because it's twenty lines.
//!
//! **Note on `TableObject::map`** — that one *can* hold keys derived from
//! program input. Saule tables are not currently exposed to untrusted data
//! in any shipped surface, but if that changes, switch `TableObject::map`
//! back to `RandomState` and keep this hasher for the scope and class maps.

use std::hash::{BuildHasherDefault, Hasher};

pub type FxBuildHasher = BuildHasherDefault<FxHasher>;
pub type FxHashMap<K, V> = std::collections::HashMap<K, V, FxBuildHasher>;

/// Constant from rustc's `FxHasher` — an odd multiplier with good
/// avalanche behaviour over the low bits `hashbrown` indexes with.
const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

#[derive(Default, Clone)]
pub struct FxHasher {
    hash: u64,
}

impl FxHasher {
    #[inline]
    fn add(&mut self, w: u64) {
        self.hash = (self.hash.rotate_left(5) ^ w).wrapping_mul(SEED);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut b = bytes;
        while b.len() >= 8 {
            self.add(u64::from_ne_bytes(b[..8].try_into().unwrap()));
            b = &b[8..];
        }
        if b.len() >= 4 {
            self.add(u32::from_ne_bytes(b[..4].try_into().unwrap()) as u64);
            b = &b[4..];
        }
        for &x in b {
            self.add(x as u64);
        }
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add(i as u64);
    }
    #[inline]
    fn write_i64(&mut self, i: i64) {
        self.add(i as u64);
    }
    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }
}

/// `FxHashMap::default()` with the key/value types left to inference.
///
/// Construction sites that build a map by inserting into a `let mut`
/// binding have nothing for `Default::default()` to infer from, so they
/// call this instead.
pub fn fxmap<K, V>() -> FxHashMap<K, V> {
    FxHashMap::default()
}
