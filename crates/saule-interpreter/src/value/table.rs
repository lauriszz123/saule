//! Hybrid array + map tables.
//!
//! The static type system decides how a table is *typed* (`table<T>` array or
//! `table<K, V>` map). At runtime there is a single representation so a table
//! passed across these boundaries (e.g. through `any`) never has to be
//! converted. Positive integer keys collapse into the dense `array` part so
//! the common array iteration path stays a `Vec` walk.

use crate::fxhash::FxHashMap as HashMap;
use std::borrow::Borrow;
use std::hash::{Hash, Hasher};

use super::{SauleStr, Value};

/// A hashable key for the map part of a table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableKey {
    Int(i64),
    Str(SauleStr),
    Bool(bool),
}

/// A borrowed view of a [`TableKey`], so a `&str` can be looked up without
/// first being copied into an owned key. See [`TableObject::get_str`].
pub(crate) enum TableKeyRef<'a> {
    Int(i64),
    /// The text **and its hash**, so a probe that already knows the hash —
    /// which is every probe made with a [`SauleStr`] — does not recompute it.
    Str(&'a str, u32),
    Bool(bool),
}

impl TableKey {
    fn as_ref(&self) -> TableKeyRef<'_> {
        match self {
            TableKey::Int(i) => TableKeyRef::Int(*i),
            // `hash32` is cached on the string, so a stored key answers this
            // without touching a byte after the first time.
            TableKey::Str(s) => TableKeyRef::Str(s, s.hash32()),
            TableKey::Bool(b) => TableKeyRef::Bool(*b),
        }
    }
}

/// Fold a variant tag and a payload into the hash the map indexes with.
///
/// One function for both halves of the key, which is what keeps the owned
/// and borrowed forms agreeing bit for bit — the property the whole
/// `Borrow<dyn AsTableKeyRef>` arrangement rests on.
fn tag_hash(tag: u8, payload: i64) -> u64 {
    let mut h = crate::fxhash::FxHasher::default();
    h.write_u8(tag);
    h.write_i64(payload);
    h.finish()
}

impl TableKeyRef<'_> {
    fn key_hash(&self) -> u64 {
        match self {
            TableKeyRef::Int(i) => tag_hash(0, *i),
            TableKeyRef::Str(_, h) => tag_hash(1, *h as i64),
            TableKeyRef::Bool(b) => tag_hash(2, *b as i64),
        }
    }
}

impl PartialEq for TableKeyRef<'_> {
    /// Text, not hash. Two keys that collide must still compare unequal, so
    /// the cached hash is a filter for the map and never the answer.
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (TableKeyRef::Int(a), TableKeyRef::Int(b)) => a == b,
            (TableKeyRef::Str(a, _), TableKeyRef::Str(b, _)) => a == b,
            (TableKeyRef::Bool(a), TableKeyRef::Bool(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for TableKeyRef<'_> {}

// `TableKey` and `TableKeyRef` are used as the owned and borrowed halves of
// the same map key, so their hashes have to agree bit for bit. Writing the
// tag explicitly (rather than deriving) makes that a property of this code
// instead of a property of how the compiler happens to hash enum
// discriminants.
impl Hash for TableKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u64(self.as_ref().key_hash());
    }
}

/// Bridge trait letting `HashMap<TableKey, _>` be probed with a borrowed
/// key, via the standard `Borrow<dyn Trait>` pattern.
pub(crate) trait AsTableKeyRef {
    fn key(&self) -> TableKeyRef<'_>;
}

impl AsTableKeyRef for TableKey {
    fn key(&self) -> TableKeyRef<'_> {
        self.as_ref()
    }
}

impl AsTableKeyRef for TableKeyRef<'_> {
    fn key(&self) -> TableKeyRef<'_> {
        match self {
            TableKeyRef::Int(i) => TableKeyRef::Int(*i),
            TableKeyRef::Str(s, h) => TableKeyRef::Str(s, *h),
            TableKeyRef::Bool(b) => TableKeyRef::Bool(*b),
        }
    }
}

impl Hash for dyn AsTableKeyRef + '_ {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u64(self.key().key_hash());
    }
}

impl PartialEq for dyn AsTableKeyRef + '_ {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}

impl Eq for dyn AsTableKeyRef + '_ {}

