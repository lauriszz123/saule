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

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::ptr;
use std::rc::Rc;

use saule_native_abi::{CValue, Handle, HostApi, ObjectReleaseFn, SET_HOST_SYMBOL, SetHostFn, tag};

use crate::call::call_value_first;
use crate::fxhash::FxHashMap;
use crate::value::SauleStr;
use crate::value::{ForeignClass, ForeignObject, TableObject, Value};

/// What converting values for one package needs to know about it: which
/// classes its objects can be, and how to give an object back.
///
/// Objects are the one kind of value whose meaning depends on *which*
/// package is on the other side — a pointer from one package means nothing
/// to another — so every call into a package names its context
/// ([`enter_package`]), and every object conversion checks against it.
pub struct PackageCtx {
    pub name: Rc<str>,
    /// The package's `#[saule_class]`es, by name.
    pub classes: FxHashMap<String, Rc<ForeignClass>>,
    /// The package's `saule_object_release`, once its library is loaded.
    pub release: Cell<Option<ObjectReleaseFn>>,
}

thread_local! {
    /// Call-scoped registry of host-owned reference values. Slot `0` is never
    /// used so handle `0` is always invalid.
    static REGISTRY: RefCell<Registry> = RefCell::new(Registry::new());

    /// The packages currently being called into, innermost last. A package
    /// can call back into Saule, which can call another package, so this is
    /// a stack.
    static PACKAGES: RefCell<Vec<Rc<PackageCtx>>> = const { RefCell::new(Vec::new()) };

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

/// Make `ctx` the package the conversions below are for, until the matching
/// [`exit_package`]. Pair with it around every call into a package.
pub fn enter_package(ctx: Rc<PackageCtx>) {
    PACKAGES.with(|p| p.borrow_mut().push(ctx));
}

/// Undo the last [`enter_package`].
pub fn exit_package() {
    PACKAGES.with(|p| {
        p.borrow_mut().pop();
    });
}

fn current_package() -> Option<Rc<PackageCtx>> {
    PACKAGES.with(|p| p.borrow().last().cloned())
}

/// Why `v` cannot cross into the package being called, or `None` if it can.
/// The one place the answer is worked out, so an argument, a table cell and
/// a callback's result are refused for the same reasons in the same words.
pub fn refusal(v: &Value) -> Option<String> {
    match v {
        Value::Foreign(o) => match current_package() {
            Some(ctx) if *ctx.name == *o.class.package => None,
            Some(ctx) => Some(format!(
                "a {} belongs to package `{}` and cannot be handed to package `{}`",
                o.class.name, o.class.package, ctx.name
            )),
            None => Some(format!(
                "a {} cannot leave the interpreter here",
                o.class.name
            )),
        },
        Value::EnumVariant(ev) if ev.value.get().is_some_and(|v| matches!(v, Value::Table(_))) => {
            Some(format!(
                "`{}.{}` carries data, and only plain enum variants cross into a package",
                ev.enum_name, ev.variant_name
            ))
        }
        _ => None,
    }
}

/// Borrowed `Value -> CValue`. Scalars convert directly; a `table` or callable
/// is parked in the registry and converts to a handle; an object of the
/// package being called is parked too (which keeps it alive for the call)
/// and converts to its pointer; a plain enum variant converts to its name.
/// Returns `None` for values with no ABI representation (Saule instances,
/// classes, another package's objects — see [`refusal`]). String payloads
/// borrow from `v`.
pub fn value_to_cvalue(v: &Value) -> Option<CValue> {
    if refusal(v).is_some() {
        return None;
    }
    Some(match v {
        Value::Nil => CValue::nil(),
        Value::Bool(b) => CValue::boolean(*b),
        Value::Int(i) => CValue::integer(*i),
        Value::Float(f) => CValue::float(*f),
        Value::Str(s) => CValue::string_borrowed(s.as_bytes()),
        Value::Table(_) => CValue::table_handle(register(v.clone())),
        Value::Native(_) | Value::NativeClosure(_) | Value::VmFunction(_) => {
            CValue::func_handle(register(v.clone()))
        }
        Value::Foreign(o) => {
            register(v.clone());
            CValue::object_borrowed(o.ptr(), o.class.name.as_bytes())
        }
        // A package enum crosses as its variant's name; the name is shared
        // with the variant, which the caller keeps alive for the call.
        Value::EnumVariant(ev) => CValue::string_borrowed(ev.variant_name.as_bytes()),
        _ => return None,
    })
}

/// Owned `CValue -> Value`. Copies string payloads; resolves table / func
/// handles back to the parked value (or `nil` if the handle is stale); and
/// takes over the reference an object carries (see
/// `saule_native_abi::ObjectPtr`).
pub fn cvalue_to_value(c: &CValue) -> Value {
    match c.tag {
        tag::BOOL => Value::Bool(c.boolean != 0),
        tag::INT => Value::Int(c.integer),
        tag::FLOAT => Value::Float(c.float),
        // SAFETY: a STR tag implies a valid `(ptr, len)` pair from the peer.
        tag::STR => Value::Str(SauleStr::new(
            unsafe { c.as_str() }.unwrap_or("").to_string(),
        )),
        tag::TABLE | tag::FUNC => resolve(c.as_handle().unwrap_or(0)).unwrap_or(Value::Nil),
        tag::OBJECT => adopt_object(c).unwrap_or(Value::Nil),
        _ => Value::Nil,
    }
}

/// Wrap an object the package is handing over. The reference is ours from
/// here on, so every path either wraps it or gives it straight back.
fn adopt_object(c: &CValue) -> Option<Value> {
    let ptr = c.as_object().filter(|p| !p.is_null())?;
    let ctx = current_package()?;
    // Without a release function there is no way to give the reference
    // back; the only way here is a package loaded without the SDK's
    // exports, which the load already refused.
    let release = ctx.release.get()?;
    // SAFETY: an OBJECT from the package carries its `'static` class name.
    let class = unsafe { c.object_class() }.and_then(|n| ctx.classes.get(n).cloned());
    match class {
        // SAFETY: the package handed us this reference with the value, per
        // the ownership rules, and `release` is its own release function.
        Some(class) => Some(Value::Foreign(Rc::new(unsafe {
            ForeignObject::adopt(ptr, class, release)
        }))),
        None => {
            // A class the package's metadata never declared: a package bug.
            // Give the reference back rather than leak it.
            // SAFETY: as above — we own this reference.
            unsafe { release(ptr) };
            None
        }
    }
}

/// Write `v` into `*out` for return to the package, and return `0`; or, for
/// a value that may not cross (see [`refusal`]), write the reason as an
/// error and return `1`. Strings are copied into a host-local buffer valid
/// until the next such write on this thread.
///
/// # Safety
/// `out` must be a valid, writable `CValue` slot.
unsafe fn write_out(v: &Value, out: *mut CValue) -> i32 {
    if let Some(why) = refusal(v) {
        return unsafe { write_err(out, &why) };
    }
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
    0
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
    // Every value a package passes in is converted before anything can fail:
    // an object carries a reference that is ours from the moment it arrives,
    // and converting is what takes it over (and gives it back on drop).
    let key = cvalue_to_value(unsafe { &*key });
    let Some(Value::Table(t)) = resolve(h) else {
        return unsafe { write_err(out, "table_get: invalid table handle") };
    };
    let v = t.borrow().get(&key);
    unsafe { write_out(&v, out) }
}

unsafe extern "C" fn table_set(
    _ctx: *mut c_void,
    h: Handle,
    key: *const CValue,
    val: *const CValue,
) -> i32 {
    let key = cvalue_to_value(unsafe { &*key });
    let val = cvalue_to_value(unsafe { &*val });
    let Some(Value::Table(t)) = resolve(h) else {
        return 1;
    };
    match t.borrow_mut().set(&key, val) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

unsafe extern "C" fn table_push(_ctx: *mut c_void, h: Handle, val: *const CValue) -> i32 {
    let val = cvalue_to_value(unsafe { &*val });
    let Some(Value::Table(t)) = resolve(h) else {
        return 1;
    };
    t.borrow_mut().array.push(val);
    0
}

unsafe extern "C" fn table_remove(_ctx: *mut c_void, h: Handle, key: *const CValue) -> i32 {
    let key = cvalue_to_value(unsafe { &*key });
    let Some(Value::Table(t)) = resolve(h) else {
        return 1;
    };
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
    let slice: &[CValue] = if args.is_null() || argc == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(args, argc) }
    };
    // Converted first, for the reason `table_get` gives.
    let args: Vec<Value> = slice.iter().map(cvalue_to_value).collect();
    let Some(callee) = resolve(h) else {
        return unsafe { write_err(out, "func_call: invalid function handle") };
    };
    match call_value_first(&callee, &args, 0..0) {
        // An object the callback made and nothing else holds stays alive:
        // `write_out` parks it in the registry until the outermost native
        // call returns, like every other reference value handed over.
        Ok(v) => unsafe { write_out(&v, out) },
        // The bare message. The package usually returns it as its own error,
        // which the interpreter wraps again — so passing the rendered
        // `type error: …` along printed the prefix twice.
        Err(crate::RuntimeError::TypeError { message, .. }) => unsafe { write_err(out, &message) },
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
