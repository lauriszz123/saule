//! Saule-typed wrappers — the `S*` "bridge" types.
//!
//! These give native-package authors ergonomic, Saule-flavoured handles to
//! values crossing the ABI, each carrying helper methods so you rarely touch
//! a raw [`CValue`](saule_native_abi::CValue):
//!
//! ## Scalars (cross the ABI by value)
//!
//! | Type        | Wraps  | Helpers (selection)                          |
//! |-------------|--------|----------------------------------------------|
//! | [`SInteger`]| `i64`  | `to_i64`, `to_float`, `hash`, `abs`          |
//! | [`SFloat`]  | `f64`  | `to_f64`, `floor`/`ceil`/`round`, `hash`     |
//! | [`SBool`]   | `bool` | `get`, `toggle`, `hash`                       |
//! | [`SString`] | `String`| `as_str`, `len`, `bytes`, `hash`, `upper`, `split` |
//!
//! ## Reference values (operated on by [`Handle`] via the host)
//!
//! | Type         | Saule kind | Helpers                                   |
//! |--------------|------------|-------------------------------------------|
//! | [`STable`]   | `table`    | `new`, `len`, `get`, `set`, `push`, `remove`, `keys`, `to_vec` |
//! | [`SFunction`]| `function` | `call`                                    |
//!
//! All of these implement [`FromSaule`](crate::convert::FromSaule) and
//! [`IntoSaule`](crate::convert::IntoSaule), so a `#[saule_export]` function
//! can take and return them directly and the macro infers the Saule type.

use std::collections::hash_map::DefaultHasher;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;

use saule_native_abi::{CValue, Handle, tag};

use crate::convert::{FromSaule, IntoSaule, require};
use crate::host;

