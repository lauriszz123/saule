//! `extern interface` in a running program: the glue between Saule values
//! and the [`Looper`].
//!
//! - **Proxies.** A value of an `extern interface` type that lives in
//!   another process is a [`ForeignObject`] — the same kind of value a
//!   native package's object is — whose class has one method per interface
//!   method. Calling one converts the arguments to [`Wire`] by the types the
//!   compiler described ([`crate::ipc_desc`]) and calls over the connection;
//!   `async fn` sends without waiting. One proxy per remote object, so two
//!   references to the same window compare equal; when the last one goes,
//!   the object is released on the other side.
//! - **Exports.** A Saule object sent where an `extern interface` is
//!   expected — published as a service, or passed as an argument or result —
//!   is kept alive here, under a key, for as long as some connection holds a
//!   reference to it.
//! - **Incoming calls** run the Saule method on the exported object
//!   ([`Glue`]). While that Saule code runs, the looper is lent back to the
//!   thread, so the method can itself call other processes or dispatch.
//!
//! All of it is per thread, like the VM.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::rc::{Rc, Weak};
use std::time::Duration;

use super::{CallError, ConnId, Handler, Looper, ObjRef, ObjectId, ROOT, Wire};
use crate::fxhash::fxmap;
use crate::ipc_desc::{Bundle, DataDesc, IfaceDesc, MethodDesc, Ty};
use crate::value::{
    ClassObject, ForeignClass, ForeignObject, InstanceObject, NativeClosure, SauleBytes, SauleStr,
    TableObject, Value,
};

/// The `package` of a proxy's class — how a proxy is told apart from a
/// native package's object.
const PROXY_PACKAGE: &str = "#extern";

struct Export {
    value: Value,
    iface: Rc<IfaceDesc>,
    /// Connections that hold a reference.
    conns: HashSet<ConnId>,
    /// Published as a service: kept until the process ends.
    published: bool,
}

/// What a proxy's pointer points at.
struct Proxy {
    conn: ConnId,
    id: ObjectId,
    iface: Rc<IfaceDesc>,
}

#[derive(Default)]
struct State {
    ifaces: HashMap<String, Rc<IfaceDesc>>,
    data: HashMap<String, Rc<DataDesc>>,
    /// Descriptions already read, by their text.
    loaded: HashSet<String>,
    /// The program's classes by name, for rebuilding copied data classes.
    classes: HashMap<String, Rc<ClassObject>>,
    exports: HashMap<u64, Export>,
    /// An exported object's identity → its key, so sending one object twice
    /// sends one reference.
    export_keys: HashMap<usize, u64>,
    next_key: u64,
    proxies: HashMap<(ConnId, ObjectId), Weak<ForeignObject>>,
    proxy_classes: HashMap<String, Rc<ForeignClass>>,
    /// Proxies dropped while the looper was busy; released at its next use.
    pending_release: Vec<(ConnId, ObjectId)>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
    /// `None` while the looper is in use further up the stack.
    static LOOPER: RefCell<Option<Looper>> = const { RefCell::new(None) };
    /// The connection whose call is being answered right now.
    static CALLER: Cell<Option<ConnId>> = const { Cell::new(None) };
    static STOP: Cell<bool> = const { Cell::new(false) };
}

fn state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

/// Run `f` with this thread's looper.
///
/// The looper is taken out of its slot while `f` runs and put back after.
/// Code `f` leads to that runs Saule — an incoming call's method — puts it
/// back for that long (see [`Glue::call`]), so it is always where the next
/// caller looks.
fn with_looper<R>(f: impl FnOnce(&mut Looper) -> R) -> R {
    let mut looper = LOOPER.with(|l| l.borrow_mut().take()).unwrap_or_default();
    let pending = state(|s| std::mem::take(&mut s.pending_release));
    for (conn, id) in pending {
        looper.release(conn, id);
    }
    let r = f(&mut looper);
    LOOPER.with(|l| *l.borrow_mut() = Some(looper));
    r
}

