//! Host-side bridge for reference values crossing the native ABI.
//!
//! Tables and callables never leave the interpreter's address space. When one
//! is passed across the boundary to a dynamically-loaded package it is parked
//! in a thread-local, call-scoped [`Registry`] and replaced by an opaque
//! [`Handle`]. The package manipulates it through the [`HostApi`] callbacks
//! defined here — `table_get`, `table_set`, `func_call`, … — which resolve the
//! handle back to the real [`Value`].
//!
//! ## Lifetime
//!
//! Handles are valid only within a single top-level native call (and any
//! nested host callbacks it triggers). [`enter`] / [`exit`] bracket each call;
//! when the outermost call returns, the whole registry is reclaimed. A package
//! therefore must not stash a handle to use after its exported function
//! returns — synchronous use (iterate, mutate, invoke a callback now) is the
//! supported model.

use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr;
use std::rc::Rc;

use saule_native_abi::{CValue, Handle, HostApi, SET_HOST_SYMBOL, SetHostFn, tag};

use crate::eval::expr::{EvaluatedArg, call_value_multi};
use crate::value::{TableObject, Value};
use crate::value::SauleStr;

thread_local! {
    /// Call-scoped registry of host-owned reference values. Slot `0` is never
    /// used so handle `0` is always invalid.
    static REGISTRY: RefCell<Registry> = RefCell::new(Registry::new());

    /// Backing store for a single string written into an `out` slot by a host
    /// callback. Valid until this thread's next such write — the package
    /// copies the bytes before the next callback, mirroring the package-side
    /// `return_string` contract.
    static OUT_BUF: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

struct Registry {
    slots: Vec<Option<Value>>,
    free: Vec<usize>,
    /// Native-call re-entrancy depth; the registry is cleared when it returns
    /// to zero.
    depth: u32,
}

impl Registry {
    fn new() -> Self {
        Self {
            slots: vec![None],
            free: Vec::new(),
            depth: 0,
        }
    }

    fn register(&mut self, v: Value) -> Handle {
        let idx = if let Some(i) = self.free.pop() {
            self.slots[i] = Some(v);
            i
        } else {
            self.slots.push(Some(v));
            self.slots.len() - 1
        };
        idx as Handle
    }

    fn resolve(&self, h: Handle) -> Option<Value> {
        self.slots.get(h as usize).and_then(|s| s.clone())
    }

    fn clear(&mut self) {
        self.slots.truncate(1);
        self.free.clear();
    }
}

/// Enter a native-call scope. Pair with [`exit`].
pub fn enter() {
    REGISTRY.with(|r| r.borrow_mut().depth += 1);
}

/// Leave a native-call scope; reclaims every handle once the outermost call
/// returns.
pub fn exit() {
    REGISTRY.with(|r| {
        let mut r = r.borrow_mut();
        r.depth = r.depth.saturating_sub(1);
        if r.depth == 0 {
            r.clear();
        }
    });
}

fn register(v: Value) -> Handle {
    REGISTRY.with(|r| r.borrow_mut().register(v))
}

fn resolve(h: Handle) -> Option<Value> {
    REGISTRY.with(|r| r.borrow().resolve(h))
}

/// Borrowed `Value -> CValue`. Scalars convert directly; a `table` or callable
/// is parked in the registry and converts to a handle. Returns `None` only for
/// values with no ABI representation (instances, classes, enums, …). String
/// payloads borrow from `v`.
pub fn value_to_cvalue(v: &Value) -> Option<CValue> {
    Some(match v {
        Value::Nil => CValue::nil(),
        Value::Bool(b) => CValue::boolean(*b),
        Value::Int(i) => CValue::integer(*i),
        Value::Float(f) => CValue::float(*f),
        Value::Str(s) => CValue::string_borrowed(s.as_bytes()),
        Value::Table(_) => CValue::table_handle(register(v.clone())),
        Value::Native(_) | Value::NativeClosure(_) | Value::Function(_) => {
            CValue::func_handle(register(v.clone()))
        }
        _ => return None,
    })
}

/// Owned `CValue -> Value`. Copies string payloads; resolves table / func
/// handles back to the parked value (or `nil` if the handle is stale).
pub fn cvalue_to_value(c: &CValue) -> Value {
    match c.tag {
        tag::BOOL => Value::Bool(c.boolean != 0),
        tag::INT => Value::Int(c.integer),
        tag::FLOAT => Value::Float(c.float),
        // SAFETY: a STR tag implies a valid `(ptr, len)` pair from the peer.
        tag::STR => Value::Str(SauleStr::new(unsafe { c.as_str() }.unwrap_or("").to_string())),
        tag::TABLE | tag::FUNC => resolve(c.as_handle().unwrap_or(0)).unwrap_or(Value::Nil),
        _ => Value::Nil,
    }
}

/// Write `v` into `*out` for return to the package. Strings are copied into a
/// host-local buffer valid until the next such write on this thread.
///
/// # Safety
/// `out` must be a valid, writable `CValue` slot.
unsafe fn write_out(v: &Value, out: *mut CValue) {
    let cv = match v {
        Value::Str(s) => OUT_BUF.with(|b| {
            let mut b = b.borrow_mut();
            b.clear();
            b.extend_from_slice(s.as_bytes());
            CValue {
                tag: tag::STR,
                str_ptr: b.as_ptr(),
                str_len: b.len(),
                ..CValue::nil()
            }
        }),
        other => value_to_cvalue(other).unwrap_or_else(CValue::nil),
    };
    unsafe { *out = cv };
}

/// Write an [`tag::ERR`] message into `*out` and return `1`.
///
/// # Safety
/// `out` must be a valid, writable `CValue` slot.
unsafe fn write_err(out: *mut CValue, msg: &str) -> i32 {
    let cv = OUT_BUF.with(|b| {
        let mut b = b.borrow_mut();
        b.clear();
        b.extend_from_slice(msg.as_bytes());
        CValue {
            tag: tag::ERR,
            str_ptr: b.as_ptr(),
            str_len: b.len(),
            ..CValue::nil()
        }
    });
    unsafe { *out = cv };
    1
}

// ─── HostApi callbacks ───────────────────────────────────────────────────────

unsafe extern "C" fn table_new(_ctx: *mut c_void) -> Handle {
    register(Value::Table(Rc::new(RefCell::new(TableObject::new()))))
}

unsafe extern "C" fn table_len(_ctx: *mut c_void, h: Handle) -> i64 {
    match resolve(h) {
        Some(Value::Table(t)) => t.borrow().array_len() as i64,
        _ => -1,
    }
}

unsafe extern "C" fn table_get(
    _ctx: *mut c_void,
    h: Handle,
    key: *const CValue,
    out: *mut CValue,
) -> i32 {
    let Some(Value::Table(t)) = resolve(h) else {
        return unsafe { write_err(out, "table_get: invalid table handle") };
    };
    let key = cvalue_to_value(unsafe { &*key });
    let v = t.borrow().get(&key);
    unsafe { write_out(&v, out) };
    0
}

unsafe extern "C" fn table_set(
    _ctx: *mut c_void,
    h: Handle,
    key: *const CValue,
    val: *const CValue,
) -> i32 {
    let Some(Value::Table(t)) = resolve(h) else {
        return 1;
    };
    let key = cvalue_to_value(unsafe { &*key });
    let val = cvalue_to_value(unsafe { &*val });
    match t.borrow_mut().set(&key, val) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

unsafe extern "C" fn table_push(_ctx: *mut c_void, h: Handle, val: *const CValue) -> i32 {
    let Some(Value::Table(t)) = resolve(h) else {
        return 1;
    };
    let val = cvalue_to_value(unsafe { &*val });
    t.borrow_mut().array.push(val);
    0
}

unsafe extern "C" fn table_remove(_ctx: *mut c_void, h: Handle, key: *const CValue) -> i32 {
    let Some(Value::Table(t)) = resolve(h) else {
        return 1;
    };
    let key = cvalue_to_value(unsafe { &*key });
    t.borrow_mut().remove(&key);
    0
}

unsafe extern "C" fn table_keys(_ctx: *mut c_void, h: Handle) -> Handle {
    let Some(Value::Table(t)) = resolve(h) else {
        return 0;
    };
    let t = t.borrow();
    let mut keys: Vec<Value> = Vec::with_capacity(t.array.len() + t.map.len());
    for i in 1..=t.array.len() {
        keys.push(Value::Int(i as i64));
    }
    for k in t.map.keys() {
        keys.push(k.to_value());
    }
    register(Value::Table(Rc::new(RefCell::new(
        TableObject::from_array(keys),
    ))))
}

unsafe extern "C" fn func_call(
    _ctx: *mut c_void,
    h: Handle,
    args: *const CValue,
    argc: usize,
    out: *mut CValue,
) -> i32 {
    let Some(callee) = resolve(h) else {
        return unsafe { write_err(out, "func_call: invalid function handle") };
    };
    let slice: &[CValue] = if args.is_null() || argc == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(args, argc) }
    };
    let evaled: Vec<EvaluatedArg> = slice
        .iter()
        .map(|c| EvaluatedArg::Positional(cvalue_to_value(c)))
        .collect();
    match call_value_multi(callee, &evaled, 0..0) {
        Ok(vs) => {
            let v = vs.into_iter().next().unwrap_or(Value::Nil);
            unsafe { write_out(&v, out) };
            0
        }
        Err(e) => unsafe { write_err(out, &e.to_string()) },
    }
}

