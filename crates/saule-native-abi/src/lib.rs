//! `saule-native-abi` — the stable C-ABI boundary shared between the Saule
//! interpreter and any dynamically-loaded native package (`.so` / `.dll` /
//! `.dylib`).
//!
//! The interpreter never links a native package directly. Instead it loads
//! the package's shared library at runtime (via `libloading`) and calls
//! exported `extern "C"` symbols whose names are declared in a TOML manifest.
//! This crate is the *only* thing both sides agree on, so its layout is
//! frozen: changing [`CValue`] is a breaking ABI change.
//!
//! ## Calling convention
//!
//! Every exported native function has this exact signature:
//!
//! ```ignore
//! #[no_mangle]
//! pub unsafe extern "C" fn saule_engine_graphics_circle(
//!     args: *const CValue,
//!     argc: usize,
//!     out: *mut CValue,
//! ) -> i32 { /* ... */ }
//! ```
//!
//! * `args` / `argc` — the positional Saule arguments, already converted to
//!   [`CValue`]. String payloads point at the caller's buffer and are valid
//!   **only for the duration of the call** — copy out anything you need to
//!   keep.
//! * `out` — write the single return value here (use [`CValue::nil`] for a
//!   `nil`-returning function).
//! * return code — `0` for success. Any non-zero value means failure; write
//!   an error message into `out` with [`CValue::error`] and the interpreter
//!   surfaces it as a runtime error at the call site.
//!
//! ## String ownership
//!
//! Strings cross the boundary as `(ptr, len)` pairs of UTF-8 bytes. The
//! producer must keep the bytes alive until the consumer copies them:
//!
//! * **Arguments** (interpreter → package): valid until the call returns.
//! * **Return / error strings** (package → interpreter): the package stores
//!   them in a thread-local buffer (see [`return_string`]) that stays valid
//!   until that thread's *next* native call. The interpreter copies the
//!   bytes immediately on return, so this contract is sufficient.

use std::cell::RefCell;

/// Discriminant for [`CValue::tag`].
pub mod tag {
    pub const NIL: u8 = 0;
    pub const BOOL: u8 = 1;
    pub const INT: u8 = 2;
    pub const FLOAT: u8 = 3;
    pub const STR: u8 = 4;
    /// Only ever written into the `out` slot alongside a non-zero return
    /// code — carries a UTF-8 error message in the string fields.
    pub const ERR: u8 = 5;
    /// A Saule `table` that stays owned by the host. The package receives
    /// an opaque [`Handle`] (in [`CValue::integer`]) and operates on the
    /// table through the [`HostApi`] callbacks rather than touching its
    /// memory. See the "Reference values" section below.
    pub const TABLE: u8 = 6;
    /// A Saule callable (function / closure / native) owned by the host.
    /// The package holds a [`Handle`] and invokes it via
    /// [`HostApi::func_call`].
    pub const FUNC: u8 = 7;
}

/// An opaque reference to a host-owned value (a `table` or a callable).
///
/// Reference values never cross the ABI by memory — only their `Rc`-counted
/// identity lives on the interpreter side. A handle is a token the host can
/// resolve back to the real [`crate`]-external `Value`. **Handles are valid
/// only for the duration of the native call that produced them** (including
/// any nested host callbacks); the host reclaims them when the outermost
/// call returns. `0` is the never-valid sentinel.
pub type Handle = u64;

/// The symbol an interpreter looks up (optionally) right after loading a
/// package's shared library, to hand the package its [`HostApi`]. A package
/// that uses reference values (`STable` / `SFunction`) must export it; one
/// that only deals in scalars may omit it. `saule-sdk` emits it for you.
pub const SET_HOST_SYMBOL: &str = "saule_set_host";

/// Signature of [`SET_HOST_SYMBOL`]. The pointer is valid for the entire
/// lifetime of the loaded library; the package should stash it.
pub type SetHostFn = unsafe extern "C" fn(*const HostApi);

