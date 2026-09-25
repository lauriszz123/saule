//! Typed conversions between Saule's ABI [`CValue`] and ordinary Rust types.
//!
//! These traits are what let a package author write a *safe* function with
//! normal parameter and return types — the export macros generate the
//! `extern "C"` shim that decodes each argument via [`FromSaule`] and
//! encodes the return via [`IntoSaule`]. The macros derive the Saule type
//! signature from the same Rust types, so the two can never disagree.
//!
//! Supported mappings:
//!
//! | Rust                                   | Saule           |
//! |----------------------------------------|-----------------|
//! | `i8`…`i64`, `u8`…`u64`, `isize`, `usize` | `integer`     |
//! | `f32`, `f64`                           | `float`         |
//! | `bool`                                 | `boolean`       |
//! | `String` (and `&str` as a parameter)   | `string`        |
//! | `Option<T>`                            | `T?`            |
//! | `Vec<T>`                               | `table<T>`      |
//! | `HashMap<K, V>`, `BTreeMap<K, V>`      | `table<K, V>`   |
//! | `()`                                   | `nil`           |
//! | a `#[saule_class]` / `#[saule_enum]`   | that class/enum |
//!
//! Integers narrower than `i64` are range-checked: a Saule `integer` that
//! does not fit an `i32` parameter is an argument error, not a silent wrap.
//! `Vec` and the maps are *copied* across the boundary; take an
//! [`STable`](crate::types::STable) to work on the Saule table in place.

use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;

use saule_native_abi::{CValue, return_error, return_string, tag};

/// Decode an argument from the positional argument slice.
///
/// `func` and `param` name the call site so error messages match what the
/// hand-written packages used to produce (e.g. `Window.create: argument
/// 'width' must be an integer`).
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be a parameter of a Saule export",
    label = "not a type Saule can pass in",
    note = "a `#[saule_class]` is taken as `&{Self}`, `&mut {Self}` or `SObject<{Self}>`; \
            other types need `#[saule_enum]`, or one of the mappings in `saule_sdk::convert`"
)]
pub trait FromSaule: Sized {
    /// Decode the value at `idx`. Required types error when the argument is
    /// missing; [`Option`] treats missing or `nil` as `None`.
    fn from_saule(args: &[CValue], idx: usize, func: &str, param: &str) -> Result<Self, String>;
}

/// Encode a return value into the ABI's single `out` slot.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be returned from a Saule export",
    label = "not a type Saule can receive",
    note = "mark the type `#[saule_class]` or `#[saule_enum]`, or return one of the \
            mappings in `saule_sdk::convert`"
)]
pub trait IntoSaule {
    /// Produce the [`CValue`] written to the caller's `out` pointer.
    fn into_saule(self) -> CValue;
}

pub(crate) fn require<'a>(
    args: &'a [CValue],
    idx: usize,
    func: &str,
    param: &str,
) -> Result<&'a CValue, String> {
    args.get(idx)
        .ok_or_else(|| format!("{func}: missing argument `{param}` (#{})", idx + 1))
}

/// Run an export's body and write its result into `out`: the value and `0`,
/// or an error message and `1`. What every generated shim calls.
///
/// A panic in the author's code is caught here and reported as a Saule
/// runtime error naming the member and where it panicked. Without this it
/// would unwind into an `extern "C"` frame, which aborts the whole
/// interpreter.
#[doc(hidden)]
pub fn run_export(
    out: &mut CValue,
    qualified: &str,
    body: impl FnOnce() -> Result<CValue, String>,
) -> i32 {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)) {
        Ok(Ok(v)) => {
            *out = v;
            0
        }
        Ok(Err(msg)) => {
            *out = return_error(&msg);
            1
        }
        Err(payload) => {
            let what = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "a panic with no message".to_string());
            let at = PANICKED_AT
                .with(|p| p.borrow_mut().take())
                .map(|loc| format!(" at {loc}"))
                .unwrap_or_default();
            *out = return_error(&format!("{qualified} panicked{at}: {what}"));
            1
        }
    }
}

