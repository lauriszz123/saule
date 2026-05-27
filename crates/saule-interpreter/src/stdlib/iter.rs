//! Built-in iteration interfaces and table iterators.
//!
//! * `Iterable<T>` and `Iterable2<K, V>` — interface contracts recognised by
//!   `for ... in instance do ... end`. Each implementer's `iter()` returns
//!   the step closure that drives the loop.
//! * `pairs(t)` — generic iterator over a table yielding `(key, value)`
//!   pairs (array part first, then map part in deterministic order).
//! * `ipairs(t)` — array-only iterator yielding `(index, value)` pairs.
//!
//! Both helpers return a `Value::NativeClosure` step function the loop drives
//! through the same multi-return protocol as user-written iterators.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::env::Environment;
use crate::value::{InterfaceObject, NativeClosure, TableKey, Value};

pub fn install(env: &Rc<RefCell<Environment>>) {
    define_interface(env, "Iterable");
    define_interface(env, "Iterable2");

    define_helper(env, "pairs",  make_pairs);
    define_helper(env, "ipairs", make_ipairs);
}

/// Register native signatures for the typechecker (lazy, via `sigs::lookup`).
pub fn register_sigs() {
    use crate::stdlib::sigs::{register, t_named};
    let any = || t_named("any");
    // `pairs(t)` / `ipairs(t)` each return a step closure — modelled as `any`
    // for now since the value types are erased once you go through them.
    register("pairs",  vec![any()], vec![any()]);
    register("ipairs", vec![any()], vec![any()]);
}

fn define_interface(env: &Rc<RefCell<Environment>>, name: &str) {
    let mut methods = HashMap::new();
    // `iter()` — zero parameters, has a return type.
    methods.insert("iter".to_string(), (0, true));
    env.borrow_mut().define(
        name.to_string(),
        Value::Interface(Rc::new(InterfaceObject {
            name: name.to_string(),
            extends: Vec::new(),
            methods,
        })),
    );
}

fn define_helper(
    env: &Rc<RefCell<Environment>>,
    name: &'static str,
    f: fn(&[Value]) -> Result<Vec<Value>, String>,
) {
    env.borrow_mut().define(
        name.to_string(),
        Value::NativeClosure(Rc::new(NativeClosure {
            name,
            func: Box::new(move |args| f(args)),
        })),
    );
}

// ─── pairs ──────────────────────────────────────────────────────────────────

fn make_pairs(args: &[Value]) -> Result<Vec<Value>, String> {
    let table = match args.first() {
        Some(Value::Table(t)) => t.clone(),
        Some(other) => {
            return Err(format!(
                "pairs expects a table, got `{}`",
                other.type_name()
            ));
        }
        None => return Err("pairs missing argument".to_string()),
    };

    // Snapshot every (key, value) pair up front so the loop body can safely
    // mutate the underlying table without invalidating iteration.
    let entries: Rc<Vec<(Value, Value)>> = Rc::new(snapshot_pairs(&table.borrow()));
    let cursor = Rc::new(RefCell::new(0usize));
    let entries_for_closure = entries.clone();
    Ok(vec![Value::NativeClosure(Rc::new(NativeClosure {
        name: "pairs#step",
        func: Box::new(move |_| {
            let mut i = cursor.borrow_mut();
            if *i >= entries_for_closure.len() {
                return Ok(vec![Value::Nil, Value::Nil]);
            }
            let (k, v) = entries_for_closure[*i].clone();
            *i += 1;
            Ok(vec![k, v])
        }),
    }))])
}

fn snapshot_pairs(t: &crate::value::TableObject) -> Vec<(Value, Value)> {
    let mut out: Vec<(Value, Value)> = Vec::with_capacity(t.array.len() + t.map.len());

    // Array part — 1-based integer keys.
    for (i, v) in t.array.iter().enumerate() {
        out.push((Value::Int((i + 1) as i64), v.clone()));
    }

    // Map part — deterministic order: ints ascending, then strings, then bools.
    let mut map_entries: Vec<(&TableKey, &Value)> = t.map.iter().collect();
    map_entries.sort_by(|a, b| match (a.0, b.0) {
        (TableKey::Int(x), TableKey::Int(y)) => x.cmp(y),
        (TableKey::Int(_), _) => std::cmp::Ordering::Less,
        (_, TableKey::Int(_)) => std::cmp::Ordering::Greater,
        (TableKey::Str(x), TableKey::Str(y)) => x.cmp(y),
        (TableKey::Str(_), _) => std::cmp::Ordering::Less,
        (_, TableKey::Str(_)) => std::cmp::Ordering::Greater,
        (TableKey::Bool(x), TableKey::Bool(y)) => x.cmp(y),
    });
    for (k, v) in map_entries {
        out.push((k.to_value(), v.clone()));
    }
    out
}

// ─── ipairs ─────────────────────────────────────────────────────────────────

fn make_ipairs(args: &[Value]) -> Result<Vec<Value>, String> {
    let table = match args.first() {
        Some(Value::Table(t)) => t.clone(),
        Some(other) => {
            return Err(format!(
                "ipairs expects a table, got `{}`",
                other.type_name()
            ));
        }
        None => return Err("ipairs missing argument".to_string()),
    };

    // Snapshot the array part. ipairs ignores the map part by design.
    let array: Rc<Vec<Value>> = Rc::new(table.borrow().array.clone());
    let cursor = Rc::new(RefCell::new(0usize));
    let array_for_closure = array.clone();
    Ok(vec![Value::NativeClosure(Rc::new(NativeClosure {
        name: "ipairs#step",
        func: Box::new(move |_| {
            let mut i = cursor.borrow_mut();
            if *i >= array_for_closure.len() {
                return Ok(vec![Value::Nil, Value::Nil]);
            }
            let v = array_for_closure[*i].clone();
            let idx = (*i + 1) as i64;
            *i += 1;
            Ok(vec![Value::Int(idx), v])
        }),
    }))])
}