// --- Descriptions and classes --------------------------------------------------

/// Read an interface description the compiler wrote, once per text.
fn load(text: &str) -> Result<Rc<IfaceDesc>, String> {
    let bundle = Bundle::parse(text)?;
    let root = bundle.root().expect("parse checks for one").name.clone();
    state(|s| {
        if s.loaded.insert(text.to_string()) {
            for i in bundle.ifaces {
                s.ifaces.entry(i.name.clone()).or_insert_with(|| Rc::new(i));
            }
            for d in bundle.data {
                s.data.entry(d.name.clone()).or_insert_with(|| Rc::new(d));
            }
        }
        Ok(s.ifaces[&root].clone())
    })
}

fn iface(name: &str) -> Result<Rc<IfaceDesc>, String> {
    state(|s| s.ifaces.get(name).cloned())
        .ok_or_else(|| format!("no `extern interface {name}` is known to this program"))
}

/// Record the program's classes, so a data class arriving from another
/// process is rebuilt as an instance of the same class here. Called by the
/// VM once its classes exist.
pub fn register_classes(classes: &[Rc<ClassObject>]) {
    state(|s| {
        for c in classes {
            s.classes.entry(c.name.clone()).or_insert_with(|| c.clone());
        }
    });
}

// --- Proxies -------------------------------------------------------------------

fn proxy_class(iface: &Rc<IfaceDesc>) -> Rc<ForeignClass> {
    if let Some(c) = state(|s| s.proxy_classes.get(&iface.name).cloned()) {
        return c;
    }
    let mut methods = fxmap();
    for m in &iface.methods {
        let iface = iface.clone();
        let name = m.name.clone();
        let label: &'static str = Box::leak(format!("{}.{}", iface.name, m.name).into_boxed_str());
        methods.insert(
            m.name.clone(),
            Value::NativeClosure(Rc::new(NativeClosure {
                name: label,
                func: Box::new(move |args: &[Value]| remote_call(&iface, &name, args)),
                param_names: Vec::new(),
            })),
        );
    }
    let class = Rc::new(ForeignClass {
        name: iface.name.clone(),
        package: Rc::from(PROXY_PACKAGE),
        methods,
        getters: fxmap(),
        setters: fxmap(),
    });
    state(|s| s.proxy_classes.insert(iface.name.clone(), class.clone()));
    class
}

unsafe extern "C" fn release_proxy(ptr: *mut c_void) {
    // SAFETY: `ptr` came from `Box::into_raw` in `proxy`, and a
    // `ForeignObject` releases its pointer exactly once.
    let proxy = unsafe { Box::from_raw(ptr.cast::<Proxy>()) };
    // During thread teardown there is nothing left to release into.
    let _ = STATE.try_with(|s| {
        let mut s = s.borrow_mut();
        s.proxies.remove(&(proxy.conn, proxy.id));
        s.pending_release.push((proxy.conn, proxy.id));
    });
}

/// The proxy for object `id` at the other end of `conn`: the existing one
/// if there is one, so identity holds.
fn proxy(conn: ConnId, id: ObjectId, iface: Rc<IfaceDesc>) -> Value {
    if let Some(p) = state(|s| s.proxies.get(&(conn, id)).and_then(Weak::upgrade)) {
        return Value::Foreign(p);
    }
    // A release queued for an earlier proxy of this object must not go out
    // now: it would give back the reference this new proxy stands on.
    state(|s| s.pending_release.retain(|&p| p != (conn, id)));
    let class = proxy_class(&iface);
    let ptr = Box::into_raw(Box::new(Proxy { conn, id, iface })).cast::<c_void>();
    // SAFETY: `ptr` is a fresh `Box<Proxy>` this object now owns, and
    // `release_proxy` gives it back.
    let obj = Rc::new(unsafe { ForeignObject::adopt(ptr, class, release_proxy) });
    state(|s| s.proxies.insert((conn, id), Rc::downgrade(&obj)));
    Value::Foreign(obj)
}

