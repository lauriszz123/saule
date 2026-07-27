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
use std::rc::Rc;

use super::Value;

/// A hashable key for the map part of a table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableKey {
    Int(i64),
    Str(String),
    Bool(bool),
}

/// A borrowed view of a [`TableKey`], so a `&str` can be looked up without
/// first being copied into an owned key. See [`TableObject::get_str`].
#[derive(PartialEq, Eq)]
pub(crate) enum TableKeyRef<'a> {
    Int(i64),
    Str(&'a str),
    Bool(bool),
}

impl TableKey {
    fn as_ref(&self) -> TableKeyRef<'_> {
        match self {
            TableKey::Int(i) => TableKeyRef::Int(*i),
            TableKey::Str(s) => TableKeyRef::Str(s),
            TableKey::Bool(b) => TableKeyRef::Bool(*b),
        }
    }
}

// `TableKey` and `TableKeyRef` are used as the owned and borrowed halves of
// the same map key, so their hashes have to agree bit for bit. Writing the
// tag explicitly (rather than deriving) makes that a property of this code
// instead of a property of how the compiler happens to hash enum
// discriminants.
impl Hash for TableKeyRef<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            TableKeyRef::Int(i) => {
                state.write_u8(0);
                i.hash(state);
            }
            TableKeyRef::Str(s) => {
                state.write_u8(1);
                s.hash(state);
            }
            TableKeyRef::Bool(b) => {
                state.write_u8(2);
                b.hash(state);
            }
        }
    }
}

impl Hash for TableKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_ref().hash(state);
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
            TableKeyRef::Str(s) => TableKeyRef::Str(s),
            TableKeyRef::Bool(b) => TableKeyRef::Bool(*b),
        }
    }
}

impl Hash for dyn AsTableKeyRef + '_ {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key().hash(state);
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

impl TableKey {
    pub fn from_value(v: &Value) -> Option<TableKey> {
        match v {
            Value::Int(i) => Some(TableKey::Int(*i)),
            Value::Str(s) => Some(TableKey::Str((**s).clone())),
            Value::Bool(b) => Some(TableKey::Bool(*b)),
            _ => None,
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            TableKey::Int(i) => Value::Int(*i),
            TableKey::Str(s) => Value::Str(Rc::new(s.clone())),
            TableKey::Bool(b) => Value::Bool(*b),
        }
    }

    pub fn display(&self) -> String {
        match self {
            TableKey::Int(i) => i.to_string(),
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
        if let Value::Int(i) = key {
            if *i >= 1 && (*i as usize) <= self.array.len() {
                return self.array[(*i as usize) - 1].clone();
            }
        }
        if let Value::Str(s) = key {
            return self.get_str(s);
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
            .get(&TableKeyRef::Str(key) as &dyn AsTableKeyRef)
            .cloned()
            .unwrap_or(Value::Nil)
    }

    /// Write by arbitrary value index. Positive integers ≤ len+1 grow the
    /// array part; everything else lands in the map.
    pub fn set(&mut self, key: &Value, value: Value) -> Result<(), String> {
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
                let mut next = self.array.len() as i64 + 1;
                while let Some(v) = self.map.remove(&TableKey::Int(next)) {
                    self.array.push(v);
                    next += 1;
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
        self.map.insert(k, value);
        Ok(())
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