/// Hash an already-hashable value with the standard hasher.
fn hash_of<T: Hash>(v: &T) -> u64 {
    let mut h = DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

// ---------------------------------------------------------------------------
// Scalars
// ---------------------------------------------------------------------------

/// A Saule `integer`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SInteger(pub i64);

impl SInteger {
    /// Wrap a raw `i64`.
    pub fn new(i: i64) -> Self {
        Self(i)
    }
    /// The underlying `i64`.
    pub fn to_i64(self) -> i64 {
        self.0
    }
    /// Widen to a [`SFloat`].
    pub fn to_float(self) -> SFloat {
        SFloat(self.0 as f64)
    }
    /// A stable 64-bit hash of the value.
    pub fn hash(self) -> u64 {
        hash_of(&self.0)
    }
    /// Absolute value (saturating at `i64::MAX` for `i64::MIN`).
    pub fn abs(self) -> Self {
        Self(self.0.saturating_abs())
    }
}

/// A Saule `float`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SFloat(pub f64);

impl SFloat {
    /// Wrap a raw `f64`.
    pub fn new(f: f64) -> Self {
        Self(f)
    }
    /// The underlying `f64`.
    pub fn to_f64(self) -> f64 {
        self.0
    }
    /// Largest integer `<= self`.
    pub fn floor(self) -> SInteger {
        SInteger(self.0.floor() as i64)
    }
    /// Smallest integer `>= self`.
    pub fn ceil(self) -> SInteger {
        SInteger(self.0.ceil() as i64)
    }
    /// Nearest integer (ties away from zero).
    pub fn round(self) -> SInteger {
        SInteger(self.0.round() as i64)
    }
    /// Whether the value is NaN.
    pub fn is_nan(self) -> bool {
        self.0.is_nan()
    }
    /// A stable 64-bit hash of the bit pattern.
    pub fn hash(self) -> u64 {
        hash_of(&self.0.to_bits())
    }
}

/// A Saule `boolean`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SBool(pub bool);

impl SBool {
    /// Wrap a raw `bool`.
    pub fn new(b: bool) -> Self {
        Self(b)
    }
    /// The underlying `bool`.
    pub fn get(self) -> bool {
        self.0
    }
    /// The logical negation.
    pub fn toggle(self) -> Self {
        Self(!self.0)
    }
    /// A stable 64-bit hash (`0` / `1`).
    pub fn hash(self) -> u64 {
        hash_of(&self.0)
    }
}

/// A Saule `string`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SString(pub String);

impl SString {
    /// Wrap an owned `String`.
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    /// Borrow as a `&str`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// Consume into the owned `String`.
    pub fn into_string(self) -> String {
        self.0
    }
    /// Length in bytes.
    pub fn len(&self) -> usize {
        self.0.len()
    }
    /// Whether the string is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    /// The raw UTF-8 bytes.
    pub fn bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
    /// A stable 64-bit hash of the contents.
    pub fn hash(&self) -> u64 {
        hash_of(&self.0)
    }
    /// An upper-cased copy.
    pub fn upper(&self) -> SString {
        SString(self.0.to_uppercase())
    }
    /// A lower-cased copy.
    pub fn lower(&self) -> SString {
        SString(self.0.to_lowercase())
    }
    /// Split on `sep`, returning the pieces as `SString`s.
    pub fn split(&self, sep: &str) -> Vec<SString> {
        if sep.is_empty() {
            return vec![self.clone()];
        }
        self.0.split(sep).map(|s| SString(s.to_string())).collect()
    }
}

// ---------------------------------------------------------------------------
// SValue — an owned, decoded Saule value (used by table iteration / func args)
// ---------------------------------------------------------------------------

/// An owned, decoded Saule value. Produced when reading out of an [`STable`]
/// or calling an [`SFunction`], and accepted (via `From`) wherever a value is
/// written back.
#[derive(Clone, Debug)]
pub enum SValue {
    /// `nil`.
    Nil,
    /// A `boolean`.
    Bool(bool),
    /// An `integer`.
    Int(i64),
    /// A `float`.
    Float(f64),
    /// A `string`.
    Str(String),
    /// A host-owned `table`.
    Table(STable),
    /// A host-owned callable.
    Func(SFunction),
}

impl SValue {
    /// Build a borrowed [`CValue`]. String payloads point into `self`, so the
    /// returned value is only valid while `self` is alive — fine for the
    /// synchronous host callbacks this crate makes.
    fn to_cvalue(&self) -> CValue {
        match self {
            SValue::Nil => CValue::nil(),
            SValue::Bool(b) => CValue::boolean(*b),
            SValue::Int(i) => CValue::integer(*i),
            SValue::Float(f) => CValue::float(*f),
            SValue::Str(s) => CValue::string_borrowed(s.as_bytes()),
            SValue::Table(t) => CValue::table_handle(t.handle),
            SValue::Func(f) => CValue::func_handle(f.handle),
        }
    }

    /// Decode a [`CValue`] into an owned value (copying strings).
    fn from_cvalue(c: &CValue) -> SValue {
        match c.tag {
            tag::BOOL => SValue::Bool(c.boolean != 0),
            tag::INT => SValue::Int(c.integer),
            tag::FLOAT => SValue::Float(c.float),
            // SAFETY: STR tag implies a valid `(ptr, len)` for the call.
            tag::STR => SValue::Str(unsafe { c.as_str() }.unwrap_or("").to_string()),
            tag::TABLE => SValue::Table(STable {
                handle: c.integer as Handle,
                _marker: PhantomData,
            }),
            tag::FUNC => SValue::Func(SFunction {
                handle: c.integer as Handle,
            }),
            _ => SValue::Nil,
        }
    }

    /// `true` if this is `nil`.
    pub fn is_nil(&self) -> bool {
        matches!(self, SValue::Nil)
    }
    /// The `i64` if this is an `integer`.
    pub fn as_int(&self) -> Option<i64> {
        match self {
            SValue::Int(i) => Some(*i),
            _ => None,
        }
    }
    /// The `f64` if this is a `float` (or `integer`, widened).
    pub fn as_float(&self) -> Option<f64> {
        match self {
            SValue::Float(f) => Some(*f),
            SValue::Int(i) => Some(*i as f64),
            _ => None,
        }
    }
    /// The `bool` if this is a `boolean`.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            SValue::Bool(b) => Some(*b),
            _ => None,
        }
    }
    /// The `&str` if this is a `string`.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            SValue::Str(s) => Some(s),
            _ => None,
        }
    }
    /// The [`STable`] if this is a `table`.
    pub fn as_table(&self) -> Option<&STable> {
        match self {
            SValue::Table(t) => Some(t),
            _ => None,
        }
    }
    /// The [`SFunction`] if this is a callable.
    pub fn as_func(&self) -> Option<&SFunction> {
        match self {
            SValue::Func(f) => Some(f),
            _ => None,
        }
    }
}

