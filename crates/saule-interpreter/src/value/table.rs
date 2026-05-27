//! Hybrid array + map tables.
//!
//! The static type system decides how a table is *typed* (`table<T>` array or
//! `table<K, V>` map). At runtime there is a single representation so a table
//! passed across these boundaries (e.g. through `any`) never has to be
//! converted. Positive integer keys collapse into the dense `array` part so
//! the common array iteration path stays a `Vec` walk.

use std::collections::HashMap;
use std::rc::Rc;

use super::Value;

/// A hashable key for the map part of a table.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TableKey {
    Int(i64),
    Str(String),
    Bool(bool),
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
            map: HashMap::new(),
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
        match TableKey::from_value(key) {
            Some(k) => self.map.get(&k).cloned().unwrap_or(Value::Nil),
            None => Value::Nil,
        }
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
}