thread_local! {
    /// Where the last panic on this thread happened, recorded by the hook
    /// [`install_panic_hook`] installs and read back by [`run_export`].
    static PANICKED_AT: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}

/// Replace the package's panic hook with one that records where a panic
/// happened instead of printing it: [`run_export`] reports it as a Saule
/// error, with that location, and printing it too is noise in someone
/// else's program.
///
/// Only this package's panics are affected. A `cdylib` carries its own copy
/// of the standard library, hook included, so the interpreter's — and any
/// other package's — is untouched. With `RUST_BACKTRACE` set the default
/// report is printed as well, for the package's author.
pub(crate) fn install_panic_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        let verbose = std::env::var_os("RUST_BACKTRACE").is_some();
        std::panic::set_hook(Box::new(move |info| {
            if let Some(loc) = info.location() {
                let at = format!("{}:{}:{}", loc.file(), loc.line(), loc.column());
                PANICKED_AT.with(|p| *p.borrow_mut() = Some(at));
            }
            if verbose {
                previous(info);
            }
        }));
    });
}

// ─── Integers ───────────────────────────────────────────────────────────────

impl FromSaule for i64 {
    fn from_saule(args: &[CValue], idx: usize, func: &str, param: &str) -> Result<Self, String> {
        let v = require(args, idx, func, param)?;
        if v.tag == tag::INT {
            Ok(v.integer)
        } else {
            Err(format!("{func}: argument `{param}` must be an integer"))
        }
    }
}

impl IntoSaule for i64 {
    fn into_saule(self) -> CValue {
        CValue::integer(self)
    }
}

macro_rules! narrow_integer {
    ($($t:ty),*) => {$(
        impl FromSaule for $t {
            fn from_saule(args: &[CValue], idx: usize, func: &str, param: &str) -> Result<Self, String> {
                let wide = i64::from_saule(args, idx, func, param)?;
                <$t>::try_from(wide).map_err(|_| format!(
                    "{func}: argument `{param}` is {wide}, which does not fit a {}",
                    stringify!($t),
                ))
            }
        }
        impl IntoSaule for $t {
            fn into_saule(self) -> CValue {
                // Saturate rather than wrap for the few values an `i64`
                // cannot hold (`u64::MAX`): a wrong sign is worse than a
                // clamped magnitude.
                CValue::integer(i64::try_from(self).unwrap_or(i64::MAX))
            }
        }
    )*};
}
narrow_integer!(i8, i16, i32, i128, isize, u8, u16, u32, u64, u128, usize);

// ─── Floats, booleans, strings ──────────────────────────────────────────────

impl FromSaule for f64 {
    fn from_saule(args: &[CValue], idx: usize, func: &str, param: &str) -> Result<Self, String> {
        let v = require(args, idx, func, param)?;
        match v.tag {
            tag::FLOAT => Ok(v.float),
            // Accept an integer where a float is expected — the interpreter
            // may pass an int literal for a `float` parameter.
            tag::INT => Ok(v.integer as f64),
            _ => Err(format!("{func}: argument `{param}` must be a float")),
        }
    }
}

impl IntoSaule for f64 {
    fn into_saule(self) -> CValue {
        CValue::float(self)
    }
}

impl FromSaule for f32 {
    fn from_saule(args: &[CValue], idx: usize, func: &str, param: &str) -> Result<Self, String> {
        Ok(f64::from_saule(args, idx, func, param)? as f32)
    }
}

impl IntoSaule for f32 {
    fn into_saule(self) -> CValue {
        CValue::float(f64::from(self))
    }
}