impl From<i64> for SValue {
    fn from(v: i64) -> Self {
        SValue::Int(v)
    }
}
impl From<f64> for SValue {
    fn from(v: f64) -> Self {
        SValue::Float(v)
    }
}
impl From<bool> for SValue {
    fn from(v: bool) -> Self {
        SValue::Bool(v)
    }
}
impl From<&str> for SValue {
    fn from(v: &str) -> Self {
        SValue::Str(v.to_string())
    }
}
impl From<String> for SValue {
    fn from(v: String) -> Self {
        SValue::Str(v)
    }
}
impl From<SInteger> for SValue {
    fn from(v: SInteger) -> Self {
        SValue::Int(v.0)
    }
}
impl From<SFloat> for SValue {
    fn from(v: SFloat) -> Self {
        SValue::Float(v.0)
    }
}
impl From<SBool> for SValue {
    fn from(v: SBool) -> Self {
        SValue::Bool(v.0)
    }
}
impl From<SString> for SValue {
    fn from(v: SString) -> Self {
        SValue::Str(v.0)
    }
}
impl<T> From<STable<T>> for SValue {
    fn from(v: STable<T>) -> Self {
        SValue::Table(STable {
            handle: v.handle,
            _marker: PhantomData,
        })
    }
}
impl From<SFunction> for SValue {
    fn from(v: SFunction) -> Self {
        SValue::Func(v)
    }
}

// ---------------------------------------------------------------------------
// STable — a host-owned table accessed by handle
// ---------------------------------------------------------------------------

/// A host-owned Saule `table`, manipulated through the [`host`] callbacks.
///
/// The optional type parameter `T` records the *element type* for signature
/// inference only — `STable<SInteger>` renders as `table<integer>` in the
/// manifest (so the Saule type checker rejects a wrongly-typed argument),
/// while a bare `STable` renders as an untyped `table`. `T` does not change
/// runtime behaviour: element access still goes through [`SValue`].
///
/// The handle is valid only for the duration of the native call that produced
/// (or created) it; don't retain one past your export's return.
pub struct STable<T = Untyped> {
    pub(crate) handle: Handle,
    _marker: PhantomData<fn() -> T>,
}

/// Default element marker for an [`STable`] with no declared element type
/// (renders as a bare `table`).
#[derive(Clone, Copy, Debug)]
pub enum Untyped {}

// Manual `Copy` / `Clone` / `Debug` so they don't pick up a spurious `T: …`
// bound from `derive` (the marker is purely phantom).
impl<T> Clone for STable<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for STable<T> {}
impl<T> fmt::Debug for STable<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("STable")
            .field("handle", &self.handle)
            .finish()
    }
}

impl<T> STable<T> {
    /// Allocate a fresh, empty table on the host.
    pub fn new() -> Self {
        Self {
            handle: host::table_new(),
            _marker: PhantomData,
        }
    }

    /// The raw handle (rarely needed directly).
    pub fn handle(&self) -> Handle {
        self.handle
    }

    /// Number of array-part elements.
    pub fn len(&self) -> usize {
        host::table_len(self.handle).max(0) as usize
    }

    /// Whether the array part is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Read `table[key]`.
    pub fn get(&self, key: impl Into<SValue>) -> Result<SValue, String> {
        let key = key.into();
        let out = host::table_get(self.handle, &key.to_cvalue())?;
        Ok(SValue::from_cvalue(&out))
    }

    /// Assign `table[key] = val`.
    pub fn set(&self, key: impl Into<SValue>, val: impl Into<SValue>) -> Result<(), String> {
        let key = key.into();
        let val = val.into();
        host::table_set(self.handle, &key.to_cvalue(), &val.to_cvalue())
    }