// ─── Host API table handed to packages ───────────────────────────────────────

/// Wrapper that lets the [`HostApi`] (which holds a raw `ctx` pointer) live in
/// a `static`. The pointer is null and all other fields are `'static` function
/// pointers, so sharing it across threads is sound.
struct StaticHostApi(HostApi);
// SAFETY: `ctx` is null and the function pointers are `'static`; nothing in the
// struct is mutated after construction.
unsafe impl Sync for StaticHostApi {}

static HOST_API: StaticHostApi = StaticHostApi(HostApi {
    ctx: ptr::null_mut(),
    table_new,
    table_len,
    table_get,
    table_set,
    table_push,
    table_remove,
    table_keys,
    func_call,
});

/// Hand a freshly-loaded package its [`HostApi`] by calling the optional
/// [`SET_HOST_SYMBOL`] export. Packages that only deal in scalars may omit the
/// symbol, in which case this is a no-op.
///
/// # Safety
/// `lib` must be a library just loaded for a native package; the symbol, if
/// present, must have the [`SetHostFn`] signature (guaranteed for packages
/// built with `saule-sdk`).
#[cfg(feature = "native-packages")]
pub unsafe fn install_host(lib: &libloading::Library) {
    if let Ok(sym) = unsafe { lib.get::<SetHostFn>(SET_HOST_SYMBOL.as_bytes()) } {
        unsafe { sym(&HOST_API.0 as *const HostApi) };
    }
}
