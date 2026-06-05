//! Typed conversions between Saule's ABI [`CValue`] and ordinary Rust types.
//!
//! These traits are what let a package author write a *safe* function with
//! normal parameter and return types — the `#[saule_export]` macro generates
//! the `extern "C"` shim that decodes each argument via [`FromSaule`] and
//! encodes the return via [`IntoSaule`]. The macro derives the Saule type
//! signature from the same Rust types, so the manifest can never drift from
//! the implementation.
//!
//! Supported mappings:
//!
//! | Rust            | Saule      |
//! |-----------------|------------|
//! | `i64`           | `integer`  |
//! | `f64`           | `float`    |
//! | `bool`          | `boolean`  |
//! | `String`        | `string`   |
//! | `Option<T>`     | `T?`       |
//! | `()`            | `nil`      |

use saule_native_abi::{return_string, tag, CValue};

/// Decode an argument from the positional argument slice.
///
/// `func` and `param` name the call site so error messages match what the
/// hand-written packages used to produce (e.g. `Window.create: argument
/// 'width' must be an integer`).
pub trait FromSaule: Sized {
    /// Decode the value at `idx`. Required types error when the argument is
    /// missing; [`Option`] treats missing or `nil` as `None`.
    fn from_saule(args: &[CValue], idx: usize, func: &str, param: &str) -> Result<Self, String>;
}

/// Encode a return value into the ABI's single `out` slot.
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

impl FromSaule for String {
    fn from_saule(args: &[CValue], idx: usize, func: &str, param: &str) -> Result<Self, String> {
        let v = require(args, idx, func, param)?;
        // SAFETY: argument string bytes are valid for the call's duration.
        match unsafe { v.as_str() } {
            Some(s) => Ok(s.to_string()),
            None => Err(format!("{func}: argument `{param}` must be a string")),
        }
    }
}

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

impl IntoSaule for i64 {
    fn into_saule(self) -> CValue {
        CValue::integer(self)
    }
}

impl IntoSaule for f64 {
    fn into_saule(self) -> CValue {
        CValue::float(self)
    }
}

impl IntoSaule for bool {
    fn into_saule(self) -> CValue {
        CValue::boolean(self)
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