/// The callback table the host hands a package so it can manipulate
/// host-owned reference values ([`tag::TABLE`], [`tag::FUNC`]) by [`Handle`].
///
/// Every callback takes `ctx` first — the host's opaque context pointer
/// ([`HostApi::ctx`]) — so the host can locate its handle registry. All
/// `CValue` arguments follow the same ownership rules as a normal call:
/// string payloads are borrowed for the duration of the callback, and any
/// table/func `CValue` carries a [`Handle`] valid in the current call scope.
#[repr(C)]
pub struct HostApi {
    /// Opaque host context, passed back into every callback below.
    pub ctx: *mut core::ffi::c_void,

    /// Allocate a fresh empty `table` on the host and return its handle
    /// (`0` on failure).
    pub table_new: unsafe extern "C" fn(ctx: *mut core::ffi::c_void) -> Handle,

    /// Number of array-part elements in the table, or `-1` for a bad handle.
    pub table_len: unsafe extern "C" fn(ctx: *mut core::ffi::c_void, table: Handle) -> i64,

    /// Read `table[key]` into `out`. Returns `0` on success (writes `nil`
    /// for a missing key), non-zero on error (bad handle / unusable key),
    /// writing an [`tag::ERR`] message into `out`.
    pub table_get: unsafe extern "C" fn(
        ctx: *mut core::ffi::c_void,
        table: Handle,
        key: *const CValue,
        out: *mut CValue,
    ) -> i32,

    /// Assign `table[key] = val`. Returns `0` on success, non-zero on error.
    pub table_set: unsafe extern "C" fn(
        ctx: *mut core::ffi::c_void,
        table: Handle,
        key: *const CValue,
        val: *const CValue,
    ) -> i32,

    /// Append `val` to the table's array part. Returns `0` on success.
    pub table_push: unsafe extern "C" fn(
        ctx: *mut core::ffi::c_void,
        table: Handle,
        val: *const CValue,
    ) -> i32,

    /// Remove `key` from the table. Returns `0` on success.
    pub table_remove: unsafe extern "C" fn(
        ctx: *mut core::ffi::c_void,
        table: Handle,
        key: *const CValue,
    ) -> i32,

    /// Return a *new* array-table handle holding this table's keys, so a
    /// package can iterate without an open iterator protocol. `0` on error.
    pub table_keys: unsafe extern "C" fn(ctx: *mut core::ffi::c_void, table: Handle) -> Handle,

    /// Invoke a host callable with `argc` positional `CValue` arguments,
    /// writing its (first) return value into `out`. Returns `0` on success;
    /// non-zero means the callee threw, with an [`tag::ERR`] message in
    /// `out`.
    pub func_call: unsafe extern "C" fn(
        ctx: *mut core::ffi::c_void,
        func: Handle,
        args: *const CValue,
        argc: usize,
        out: *mut CValue,
    ) -> i32,
}

/// FFI-safe tagged value. A flat struct (not a real union) so both sides
/// can construct and inspect it without `unsafe` field access. Only the
/// field selected by [`CValue::tag`] is meaningful.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CValue {
    /// One of the [`tag`] constants.
    pub tag: u8,
    /// Valid when `tag == tag::BOOL`. `0` = false, non-zero = true.
    pub boolean: u8,
    /// Valid when `tag == tag::INT`.
    pub integer: i64,
    /// Valid when `tag == tag::FLOAT`.
    pub float: f64,
    /// Pointer to UTF-8 bytes. Valid when `tag == tag::STR` or `tag::ERR`.
    pub str_ptr: *const u8,
    /// Byte length of the string pointed to by [`CValue::str_ptr`].
    pub str_len: usize,
}

impl CValue {
    /// A `nil` value.
    pub fn nil() -> Self {
        Self {
            tag: tag::NIL,
            boolean: 0,
            integer: 0,
            float: 0.0,
            str_ptr: std::ptr::null(),
            str_len: 0,
        }
    }

    /// A `boolean` value.
    pub fn boolean(b: bool) -> Self {
        Self {
            tag: tag::BOOL,
            boolean: b as u8,
            ..Self::nil()
        }
    }