    /// Append `val` to the array part.
    pub fn push(&self, val: impl Into<SValue>) -> Result<(), String> {
        let val = val.into();
        host::table_push(self.handle, &val.to_cvalue())
    }

    /// Remove `key` from the table.
    pub fn remove(&self, key: impl Into<SValue>) -> Result<(), String> {
        let key = key.into();
        host::table_remove(self.handle, &key.to_cvalue())
    }

    /// A new array-table holding this table's keys. The element marker of
    /// the result is left free for the caller to choose (Saule tables are
    /// integer-keyed, so a typical annotation is `STable<SInteger>`).
    pub fn keys<U>(&self) -> Result<STable<U>, String> {
        Ok(STable {
            handle: host::table_keys(self.handle)?,
            _marker: PhantomData,
        })
    }

    /// Collect the array part into a `Vec` (1-based indices `1..=len`).
    pub fn to_vec(&self) -> Result<Vec<SValue>, String> {
        let n = self.len();
        let mut out = Vec::with_capacity(n);
        for i in 1..=n as i64 {
            out.push(self.get(i)?);
        }
        Ok(out)
    }
}

impl<T> Default for STable<T> {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Generic type-variable markers + SElem (a value typed as a type parameter)
// ---------------------------------------------------------------------------

/// Type-variable markers for *generic* native signatures.
///
/// Used only as the element argument of [`STable`] / [`SElem`], they render as
/// the Saule type-parameter tokens `T`, `U`, `V`, `W` in the manifest, e.g.
/// `STable<T>` → `table<T>` and `SElem<T>` → `T`. The type checker treats
/// these as type variables: it binds them from the call's actual arguments
/// and substitutes them into later parameters and the return type. So
/// `fn find(t: STable<T>, f: SFunction) -> Option<SElem<T>>` checks as
/// `fn<T>(t: table<T>, f: function) -> T?` and a call on a `table<integer>`
/// yields an `integer?`.
///
/// They carry no data and are never constructed.
#[derive(Clone, Copy, Debug)]
pub enum T {}
/// See [`T`].
#[derive(Clone, Copy, Debug)]
pub enum U {}
/// See [`T`].
#[derive(Clone, Copy, Debug)]
pub enum V {}
/// See [`T`].
#[derive(Clone, Copy, Debug)]
pub enum W {}

/// A value whose Saule type is a generic type parameter (`T`, `U`, …).
///
/// At runtime it simply carries an arbitrary [`SValue`]; in a `#[saule_export]`
/// signature it renders as the marker's token (`SElem<T>` → `T`). Use it as a
/// parameter to constrain an argument to a table's element type
/// (`contains(t: STable<T>, value: SElem<T>)`) or as a return to thread that
/// element type out (`find(...) -> Option<SElem<T>>`).
pub struct SElem<M> {
    value: SValue,
    _marker: PhantomData<fn() -> M>,
}

impl<M> SElem<M> {
    /// Wrap a runtime value.
    pub fn new(value: impl Into<SValue>) -> Self {
        Self {
            value: value.into(),
            _marker: PhantomData,
        }
    }
    /// Borrow the underlying value.
    pub fn value(&self) -> &SValue {
        &self.value
    }
    /// Consume into the underlying value.
    pub fn into_value(self) -> SValue {
        self.value
    }
}

impl<M> fmt::Debug for SElem<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SElem").field(&self.value).finish()
    }
}

impl<M> FromSaule for SElem<M> {
    fn from_saule(args: &[CValue], idx: usize, func: &str, param: &str) -> Result<Self, String> {
        let v = require(args, idx, func, param)?;
        Ok(SElem {
            value: SValue::from_cvalue(v),
            _marker: PhantomData,
        })
    }
}
impl<M> IntoSaule for SElem<M> {
    fn into_saule(self) -> CValue {
        self.value.into_saule()
    }
}

// ---------------------------------------------------------------------------
// SFunction — a host-owned callable accessed by handle
// ---------------------------------------------------------------------------

/// A host-owned Saule callable (function / closure / native), invoked through
/// the [`host`] callbacks. Same call-scoped handle lifetime as [`STable`].
#[derive(Clone, Copy, Debug)]
pub struct SFunction {
    pub(crate) handle: Handle,
}