fn as_proxy(value: &Value) -> Option<&Proxy> {
    match value {
        Value::Foreign(obj) if &*obj.class.package == PROXY_PACKAGE => {
            // SAFETY: every object of a `PROXY_PACKAGE` class was made by
            // `proxy` around a `Box<Proxy>`, alive as long as `obj` is.
            Some(unsafe { &*obj.ptr().cast::<Proxy>() })
        }
        _ => None,
    }
}

/// A method call on a proxy: the call goes to the other process.
fn remote_call(iface: &IfaceDesc, method: &str, args: &[Value]) -> Result<Vec<Value>, String> {
    let receiver = args.first().and_then(as_proxy).ok_or("not called on a proxy")?;
    let m = iface
        .method(method)
        .ok_or_else(|| format!("`{}` has no method `{method}`", iface.name))?;
    let given = &args[1..];
    if given.len() > m.params.len() {
        return Err(format!(
            "`{}.{method}` takes {} argument(s), got {}",
            iface.name,
            m.params.len(),
            given.len()
        ));
    }
    let (conn, id) = (receiver.conn, receiver.id);
    let mut wires = Vec::with_capacity(m.params.len());
    for (i, ty) in m.params.iter().enumerate() {
        let v = given.get(i).unwrap_or(&Value::Nil);
        wires.push(to_wire(v, ty, conn).map_err(|e| format!("argument {} of `{}.{method}`: {e}", i + 1, iface.name))?);
    }
    if m.is_async {
        // Fire and forget means exactly that: an `async fn` to a process
        // that has gone is dropped, the way a one-way Binder call is. The
        // caller learns of the disconnect from its next waited-for call.
        let _ = with_looper(|lp| lp.send(conn, id, method, wires));
        return Ok(Vec::new());
    }
    let results = with_looper(|lp| lp.call(&mut Glue, conn, id, method, wires))
        .map_err(|e| thrown(call_error(e)))?;
    from_wire_results(results, m, conn)
}

/// A failure the calling Saule code can `catch` as a `string`.
fn thrown(message: String) -> String {
    crate::call::throw_from_native(Value::Str(SauleStr::new(message.clone())));
    message
}

fn from_wire_results(results: Vec<Wire>, m: &MethodDesc, conn: ConnId) -> Result<Vec<Value>, String> {
    let mut values = Vec::with_capacity(m.returns.len());
    let mut results = results.into_iter();
    for ty in &m.returns {
        let w = results.next().unwrap_or(Wire::Nil);
        values.push(from_wire(w, ty, conn).map_err(|e| format!("the result of `{}`: {e}", m.name))?);
    }
    Ok(values)
}

fn call_error(e: CallError) -> String {
    match e {
        CallError::Disconnected => {
            "the other process has gone away (it ended, crashed, or closed the connection)".into()
        }
        CallError::Remote(m) => m,
        CallError::Invalid(m) => m,
    }
}

// --- Exports -------------------------------------------------------------------

fn identity(value: &Value) -> Option<usize> {
    match value {
        Value::Instance(i) => Some(Rc::as_ptr(i) as *const c_void as usize),
        _ => None,
    }
}

/// The key `value` is exported under as an `iface_name`, exporting it if it
/// is not yet, and noting that `conn` will hold a reference.
fn export(value: &Value, iface_name: &str, conn: Option<ConnId>, published: bool) -> Result<u64, String> {
    let id = identity(value).ok_or_else(|| {
        format!(
            "{} cannot be sent as a `{iface_name}` — only an object of a class implementing it can",
            crate::error::a_type(value.type_name())
        )
    })?;
    let desc = iface(iface_name)?;
    state(|s| {
        let key = match s.export_keys.get(&id) {
            Some(&k) => k,
            None => {
                s.next_key += 1;
                let k = s.next_key;
                s.export_keys.insert(id, k);
                s.exports.insert(
                    k,
                    Export {
                        value: value.clone(),
                        iface: desc,
                        conns: HashSet::new(),
                        published: false,
                    },
                );
                k
            }
        };
        let e = s.exports.get_mut(&key).expect("just ensured");
        e.published |= published;
        if let Some(c) = conn {
            e.conns.insert(c);
        }
        Ok(key)
    })
}