    /// An `integer` value.
    pub fn integer(i: i64) -> Self {
        Self {
            tag: tag::INT,
            integer: i,
            ..Self::nil()
        }
    }

    /// A `float` value.
    pub fn float(f: f64) -> Self {
        Self {
            tag: tag::FLOAT,
            float: f,
            ..Self::nil()
        }
    }

    /// A `string` value borrowing `bytes`. The caller must keep `bytes`
    /// alive for as long as the receiver may read it (see the module-level
    /// string-ownership rules). Prefer [`return_string`] on the package
    /// side, which manages the lifetime for you.
    ///
    /// # Safety
    /// `bytes` must remain valid and immutable for the duration of the
    /// receiver's access.
    pub fn string_borrowed(bytes: &[u8]) -> Self {
        Self {
            tag: tag::STR,
            str_ptr: bytes.as_ptr(),
            str_len: bytes.len(),
            ..Self::nil()
        }
    }

    /// A host-owned `table` referenced by [`Handle`].
    pub fn table_handle(h: Handle) -> Self {
        Self {
            tag: tag::TABLE,
            integer: h as i64,
            ..Self::nil()
        }
    }

    /// A host-owned callable referenced by [`Handle`].
    pub fn func_handle(h: Handle) -> Self {
        Self {
            tag: tag::FUNC,
            integer: h as i64,
            ..Self::nil()
        }
    }

    /// The [`Handle`] carried by a [`tag::TABLE`] or [`tag::FUNC`] value, or
    /// `None` for any other tag.
    pub fn as_handle(&self) -> Option<Handle> {
        match self.tag {
            tag::TABLE | tag::FUNC => Some(self.integer as Handle),
            _ => None,
        }
    }

    /// Read the string payload as a `&str`, or `None` if this value is not
    /// a string / error or the bytes are not valid UTF-8.
    ///
    /// # Safety
    /// The pointer must be valid for `str_len` bytes — true for any
    /// `CValue` the peer produced through this crate's constructors and
    /// not yet invalidated.
    pub unsafe fn as_str(&self) -> Option<&str> {
        if self.tag != tag::STR && self.tag != tag::ERR {
            return None;
        }
        if self.str_ptr.is_null() {
            return Some("");
        }
        // SAFETY: caller guarantees `str_ptr` is valid for `str_len` bytes.
        let slice = unsafe { std::slice::from_raw_parts(self.str_ptr, self.str_len) };
        std::str::from_utf8(slice).ok()
    }
}

thread_local! {
    /// Holds the bytes of the most recent string returned by a native call
    /// on this thread, keeping them alive until the interpreter copies them
    /// out (which it does immediately on return). Overwritten on the next
    /// call — see the module-level string-ownership contract.
    static RETURN_BUF: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Build a string [`CValue`] whose bytes live in a thread-local buffer that
/// survives until this thread's next native call. Use this from a package's
/// exported function when returning a `string`; the interpreter copies the
/// bytes before relinquishing control, so the single-slot buffer is safe.
pub fn return_string(s: &str) -> CValue {
    RETURN_BUF.with(|buf| {
        let mut buf = buf.borrow_mut();
        buf.clear();
        buf.extend_from_slice(s.as_bytes());
        CValue {
            tag: tag::STR,
            str_ptr: buf.as_ptr(),
            str_len: buf.len(),
            ..CValue::nil()
        }
    })
}

/// Build an error [`CValue`] (tag [`tag::ERR`]) for the `out` slot. Pair it
/// with a non-zero return code. The message bytes follow the same
/// thread-local lifetime as [`return_string`].
pub fn return_error(msg: &str) -> CValue {
    let mut v = return_string(msg);
    v.tag = tag::ERR;
    v
}

/// The frozen signature every exported native package function implements.
///
/// `(args, argc, out) -> code`: see the module-level docs. The interpreter
/// transmutes a resolved symbol to this type to call it.
pub type NativeSymbolFn = unsafe extern "C" fn(*const CValue, usize, *mut CValue) -> i32;