impl FromSaule for bool {
    fn from_saule(args: &[CValue], idx: usize, func: &str, param: &str) -> Result<Self, String> {
        let v = require(args, idx, func, param)?;
        if v.tag == tag::BOOL {
            Ok(v.boolean != 0)
        } else {
            Err(format!("{func}: argument `{param}` must be a boolean"))
        }
    }
}

impl IntoSaule for bool {
    fn into_saule(self) -> CValue {
        CValue::boolean(self)
    }
}

impl FromSaule for String {
    fn from_saule(args: &[CValue], idx: usize, func: &str, param: &str) -> Result<Self, String> {
        let v = require(args, idx, func, param)?;
        // SAFETY: argument string bytes are valid for the call's duration.
        match unsafe { v.as_str() } {
            Some(s) if v.tag == tag::STR => Ok(s.to_string()),
            _ => Err(format!("{func}: argument `{param}` must be a string")),
        }
    }
}

impl IntoSaule for String {
    fn into_saule(self) -> CValue {
        return_string(&self)
    }
}

impl IntoSaule for &str {
    fn into_saule(self) -> CValue {
        return_string(self)
    }
}

// ─── Option and unit ────────────────────────────────────────────────────────

impl<T: FromSaule> FromSaule for Option<T> {
    fn from_saule(args: &[CValue], idx: usize, func: &str, param: &str) -> Result<Self, String> {
        match args.get(idx) {
            None => Ok(None),
            Some(v) if v.tag == tag::NIL => Ok(None),
            Some(_) => Ok(Some(T::from_saule(args, idx, func, param)?)),
        }
    }
}

impl IntoSaule for () {
    fn into_saule(self) -> CValue {
        CValue::nil()
    }
}

/// `Some(v)` encodes as `v`; `None` encodes as `nil` (Saule `T?`).
impl<T: IntoSaule> IntoSaule for Option<T> {
    fn into_saule(self) -> CValue {
        match self {
            Some(v) => v.into_saule(),
            None => CValue::nil(),
        }
    }
}

// ─── Collections, copied through a host table ───────────────────────────────

/// The table handle an argument carries, or an error naming the parameter.
fn table_arg(args: &[CValue], idx: usize, func: &str, param: &str) -> Result<u64, String> {
    let v = require(args, idx, func, param)?;
    match v.as_handle() {
        Some(h) if v.tag == tag::TABLE => Ok(h),
        _ => Err(format!("{func}: argument `{param}` must be a table")),
    }
}

impl<T: FromSaule> FromSaule for Vec<T> {
    fn from_saule(args: &[CValue], idx: usize, func: &str, param: &str) -> Result<Self, String> {
        let t = table_arg(args, idx, func, param)?;
        let n = crate::host::table_len(t).max(0);
        let mut out = Vec::with_capacity(n as usize);
        for i in 1..=n {
            let cell = crate::host::table_get(t, &CValue::integer(i))?;
            // Each element decodes exactly as an argument would, so every
            // mapping above works inside a table too. It is decoded before
            // the next `table_get`, which may reuse the host's string buffer.
            out.push(T::from_saule(&[cell], 0, func, &format!("{param}[{i}]"))?);
        }
        Ok(out)
    }
}

impl<T: IntoSaule> IntoSaule for Vec<T> {
    fn into_saule(self) -> CValue {
        let t = crate::host::table_new();
        for item in self {
            // Pushed as soon as it is encoded: a string element lives in the
            // single-slot return buffer, which the next element overwrites.
            let cv = item.into_saule();
            let _ = crate::host::table_push(t, &cv);
        }
        CValue::table_handle(t)
    }
}