fn drop_export_if_unused(s: &mut State, key: u64) {
    if let Some(e) = s.exports.get(&key)
        && e.conns.is_empty()
        && !e.published
    {
        let e = s.exports.remove(&key).expect("just read");
        if let Some(id) = identity(&e.value) {
            s.export_keys.remove(&id);
        }
    }
}

// --- Values to wire and back -----------------------------------------------------

fn mismatch(want: &str, got: &Value) -> String {
    format!(
        "expected {want}, got {}",
        crate::error::a_type(got.type_name())
    )
}

fn to_wire(v: &Value, ty: &Ty, conn: ConnId) -> Result<Wire, String> {
    Ok(match (ty, v) {
        (_, Value::Nil) => Wire::Nil,
        (Ty::Opt(t), v) => return to_wire(v, t, conn),
        (Ty::Any, v) => return any_to_wire(v, conn, 0),
        (Ty::Bool, Value::Bool(b)) => Wire::Bool(*b),
        (Ty::Int, Value::Int(i)) => Wire::Int(*i),
        (Ty::Float, Value::Float(f)) => Wire::Float(*f),
        (Ty::Float, Value::Int(i)) => Wire::Float(*i as f64),
        (Ty::Str, Value::Str(s)) => Wire::Str(s.as_str().to_string()),
        (Ty::Bytes, Value::Bytes(b)) => Wire::Bytes(b.as_slice().to_vec()),
        (Ty::List(t), Value::Table(tb)) => {
            let tb = tb.borrow();
            Wire::List(tb.array.iter().map(|v| to_wire(v, t, conn)).collect::<Result<_, _>>()?)
        }
        (Ty::Map(k, vt), Value::Table(tb)) => {
            let tb = tb.borrow();
            let mut pairs = Vec::with_capacity(tb.array.len() + tb.map.len());
            for (i, v) in tb.array.iter().enumerate() {
                pairs.push((to_wire(&Value::Int(i as i64 + 1), k, conn)?, to_wire(v, vt, conn)?));
            }
            for (key, v) in &tb.map {
                pairs.push((to_wire(&key.to_value(), k, conn)?, to_wire(v, vt, conn)?));
            }
            Wire::Map(pairs)
        }
        (Ty::Iface(name), v) => object_to_wire(v, name, conn)?,
        (Ty::Data(name), Value::Instance(inst)) => {
            let desc = state(|s| s.data.get(name).cloned())
                .ok_or_else(|| format!("no data class `{name}` is known to this program"))?;
            let inst = inst.borrow();
            let mut fields = Vec::with_capacity(desc.fields.len());
            for (f, t) in &desc.fields {
                let v = inst.field(f).unwrap_or(&Value::Nil);
                fields.push((f.clone(), to_wire(v, t, conn).map_err(|e| format!("field `{f}`: {e}"))?));
            }
            Wire::Record {
                class: name.clone(),
                fields,
            }
        }
        (Ty::Nil, v) => return Err(mismatch("nil", v)),
        (Ty::Bool, v) => return Err(mismatch("a boolean", v)),
        (Ty::Int, v) => return Err(mismatch("an integer", v)),
        (Ty::Float, v) => return Err(mismatch("a float", v)),
        (Ty::Str, v) => return Err(mismatch("a string", v)),
        (Ty::Bytes, v) => return Err(mismatch("Bytes", v)),
        (Ty::List(_) | Ty::Map(..), v) => return Err(mismatch("a table", v)),
        (Ty::Data(name), v) => return Err(mismatch(&format!("a `{name}`"), v)),
    })
}

