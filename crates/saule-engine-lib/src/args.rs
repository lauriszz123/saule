//! Ergonomic, panic-free reading of the `CValue` argument array passed to
//! every exported native function.
//!
//! The interpreter hands us a raw `(*const CValue, usize)` pair. [`Args`]
//! wraps it in a bounds-checked slice and exposes typed accessors that
//! produce a `Result<_, String>` so a wrong arity or wrong argument type
//! becomes a clean error returned to Saule rather than a crash.

use saule_native_abi::{tag, CValue};

/// Borrowed, bounds-checked view over the argument array.
pub struct Args<'a> {
    slice: &'a [CValue],
}

impl<'a> Args<'a> {
    /// # Safety
    /// `ptr` must point to `len` initialised `CValue`s that stay valid for
    /// `'a` — guaranteed by the interpreter for the duration of the call.
    pub unsafe fn new(ptr: *const CValue, len: usize) -> Self {
        let slice = if ptr.is_null() || len == 0 {
            &[][..]
        } else {
            // SAFETY: caller guarantees `ptr` is valid for `len` `CValue`s.
            unsafe { std::slice::from_raw_parts(ptr, len) }
        };
        Self { slice }
    }

    /// Number of arguments received.
    pub fn len(&self) -> usize {
        self.slice.len()
    }

    /// Fail unless exactly `n` arguments were passed.
    pub fn expect_arity(&self, fn_name: &str, n: usize) -> Result<(), String> {
        if self.slice.len() != n {
            return Err(format!(
                "{fn_name} expects {n} argument{}, got {}",
                if n == 1 { "" } else { "s" },
                self.slice.len()
            ));
        }
        Ok(())
    }

    fn at(&self, i: usize) -> Result<&CValue, String> {
        self.slice
            .get(i)
            .ok_or_else(|| format!("missing argument #{}", i + 1))
    }

    /// Read argument `i` as an `integer`.
    pub fn integer(&self, i: usize) -> Result<i64, String> {
        let v = self.at(i)?;
        if v.tag == tag::INT {
            Ok(v.integer)
        } else {
            Err(format!("argument #{} must be an integer", i + 1))
        }
    }

    /// Read argument `i` as a `float`.
    pub fn float(&self, i: usize) -> Result<f64, String> {
        let v = self.at(i)?;
        if v.tag == tag::FLOAT {
            Ok(v.float)
        } else {
            Err(format!("argument #{} must be a float", i + 1))
        }
    }

    /// Read argument `i` as a `string` (copied into an owned `String`).
    pub fn string(&self, i: usize) -> Result<String, String> {
        let v = self.at(i)?;
        // SAFETY: argument string bytes are valid for the call duration.
        match unsafe { v.as_str() } {
            Some(s) => Ok(s.to_string()),
            None => Err(format!("argument #{} must be a string", i + 1)),
        }
    }
}