/// Decode every `key → value` pair of a table argument.
fn map_pairs<K: FromSaule, V: FromSaule>(
    args: &[CValue],
    idx: usize,
    func: &str,
    param: &str,
) -> Result<Vec<(K, V)>, String> {
    let t = table_arg(args, idx, func, param)?;
    let keys = crate::host::table_keys(t)?;
    let n = crate::host::table_len(keys).max(0);
    let mut out = Vec::with_capacity(n as usize);
    for i in 1..=n {
        let key_cv = crate::host::table_get(keys, &CValue::integer(i))?;
        let key = K::from_saule(&[key_cv], 0, func, &format!("{param} key"))?;
        // A string key's bytes are in the host's reusable output buffer. That
        // is still fine to pass back: the host copies the key before it
        // writes the value into the same buffer.
        let val_cv = crate::host::table_get(t, &key_cv)?;
        let val = V::from_saule(&[val_cv], 0, func, &format!("{param} value"))?;
        out.push((key, val));
    }
    Ok(out)
}

/// Encode `key → value` pairs into a new host table.
fn map_into<K: IntoSaule, V: IntoSaule>(pairs: impl IntoIterator<Item = (K, V)>) -> CValue {
    let t = crate::host::table_new();
    for (k, v) in pairs {
        // The key is encoded first, and a string key lands in the return
        // buffer that encoding the value would overwrite — so its bytes are
        // copied out before the value is touched.
        let key = k.into_saule();
        let owned_key: Option<Vec<u8>> = (key.tag == tag::STR)
            // SAFETY: a STR `CValue` from `into_saule` points at valid bytes.
            .then(|| unsafe { key.as_str() }.unwrap_or("").as_bytes().to_vec());
        let val = v.into_saule();
        let key = match &owned_key {
            Some(bytes) => CValue::string_borrowed(bytes),
            None => key,
        };
        let _ = crate::host::table_set(t, &key, &val);
    }
    CValue::table_handle(t)
}

impl<K: FromSaule + Eq + Hash, V: FromSaule> FromSaule for HashMap<K, V> {
    fn from_saule(args: &[CValue], idx: usize, func: &str, param: &str) -> Result<Self, String> {
        Ok(map_pairs(args, idx, func, param)?.into_iter().collect())
    }
}

impl<K: IntoSaule, V: IntoSaule> IntoSaule for HashMap<K, V> {
    fn into_saule(self) -> CValue {
        map_into(self)
    }
}

impl<K: FromSaule + Ord, V: FromSaule> FromSaule for BTreeMap<K, V> {
    fn from_saule(args: &[CValue], idx: usize, func: &str, param: &str) -> Result<Self, String> {
        Ok(map_pairs(args, idx, func, param)?.into_iter().collect())
    }
}

impl<K: IntoSaule, V: IntoSaule> IntoSaule for BTreeMap<K, V> {
    fn into_saule(self) -> CValue {
        map_into(self)
    }
}

// ---------------------------------------------------------------------------
// Tuple returns — multi-value returns marshalled as a host array-table.
//
// The single-valued C ABI can't carry several values at once, so a tuple
// return is packed into a freshly allocated host `table` (its array part).
// The interpreter, knowing the method's declared return arity from its
// signature, spreads that table back into multiple Saule values at the call
// site (`local a, b = native_call()`).
//
// Each element is converted and pushed *before* the next is converted, so the
// thread-local string buffer used by `return_string` is copied into the table
// by `table_push` before a later string element can overwrite it.
// ---------------------------------------------------------------------------
macro_rules! impl_into_saule_tuple {
    ($($name:ident),+) => {
        impl<$($name: IntoSaule),+> IntoSaule for ($($name,)+) {
            fn into_saule(self) -> CValue {
                #[allow(non_snake_case)]
                let ($($name,)+) = self;
                let handle = crate::host::table_new();
                $(
                    let cv = $name.into_saule();
                    let _ = crate::host::table_push(handle, &cv);
                )+
                CValue::table_handle(handle)
            }
        }
    };
}

impl_into_saule_tuple!(A, B);
impl_into_saule_tuple!(A, B, C);
impl_into_saule_tuple!(A, B, C, D);
impl_into_saule_tuple!(A, B, C, D, E);
impl_into_saule_tuple!(A, B, C, D, E, F);