fn object_to_wire(v: &Value, iface_name: &str, conn: ConnId) -> Result<Wire, String> {
    if let Some(p) = as_proxy(v) {
        if p.conn != conn {
            return Err(format!(
                "this `{}` belongs to another connection, and objects cannot be passed between \
                 connections yet",
                p.iface.name
            ));
        }
        return Ok(Wire::Object(ObjRef::Remote {
            id: p.id,
            interface: p.iface.name.clone(),
        }));
    }
    let key = export(v, iface_name, Some(conn), false)?;
    Ok(Wire::Object(ObjRef::Local {
        key,
        interface: iface_name.to_string(),
    }))
}

/// A value whose type is `any`: sent as what it is.
fn any_to_wire(v: &Value, conn: ConnId, depth: usize) -> Result<Wire, String> {
    if depth > super::wire::MAX_DEPTH {
        return Err("a value nested too deeply to send".into());
    }
    Ok(match v {
        Value::Nil => Wire::Nil,
        Value::Bool(b) => Wire::Bool(*b),
        Value::Int(i) => Wire::Int(*i),
        Value::Float(f) => Wire::Float(*f),
        Value::Str(s) => Wire::Str(s.as_str().to_string()),
        Value::Bytes(b) => Wire::Bytes(b.as_slice().to_vec()),
        Value::Table(tb) => {
            let tb = tb.borrow();
            if tb.map.is_empty() {
                Wire::List(tb.array.iter().map(|v| any_to_wire(v, conn, depth + 1)).collect::<Result<_, _>>()?)
            } else {
                let mut pairs = Vec::new();
                for (i, v) in tb.array.iter().enumerate() {
                    pairs.push((Wire::Int(i as i64 + 1), any_to_wire(v, conn, depth + 1)?));
                }
                for (k, v) in &tb.map {
                    pairs.push((any_to_wire(&k.to_value(), conn, depth + 1)?, any_to_wire(v, conn, depth + 1)?));
                }
                Wire::Map(pairs)
            }
        }
        Value::Instance(inst) => {
            let inst = inst.borrow();
            let mut fields = Vec::new();
            for (name, v) in inst.iter_fields() {
                fields.push((name.to_string(), any_to_wire(v, conn, depth + 1)?));
            }
            Wire::Record {
                class: inst.class.name.clone(),
                fields,
            }
        }
        v if as_proxy(v).is_some() => {
            let name = as_proxy(v).expect("just checked").iface.name.clone();
            object_to_wire(v, &name, conn)?
        }
        other => {
            return Err(format!(
                "{} belongs to this process and cannot be sent to another",
                crate::error::a_type(other.type_name())
            ));
        }
    })
}

fn table(t: TableObject) -> Value {
    Value::Table(Rc::new(RefCell::new(t)))
}

fn from_wire(w: Wire, ty: &Ty, conn: ConnId) -> Result<Value, String> {
    Ok(match (ty, w) {
        (_, Wire::Nil) => Value::Nil,
        (Ty::Opt(t), w) => return from_wire(w, t, conn),
        (Ty::Any, w) => return any_from_wire(w, conn),
        (_, Wire::Object(ObjRef::Local { key, .. })) => state(|s| s.exports.get(&key).map(|e| e.value.clone()))
            .ok_or("a reference to an object this process no longer has")?,
        (Ty::Iface(name), Wire::Object(ObjRef::Remote { id, .. })) => proxy(conn, id, iface(name)?),
        (Ty::Bool, Wire::Bool(b)) => Value::Bool(b),
        (Ty::Int, Wire::Int(i)) => Value::Int(i),
        (Ty::Float, Wire::Float(f)) => Value::Float(f),
        (Ty::Float, Wire::Int(i)) => Value::Float(i as f64),
        (Ty::Str, Wire::Str(s)) => Value::Str(SauleStr::new(s)),
        (Ty::Bytes, Wire::Bytes(b)) => Value::Bytes(SauleBytes::new(b)),
        (Ty::List(t), Wire::List(items)) => table(TableObject::from_array(
            items.into_iter().map(|w| from_wire(w, t, conn)).collect::<Result<_, _>>()?,
        )),
        (Ty::Map(k, vt), Wire::Map(pairs)) => {
            let mut t = TableObject::new();
            for (kw, vw) in pairs {
                let key = from_wire(kw, k, conn)?;
                let value = from_wire(vw, vt, conn)?;
                t.set(&key, value)?;
            }
            table(t)
        }
        (Ty::Data(name), Wire::Record { fields, .. }) => record(name, fields, conn)?,
        (ty, w) => return Err(format!("expected {ty:?}, but {w:?} arrived")),
    })
}