impl SFunction {
    /// The raw handle (rarely needed directly).
    pub fn handle(&self) -> Handle {
        self.handle
    }

    /// Call with positional arguments, returning the first result.
    pub fn call(&self, args: &[SValue]) -> Result<SValue, String> {
        let cargs: Vec<CValue> = args.iter().map(SValue::to_cvalue).collect();
        let out = host::func_call(self.handle, &cargs)?;
        Ok(SValue::from_cvalue(&out))
    }
}

// ---------------------------------------------------------------------------
// FromSaule / IntoSaule
// ---------------------------------------------------------------------------

impl FromSaule for SInteger {
    fn from_saule(args: &[CValue], idx: usize, func: &str, param: &str) -> Result<Self, String> {
        Ok(SInteger(i64::from_saule(args, idx, func, param)?))
    }
}
impl IntoSaule for SInteger {
    fn into_saule(self) -> CValue {
        CValue::integer(self.0)
    }
}

impl FromSaule for SFloat {
    fn from_saule(args: &[CValue], idx: usize, func: &str, param: &str) -> Result<Self, String> {
        Ok(SFloat(f64::from_saule(args, idx, func, param)?))
    }
}
impl IntoSaule for SFloat {
    fn into_saule(self) -> CValue {
        CValue::float(self.0)
    }
}

impl FromSaule for SBool {
    fn from_saule(args: &[CValue], idx: usize, func: &str, param: &str) -> Result<Self, String> {
        Ok(SBool(bool::from_saule(args, idx, func, param)?))
    }
}
impl IntoSaule for SBool {
    fn into_saule(self) -> CValue {
        CValue::boolean(self.0)
    }
}

impl FromSaule for SString {
    fn from_saule(args: &[CValue], idx: usize, func: &str, param: &str) -> Result<Self, String> {
        Ok(SString(String::from_saule(args, idx, func, param)?))
    }
}
impl IntoSaule for SString {
    fn into_saule(self) -> CValue {
        saule_native_abi::return_string(&self.0)
    }
}

impl<T> FromSaule for STable<T> {
    fn from_saule(args: &[CValue], idx: usize, func: &str, param: &str) -> Result<Self, String> {
        let v = require(args, idx, func, param)?;
        match v.as_handle() {
            Some(h) if v.tag == tag::TABLE => Ok(STable {
                handle: h,
                _marker: PhantomData,
            }),
            _ => Err(format!("{func}: argument `{param}` must be a table")),
        }
    }
}
impl<T> IntoSaule for STable<T> {
    fn into_saule(self) -> CValue {
        CValue::table_handle(self.handle)
    }
}

impl FromSaule for SFunction {
    fn from_saule(args: &[CValue], idx: usize, func: &str, param: &str) -> Result<Self, String> {
        let v = require(args, idx, func, param)?;
        match v.as_handle() {
            Some(h) if v.tag == tag::FUNC => Ok(SFunction { handle: h }),
            _ => Err(format!("{func}: argument `{param}` must be a function")),
        }
    }
}
impl IntoSaule for SFunction {
    fn into_saule(self) -> CValue {
        CValue::func_handle(self.handle)
    }
}

impl FromSaule for SValue {
    fn from_saule(args: &[CValue], idx: usize, func: &str, param: &str) -> Result<Self, String> {
        let v = require(args, idx, func, param)?;
        Ok(SValue::from_cvalue(v))
    }
}
impl IntoSaule for SValue {
    fn into_saule(self) -> CValue {
        match self {
            SValue::Nil => CValue::nil(),
            SValue::Bool(b) => CValue::boolean(b),
            SValue::Int(i) => CValue::integer(i),
            SValue::Float(f) => CValue::float(f),
            // Copy the bytes into the thread-local return buffer so the
            // returned `CValue` does not borrow `self` (which is dropped here).
            SValue::Str(s) => saule_native_abi::return_string(&s),
            SValue::Table(t) => CValue::table_handle(t.handle),
            SValue::Func(f) => CValue::func_handle(f.handle),
        }
    }
}