impl<'a> Borrow<dyn AsTableKeyRef + 'a> for TableKey {
    fn borrow(&self) -> &(dyn AsTableKeyRef + 'a) {
        self
    }
}

/// **The** iteration order of a table's map part, for both engines.
///
/// `for k, v in t` yields the array part and then the map part *sorted*, so
/// that iterating a table is deterministic and the order is observable. That
/// makes this comparator a language-level guarantee rather than a detail, and
/// it lived in two places until it was found to disagree with itself: the
/// tree-walker compared keys by type and value, while the VM's snapshot
/// sorted on `display()` — so `{10, 2, 3}` iterated `2, 3, 10` under one
/// engine and `10, 2, 3` under the other, integer keys ordering
/// lexicographically on one side and numerically on the other.
///
/// Written out rather than derived. A derived `Ord` would order the variants
/// by however they happen to be declared above, which makes reordering them a
/// silent change to what every `for … in` yields — the same reason `Hash` is
/// written out with explicit tags.
impl Ord for TableKey {
    fn cmp(&self, other: &TableKey) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        match (self, other) {
            (TableKey::Int(x), TableKey::Int(y)) => x.cmp(y),
            (TableKey::Int(_), _) => Ordering::Less,
            (_, TableKey::Int(_)) => Ordering::Greater,
            (TableKey::Str(x), TableKey::Str(y)) => x.cmp(y),
            (TableKey::Str(_), _) => Ordering::Less,
            (_, TableKey::Str(_)) => Ordering::Greater,
            (TableKey::Bool(x), TableKey::Bool(y)) => x.cmp(y),
        }
    }
}

impl PartialOrd for TableKey {
    fn partial_cmp(&self, other: &TableKey) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl TableKey {
    pub fn from_value(v: &Value) -> Option<TableKey> {
        match v {
            Value::Int(i) => Some(TableKey::Int(*i)),
            // Was `(**s).clone()` — a fresh `String` allocation and a
            // memcpy on *every map insert*. Sharing the allocation is the
            // whole point of `SauleStr`.
            Value::Str(s) => Some(TableKey::Str(s.clone())),
            Value::Bool(b) => Some(TableKey::Bool(*b)),
            _ => None,
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            TableKey::Int(i) => Value::Int(*i),
            TableKey::Str(s) => Value::Str(s.clone()),
            TableKey::Bool(b) => Value::Bool(*b),
        }
    }

    pub fn display(&self) -> String {
        match self {
            TableKey::Int(i) => crate::itoa::i64_to_string(*i),
            TableKey::Str(s) => format!("\"{s}\""),
            TableKey::Bool(b) => b.to_string(),
        }
    }
}

#[derive(Debug, Default)]
pub struct TableObject {
    /// 1-based logical indices stored 0-based here.
    pub array: Vec<Value>,
    /// All non-array entries (non-integer keys or sparse integer keys).
    pub map: HashMap<TableKey, Value>,
}

impl TableObject {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_array(items: Vec<Value>) -> Self {
        Self {
            array: items,
            map: HashMap::default(),
        }
    }

    /// Array length (Lua-style `#t`). Does not include map entries.
    pub fn array_len(&self) -> usize {
        self.array.len()
    }

    /// Read by arbitrary value index. Returns `Nil` for missing keys.
    pub fn get(&self, key: &Value) -> Value {
        if let Value::Int(i) = key
            && *i >= 1
            && (*i as usize) <= self.array.len()
        {
            return self.array[(*i as usize) - 1].clone();
        }
        if let Value::Str(s) = key {
            // Straight to the cached hash — `get_str` would take the `&str`
            // path and hash the bytes again.
            return self
                .map
                .get(&TableKeyRef::Str(s, s.hash32()) as &dyn AsTableKeyRef)
                .cloned()
                .unwrap_or(Value::Nil);
        }
        match TableKey::from_value(key) {
            Some(k) => self.map.get(&k).cloned().unwrap_or(Value::Nil),
            None => Value::Nil,
        }
    }