/// A data class arriving: an instance of the same class, its fields filled.
fn record(name: &str, fields: Vec<(String, Wire)>, conn: ConnId) -> Result<Value, String> {
    let class = state(|s| s.classes.get(name).cloned())
        .ok_or_else(|| format!("a `{name}` arrived, and this program has no class `{name}`"))?;
    let desc = state(|s| s.data.get(name).cloned());
    let mut inst = InstanceObject::new(class);
    for (f, w) in fields {
        let ty = desc
            .as_ref()
            .and_then(|d| d.fields.iter().find(|(n, _)| *n == f).map(|(_, t)| t.clone()))
            .unwrap_or(Ty::Any);
        let v = from_wire(w, &ty, conn).map_err(|e| format!("field `{f}` of `{name}`: {e}"))?;
        // A field this build of the class does not have is dropped: the
        // two sides may be different versions of the program.
        inst.set_field(&f, v);
    }
    Ok(Value::Instance(Rc::new(RefCell::new(inst))))
}

fn any_from_wire(w: Wire, conn: ConnId) -> Result<Value, String> {
    Ok(match w {
        Wire::Nil => Value::Nil,
        Wire::Bool(b) => Value::Bool(b),
        Wire::Int(i) => Value::Int(i),
        Wire::Float(f) => Value::Float(f),
        Wire::Str(s) => Value::Str(SauleStr::new(s)),
        Wire::Bytes(b) => Value::Bytes(SauleBytes::new(b)),
        Wire::List(items) => table(TableObject::from_array(
            items.into_iter().map(|w| any_from_wire(w, conn)).collect::<Result<_, _>>()?,
        )),
        Wire::Map(pairs) => {
            let mut t = TableObject::new();
            for (k, v) in pairs {
                let k = any_from_wire(k, conn)?;
                let v = any_from_wire(v, conn)?;
                t.set(&k, v)?;
            }
            table(t)
        }
        Wire::Record { class, fields } => {
            if state(|s| s.classes.contains_key(&class)) {
                record(&class, fields, conn)?
            } else {
                let mut t = TableObject::new();
                for (f, w) in fields {
                    t.set(&Value::Str(SauleStr::new(f)), any_from_wire(w, conn)?)?;
                }
                table(t)
            }
        }
        Wire::Object(ObjRef::Remote { id, interface }) => proxy(conn, id, iface(&interface)?),
        Wire::Object(ObjRef::Local { key, .. }) => state(|s| s.exports.get(&key).map(|e| e.value.clone()))
            .ok_or("a reference to an object this process no longer has")?,
        Wire::Fd(_) | Wire::Shm(..) => {
            return Err("a file descriptor arrived where a plain value was expected".into());
        }
    })
}

// --- Incoming calls ------------------------------------------------------------------

/// Answers calls to this process's exported objects with their Saule
/// methods.
pub(crate) struct Glue;

