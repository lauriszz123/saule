//! Access to the host callback table ([`HostApi`]) for manipulating
//! host-owned reference values (`table`s and callables) from inside a package.
//!
//! Scalar values (`integer`, `float`, `boolean`, `string`) cross the native
//! ABI *by value*, so a package can own them outright. Reference values can't
//! — their identity is an `Rc` living on the interpreter side. Instead the
//! host hands the package a small vtable of function pointers ([`HostApi`])
//! and the package operates on those values *by [`Handle`]*.
//!
//! The interpreter installs the vtable by calling the package's generated
//! `saule_set_host` export immediately after loading the library (see
//! [`crate::saule_package`]). Until that happens — e.g. if someone loads the
//! `cdylib` outside the interpreter — [`STable`](crate::types::STable) and
//! [`SFunction`](crate::types::SFunction) operations panic with a clear
//! message rather than dereferencing a null vtable.
//!
//! ## Handle lifetime
//!
//! A handle is only valid for the duration of the native call that produced
//! it (including nested host callbacks). Do not stash an `STable` / `SFunction`
//! in a `static` and use it after your export returns.

use core::sync::atomic::{AtomicPtr, Ordering};

use saule_native_abi::{CValue, Handle, HostApi};

/// The installed host vtable, or null before `saule_set_host` runs.
static HOST: AtomicPtr<HostApi> = AtomicPtr::new(core::ptr::null_mut());

/// Store the host API pointer. Called by the generated `saule_set_host`
/// export the moment the interpreter loads the package. Not a stable API.
///
/// # Safety
/// `api` must point to a [`HostApi`] that stays valid for the lifetime of the
/// loaded library — the interpreter guarantees this.
#[doc(hidden)]
pub unsafe fn __set_host(api: *const HostApi) {
    HOST.store(api as *mut HostApi, Ordering::Release);
}

/// Borrow the installed host API, panicking with a clear message if the
/// package is being used outside the Saule interpreter.
fn host() -> &'static HostApi {
    let p = HOST.load(Ordering::Acquire);
    assert!(
        !p.is_null(),
        "saule-sdk: host API not installed — `STable` / `SFunction` only work \
         when the package is loaded by the Saule interpreter"
    );
    // SAFETY: non-null, and the host installed a pointer to a `HostApi` that
    // outlives every call into this library.
    unsafe { &*p }
}

/// Read an error message out of a callback's `out` slot.
fn read_err(out: &CValue) -> String {
    // SAFETY: on failure the host writes an ERR/STR payload into `out`.
    unsafe { out.as_str() }
        .unwrap_or("native host error")
        .to_string()
}

/// Allocate a fresh empty host table; returns its handle.
pub(crate) fn table_new() -> Handle {
    let h = host();
    // SAFETY: `ctx` is the host's own context; the call matches the frozen sig.
    unsafe { (h.table_new)(h.ctx) }
}

/// Array-part length of a host table (`-1` for a bad handle).
pub(crate) fn table_len(t: Handle) -> i64 {
    let h = host();
    // SAFETY: see `table_new`.
    unsafe { (h.table_len)(h.ctx, t) }
}

/// Read `table[key]`.
pub(crate) fn table_get(t: Handle, key: &CValue) -> Result<CValue, String> {
    let h = host();
    let mut out = CValue::nil();
    // SAFETY: `key` and `&mut out` are valid for the call.
    let code = unsafe { (h.table_get)(h.ctx, t, key, &mut out) };
    if code == 0 {
        Ok(out)
    } else {
        Err(read_err(&out))
    }
}

/// Assign `table[key] = val`.
pub(crate) fn table_set(t: Handle, key: &CValue, val: &CValue) -> Result<(), String> {
    let h = host();
    // SAFETY: `key` / `val` are valid for the call.
    let code = unsafe { (h.table_set)(h.ctx, t, key, val) };
    if code == 0 {
        Ok(())
    } else {
        Err("native host: table assignment failed (bad handle or key)".to_string())
    }
}

/// Append `val` to the table's array part.
pub(crate) fn table_push(t: Handle, val: &CValue) -> Result<(), String> {
    let h = host();
    // SAFETY: `val` is valid for the call.
    let code = unsafe { (h.table_push)(h.ctx, t, val) };
    if code == 0 {
        Ok(())
    } else {
        Err("native host: table push failed (bad handle)".to_string())
    }
}

/// Remove `key` from the table.
pub(crate) fn table_remove(t: Handle, key: &CValue) -> Result<(), String> {
    let h = host();
    // SAFETY: `key` is valid for the call.
    let code = unsafe { (h.table_remove)(h.ctx, t, key) };
    if code == 0 {
        Ok(())
    } else {
        Err("native host: table remove failed (bad handle or key)".to_string())
    }
}

/// Obtain a new array-table of the table's keys (`0` on error).
pub(crate) fn table_keys(t: Handle) -> Result<Handle, String> {
    let h = host();
    // SAFETY: see `table_new`.
    let handle = unsafe { (h.table_keys)(h.ctx, t) };
    if handle == 0 {
        Err("native host: could not read table keys (bad handle)".to_string())
    } else {
        Ok(handle)
    }
}

/// Invoke a host callable with positional `args`, returning its first result.
pub(crate) fn func_call(f: Handle, args: &[CValue]) -> Result<CValue, String> {
    let h = host();
    let mut out = CValue::nil();
    // SAFETY: `args` is a valid contiguous slice for `args.len()` elements,
    // `&mut out` is a valid writable slot.
    let code = unsafe { (h.func_call)(h.ctx, f, args.as_ptr(), args.len(), &mut out) };
    if code == 0 {
        Ok(out)
    } else {
        Err(read_err(&out))
    }
}