    /// Read a string key without materialising an owned [`TableKey`].
    ///
    /// `t.foo` used to cost three allocations — an `Rc<String>` for the
    /// name, a `String` clone inside `TableKey::from_value`, and the
    /// `TableKey` itself — purely to hash a `&str` we already had. The
    /// borrowed-key lookup does it with none.
    pub fn get_str(&self, key: &str) -> Value {
        self.map
            .get(&TableKeyRef::Str(key, super::hash_str(key)) as &dyn AsTableKeyRef)
            .cloned()
            .unwrap_or(Value::Nil)
    }

    /// Write by arbitrary value index. Positive integers ≤ len+1 grow the
    /// array part; everything else lands in the map.
    pub fn set(&mut self, key: &Value, value: Value) -> Result<(), String> {
        // A container coming to rest inside another container is the only
        // way a cycle can form, so it is the only thing the collector needs
        // to hear about. Everything else — every integer, every string —
        // returns from here immediately. See `crate::gc`.
        crate::gc::on_store(&value);
        if let Value::Int(i) = key
            && *i >= 1
        {
            let slot = (*i as usize) - 1;
            if slot < self.array.len() {
                self.array[slot] = value;
                return Ok(());
            }
            if slot == self.array.len() {
                self.array.push(value);
                // Pull any contiguous map entries into the array.
                //
                // Guarded on a non-empty map, and that guard is the whole
                // point: appending to an array-shaped `table<T>` is the
                // common write, and without it every append hashed
                // `TableKey::Int` and probed a map that could not contain
                // it. `interp`'s stack push, `array`'s fill loop and every
                // `Table.insert` were paying for a lookup that only a table
                // with sparse integer keys can ever answer.
                if !self.map.is_empty() {
                    let mut next = self.array.len() as i64 + 1;
                    while let Some(v) = self.map.remove(&TableKey::Int(next)) {
                        self.array.push(v);
                        next += 1;
                    }
                }
                return Ok(());
            }
        }
        let Some(k) = TableKey::from_value(key) else {
            return Err(format!(
                "table keys must be integer, string, or boolean, got `{}`",
                key.type_name()
            ));
        };
        self.grow_hint();
        self.map.insert(k, value);
        Ok(())
    }

    /// Widen the map's growth step from 2x to 3x once it is big enough for
    /// the difference to be worth paying for.
    ///
    /// `hashbrown` grows by doubling, and every growth rehashes each key
    /// already stored — for `TableKey::Str` that means walking the string's
    /// bytes again, not re-reading a cached hash. Filling a map to `n`
    /// entries therefore rehashes ~`n` keys in total (the geometric sum of
    /// the doublings), which is why `benchmarks/sau/map.sau` spent 14% of
    /// its runtime inside `reserve_rehash`.
    ///
    /// Reserving two further capacities at the growth point makes each step
    /// 3x, so that sum falls to ~`n/2` and half the rehashing disappears —
    /// worth 6-8% of `map` on both engines. What it buys with is slack:
    /// capacity settles in `[n, 3n)` rather than `[n, 2n)`.
    ///
    /// 4x was measured too and is not better: it ran `map` within 0.1% of
    /// 3x for a looser `[n, 4n)` bound, so the extra step buys nothing the
    /// allocator does not already give back. Caching each key's hash inside
    /// `TableKey` — the other way to make rehashing cheap — was worth a
    /// further 2% on top, which does not pay for reshaping a `pub` enum
    /// that also carries the `Ord` iteration-order contract below.
    ///
    /// The `>= SMALL` guard is what keeps that trade honest. Most tables in
    /// a Saule program are record-shaped — a handful of string keys — and
    /// they are numerous, so over-allocating each one would trade a
    /// benchmark win for a memory regression everywhere else. Below the
    /// threshold `hashbrown`'s own doubling stands, and rehashing a few
    /// dozen keys costs nothing worth reclaiming.
    #[inline]
    fn grow_hint(&mut self) {
        /// Chosen so a record-shaped table never reaches it.
        const SMALL: usize = 32;
        let cap = self.map.capacity();
        if cap >= SMALL && self.map.len() == cap {
            self.map.reserve(cap * 2);
        }
    }

    /// Remove a hash-map entry by key. Returns the previous value (or
    /// `nil` if absent). Array slots are left untouched — use this only
    /// for the map portion.
    pub fn remove(&mut self, key: &Value) -> Value {
        let Some(k) = TableKey::from_value(key) else {
            return Value::Nil;
        };
        self.map.remove(&k).unwrap_or(Value::Nil)
    }
}