impl Handler for Glue {
    fn call(
        &mut self,
        looper: &mut Looper,
        from: ConnId,
        key: u64,
        method: &str,
        args: Vec<Wire>,
    ) -> Result<Vec<Wire>, String> {
        let (value, iface) = state(|s| s.exports.get(&key).map(|e| (e.value.clone(), e.iface.clone())))
            .ok_or("this object no longer exists")?;
        let m = iface
            .method(method)
            .ok_or_else(|| format!("`{}` has no method `{method}`", iface.name))?
            .clone();
        let mut values = Vec::with_capacity(m.params.len());
        let mut args = args.into_iter();
        for (i, ty) in m.params.iter().enumerate() {
            let w = args.next().unwrap_or(Wire::Nil);
            values.push(
                from_wire(w, ty, from)
                    .map_err(|e| format!("argument {} of `{}.{method}`: {e}", i + 1, iface.name))?,
            );
        }

        // Lend the looper back while the Saule method runs: it may call
        // other processes, or dispatch, and they look for it there.
        LOOPER.with(|l| *l.borrow_mut() = Some(std::mem::take(looper)));
        let outer = CALLER.replace(Some(from));
        let result = crate::call::call_method(&value, method, &values, 0..0);
        CALLER.set(outer);
        *looper = LOOPER.with(|l| l.borrow_mut().take()).unwrap_or_default();

        let results = result.map_err(|e| match e {
            // A `throw` in the method: its value is the message.
            crate::RuntimeError::Thrown { value, .. } => value,
            other => format!("`{}.{method}` failed: {other}", iface.name),
        })?;
        if m.is_async {
            return Ok(Vec::new());
        }
        m.returns
            .iter()
            .enumerate()
            .map(|(i, ty)| {
                to_wire(results.get(i).unwrap_or(&Value::Nil), ty, from)
                    .map_err(|e| format!("the result of `{}.{method}`: {e}", iface.name))
            })
            .collect()
    }

    fn disconnected(&mut self, _: &mut Looper, conn: ConnId) {
        state(|s| {
            s.proxies.retain(|&(c, _), _| c != conn);
            s.pending_release.retain(|&(c, _)| c != conn);
            let keys: Vec<u64> = s.exports.keys().copied().collect();
            for key in keys {
                if let Some(e) = s.exports.get_mut(&key) {
                    e.conns.remove(&conn);
                }
                drop_export_if_unused(s, key);
            }
        });
    }

    fn released(&mut self, _: &mut Looper, conn: ConnId, key: u64) {
        state(|s| {
            if let Some(e) = s.exports.get_mut(&key) {
                e.conns.remove(&conn);
            }
            drop_export_if_unused(s, key);
        });
    }
}

// --- What the language calls ---------------------------------------------------------

/// `Iface.connect(name)`: a proxy for the service `name`.
pub fn connect(description: &str, name: &str) -> Result<Value, String> {
    let iface = load(description)?;
    let conn = with_looper(|lp| lp.connect(name)).map_err(|e| thrown(e.to_string()))?;
    Ok(proxy(conn, ROOT, iface))
}

/// `Iface.publish(name, service)`: offer `service` as service `name`.
pub fn publish(description: &str, name: &str, service: &Value) -> Result<(), String> {
    let iface = load(description)?;
    let key = export(service, &iface.name, None, true)?;
    with_looper(|lp| lp.publish(name, key)).map_err(|e| thrown(e.to_string()))
}

/// `Looper.dispatch(timeout)`: answer whatever calls have arrived, waiting
/// up to `timeout` (`None`: until something does). How many were handled.
pub fn dispatch(timeout: Option<Duration>) -> Result<usize, String> {
    with_looper(|lp| lp.dispatch(&mut Glue, timeout)).map_err(|e| e.to_string())
}

/// `Looper.run()`: dispatch until `Looper.stop()`.
pub fn run() -> Result<(), String> {
    STOP.set(false);
    while !STOP.get() {
        dispatch(None)?;
    }
    Ok(())
}

/// `Looper.stop()`: make `run` return after the call being handled.
pub fn stop() {
    STOP.set(true);
}

/// The process id of whoever made the call being answered, as the kernel
/// reports it. `None` outside a call.
pub fn caller_pid() -> Option<u32> {
    let conn = CALLER.get()?;
    // The looper is lent out to exactly this Saule code, so it is in its
    // slot.
    LOOPER.with(|l| l.borrow().as_ref().and_then(|lp| lp.peer(conn)).and_then(|p| p.pid))
}
