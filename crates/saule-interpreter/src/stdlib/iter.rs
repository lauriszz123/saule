//! Iteration: the `Iterable` contracts and the `Iter` combinators.
//!
//! * `Iterable<T>` and `Iterable2<K, V>` — interface contracts recognised by
//!   `for ... in instance do ... end`. Each implementer's `iter()` returns
//!   the step closure that drives the loop.
//! * `Iter` — a static class of combinators over sequences: `map`, `filter`,
//!   `reduce`, and the rest.
//!
//! For ad-hoc iteration over a `table`, the loop itself accepts the table
//! directly (`for v in t do` / `for k, v in t do`); no helper is needed.
//!
//! ## Why `Iter` takes a `table`, and how the other sources reach it
//!
//! Every combinator here is **eager**: `table` in, `table` out. That is what
//! makes them typeable. `Iter.map<V, U>(t: table<V>, f: fn(V) -> U)` binds
//! `V` from the receiver, so the lambda's parameter is a real type inside its
//! body and the result is a real `table<U>` — none of which survives if the
//! source slot has to be widened to `any` to also admit closures.
//!
//! The other two source kinds arrive through [`iter_collect`] instead:
//!
//! ```saule
//! Iter.map(Iter.collect(step), f)          -- a step closure
//! Iter.map(Iter.collect(list.iter()), f)   -- anything Iterable
//! ```
//!
//! `Iterable<V>` cannot be written in a native signature at all — `Type` has
//! no generic-application form — but `iter()` is declared by the user's own
//! class, so `Iter.collect(list.iter())` is checked end to end from types the
//! typechecker already has. And because evaluation is eager, the drain that
//! `collect` performs is one the combinator would have done anyway; the call
//! only makes it visible.
//!
//! ## The split with `Table`
//!
//! `Table.*` mutates a table or answers a question about one — `insert`,
//! `remove`, `sort`, `reverse`, `clear`, `contains`, `keys`. `Iter.*` derives
//! a new sequence from an existing one and never writes to its argument.
//! Where both have a reasonable claim to a name, they differ in meaning and
//! are named apart: `Table.reverse` reverses in place and answers `nil`,
//! `Iter.reverse` returns a new table; `Table.indexOf(t, value)` searches for
//! a value, `Iter.findIndex(t, pred)` searches with a predicate.

use crate::fxhash::fxmap;
use std::cell::RefCell;
use std::rc::Rc;

use crate::env::Environment;
use crate::native_packages::NativePackage;
use crate::stdlib::{expect_arity, expect_min_arity};
use crate::value::{ClassObject, InterfaceObject, NativeClosure, TableObject, Value};

/// `import Iterable, Iterable2, Iter from "iter"`. Auto-prelude'd so the
/// `for … in … do` desugar can rely on the interface names always being in
/// scope, and so `Iter.map(…)` needs no import either.
pub static ITER_PACKAGE: NativePackage = NativePackage {
    name: "iter",
    version: saule_version::VERSION,
    install,
    exports: &["Iterable", "Iterable2", "Iter"],
    register_sigs,
    builtins: empty_builtins,
    auto_prelude: true,
};

fn empty_builtins() -> saule_semantic::builtins::Builtins {
    saule_semantic::builtins::Builtins::default()
}

pub fn install(env: &Rc<RefCell<Environment>>) {
    define_interface(env, "Iterable");
    define_interface(env, "Iterable2");
    define_iter_class(env);
}

fn define_iter_class(env: &Rc<RefCell<Environment>>) {
    let mut static_fields = fxmap();
    let mut add =
        |name: &str, qname: &'static str, f: fn(&[Value]) -> Result<Vec<Value>, String>| {
            static_fields.insert(name.to_string(), native_multi(qname, f));
        };
    add("collect", "Iter.collect", iter_collect);
    add("map", "Iter.map", iter_map);
    add("filter", "Iter.filter", iter_filter);
    add("reduce", "Iter.reduce", iter_reduce);
    add("forEach", "Iter.forEach", iter_for_each);
    add("find", "Iter.find", iter_find);
    add("findIndex", "Iter.findIndex", iter_find_index);
    add("any", "Iter.any", iter_any);
    add("all", "Iter.all", iter_all);
    add("count", "Iter.count", iter_count);
    add("take", "Iter.take", iter_take);
    add("skip", "Iter.skip", iter_skip);
    add("first", "Iter.first", iter_first);
    add("last", "Iter.last", iter_last);
    add("chunk", "Iter.chunk", iter_chunk);
    add("zipWith", "Iter.zipWith", iter_zip_with);
    add("flatten", "Iter.flatten", iter_flatten);
    add("reverse", "Iter.reverse", iter_reverse);
    add("unique", "Iter.unique", iter_unique);
    add("sortBy", "Iter.sortBy", iter_sort_by);
    add("groupBy", "Iter.groupBy", iter_group_by);

    let class = ClassObject {
        name: "Iter".to_string(),
        parent: None,
        field_defs: Vec::new(),
        // Statics only — a stdlib namespace class is never instantiated.
        layout: Default::default(),
        methods: Default::default(),
        static_fields: RefCell::new(static_fields),
        static_methods: Default::default(),
        constructor: None,
    };
    env.borrow_mut()
        .define("Iter".to_string(), Value::Class(Rc::new(class)));
}

/// Register native signatures for the typechecker (lazy, via `sigs::lookup`).
pub fn register_sigs() {
    use crate::stdlib::sigs::{register_g, t_function, t_named, t_nullable, t_table, t_table_map};
    use saule_ast::Type;

    let i = || t_named("integer");
    let b = || t_named("boolean");
    let nil = || t_named("nil");
    let v = || t_named("V");
    let u = || t_named("U");
    let k = || t_named("K");
    let a = || t_named("A");
    let table_v = || t_table(v());
    let pred = || t_function(vec![v()], b());

    // `collect<V>(step: fn() -> V?) -> table<V>` — drain a step closure into
    // a table. The entry point for every source that isn't already one:
    // `Iter.collect(list.iter())` for an `Iterable`, `Iter.collect(step)` for
    // a bare closure. See the module docs for why this is a call rather than
    // an overload of every combinator.
    register_g(
        "Iter.collect",
        vec!["V"],
        vec![t_function(vec![], t_nullable(v()))],
        vec![table_v()],
    );

    // ─── core ───────────────────────────────────────────────────────────
    register_g(
        "Iter.map",
        vec!["V", "U"],
        vec![table_v(), t_function(vec![v()], u())],
        vec![t_table(u())],
    );
    register_g(
        "Iter.filter",
        vec!["V"],
        vec![table_v(), pred()],
        vec![table_v()],
    );
    // `reduce<V, A>(t, init: A, step: fn(A, V) -> A) -> A`. The accumulator
    // comes before the callback so the callback stays last and can be written
    // as a trailing lambda.
    register_g(
        "Iter.reduce",
        vec!["V", "A"],
        vec![table_v(), a(), t_function(vec![a(), v()], a())],
        vec![a()],
    );
    register_g(
        "Iter.forEach",
        vec!["V"],
        vec![table_v(), t_function(vec![v()], nil())],
        vec![nil()],
    );

    // ─── search ─────────────────────────────────────────────────────────
    // `find` and `findIndex` are nullable: no match is an ordinary outcome.
    register_g(
        "Iter.find",
        vec!["V"],
        vec![table_v(), pred()],
        vec![t_nullable(v())],
    );
    register_g(
        "Iter.findIndex",
        vec!["V"],
        vec![table_v(), pred()],
        vec![t_nullable(i())],
    );
    register_g("Iter.any", vec!["V"], vec![table_v(), pred()], vec![b()]);
    register_g("Iter.all", vec!["V"], vec![table_v(), pred()], vec![b()]);
    register_g("Iter.count", vec!["V"], vec![table_v(), pred()], vec![i()]);

    // ─── slicing ────────────────────────────────────────────────────────
    register_g(
        "Iter.take",
        vec!["V"],
        vec![table_v(), i()],
        vec![table_v()],
    );
    register_g(
        "Iter.skip",
        vec!["V"],
        vec![table_v(), i()],
        vec![table_v()],
    );
    register_g(
        "Iter.first",
        vec!["V"],
        vec![table_v()],
        vec![t_nullable(v())],
    );
    register_g(
        "Iter.last",
        vec!["V"],
        vec![table_v()],
        vec![t_nullable(v())],
    );
    // `chunk<V>(t, size) -> table<table<V>>` — fixed-size groups, the last
    // one short when the length doesn't divide evenly.
    register_g(
        "Iter.chunk",
        vec!["V"],
        vec![table_v(), i()],
        vec![t_table(table_v())],
    );

    // ─── shaping ────────────────────────────────────────────────────────
    // `zipWith` rather than `zip`: an eager `zip` would have to return pairs,
    // and a pair has no representation here — a table holding one `integer`
    // and one `V` types as `table<any>`, which loses both. Combining the two
    // elements at the point they meet keeps every type intact.
    register_g(
        "Iter.zipWith",
        vec!["V", "U", "A"],
        vec![table_v(), t_table(u()), t_function(vec![v(), u()], a())],
        vec![t_table(a())],
    );
    register_g(
        "Iter.flatten",
        vec!["V"],
        vec![t_table(table_v())],
        vec![table_v()],
    );
    register_g("Iter.reverse", vec!["V"], vec![table_v()], vec![table_v()]);
    register_g("Iter.unique", vec!["V"], vec![table_v()], vec![table_v()]);
    // `sortBy<V, K>(t, key: fn(V) -> K) -> table<V>` — sorted ascending by
    // the extracted key using the language's own `<`, so a class that
    // implements `OpCompare` sorts by its own rule. For a bespoke ordering
    // reach for `Table.sort`, which takes the comparator directly.
    register_g(
        "Iter.sortBy",
        vec!["V", "K"],
        vec![table_v(), t_function(vec![v()], k())],
        vec![table_v()],
    );
    // `groupBy<V, K>(t, key: fn(V) -> K) -> table<K, table<V>>`.
    register_g(
        "Iter.groupBy",
        vec!["V", "K"],
        vec![table_v(), t_function(vec![v()], k())],
        vec![t_table_map(k(), table_v())],
    );
    let _ = Type::Named(String::new());
}

fn define_interface(env: &Rc<RefCell<Environment>>, name: &str) {
    let mut methods = fxmap();
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

fn native_multi(name: &'static str, func: fn(&[Value]) -> Result<Vec<Value>, String>) -> Value {
    Value::NativeClosure(Rc::new(NativeClosure {
        name,
        func: Box::new(func),
        param_names: Vec::new(),
    }))
}

// ─── helpers ────────────────────────────────────────────────────────────────

/// The array part of the table at `idx`, copied out.
///
/// Copied, not borrowed: every combinator here runs a Saule callback per
/// element, and that callback may read or write the very table it was called
/// about. Holding a `RefCell` borrow across it would turn an ordinary program
/// into a panic. The copy is of `Value`s — refcount bumps, not deep clones.
fn elements(name: &str, args: &[Value], idx: usize) -> Result<Vec<Value>, String> {
    match args.get(idx) {
        Some(Value::Table(t)) => Ok(t.borrow().array.clone()),
        Some(other) => Err(format!(
            "{name} expects a table at argument {}, got `{}`",
            idx + 1,
            other.type_name()
        )),
        None => Err(format!("{name} missing argument {}", idx + 1)),
    }
}

fn expect_int(name: &str, args: &[Value], idx: usize) -> Result<i64, String> {
    match args.get(idx) {
        Some(Value::Int(n)) => Ok(*n),
        Some(other) => Err(format!(
            "{name} expects an integer at argument {}, got `{}`",
            idx + 1,
            other.type_name()
        )),
        None => Err(format!("{name} missing argument {}", idx + 1)),
    }
}

fn table_value(items: Vec<Value>) -> Value {
    Value::Table(Rc::new(RefCell::new(TableObject::from_array(items))))
}

/// Call a Saule callable with `args` and take its first return value.
///
/// The dispatch mirrors `Table.sort`'s comparator path: a VM function is
/// invoked directly, natives are called as natives, and a tree-walker closure
/// goes through the argument machinery it needs. `what` names the combinator
/// so a failure inside the callback says which one was running.
fn call(f: &Value, args: Vec<Value>, what: &str) -> Result<Value, String> {
    use crate::eval::expr::{EvaluatedArg, call_value_multi};
    if !matches!(
        f,
        Value::Function(_) | Value::Native(_) | Value::NativeClosure(_) | Value::VmFunction(_)
    ) {
        return Err(format!(
            "{what}: expected a function, got `{}`",
            f.type_name()
        ));
    }
    let result = match f {
        Value::VmFunction(vf) => vf.invoke_first(&args, 0..0),
        Value::Native(nf) => {
            (nf.func)(&args).map_err(|message| crate::error::RuntimeError::TypeError {
                message,
                span: 0..0,
            })
        }
        Value::NativeClosure(nc) => (nc.func)(&args)
            .map(|vs| vs.into_iter().next().unwrap_or(Value::Nil))
            .map_err(|message| crate::error::RuntimeError::TypeError {
                message,
                span: 0..0,
            }),
        other => {
            let packed: Vec<EvaluatedArg> =
                args.into_iter().map(EvaluatedArg::Positional).collect();
            call_value_multi(other.clone(), &packed, 0..0)
                .map(|vs| vs.into_iter().next().unwrap_or(Value::Nil))
        }
    };
    result.map_err(|e| format!("{what}: {e}"))
}

/// `a == b` by the language's own rule, so `OpEq` is honoured.
fn eq(a: &Value, b: &Value, what: &str) -> Result<bool, String> {
    crate::eval::ops::binary(saule_ast::BinOp::Eq, a.clone(), b.clone(), 0..0)
        .map(|v| v.is_truthy())
        .map_err(|e| format!("{what}: comparison failed: {e}"))
}

/// `a < b` by the language's own rule, so `OpCompare` is honoured.
fn lt(a: &Value, b: &Value, what: &str) -> Result<bool, String> {
    crate::eval::ops::binary(saule_ast::BinOp::Lt, a.clone(), b.clone(), 0..0)
        .map(|v| v.is_truthy())
        .map_err(|e| format!("{what}: comparison failed: {e}"))
}

// ─── collect ────────────────────────────────────────────────────────────────

/// Drain a step closure until it answers `nil`.
///
/// The step-closure protocol is the one `for … in` already runs on and the
/// one `Iterable.iter()` returns, so this is the single adapter that brings
/// every non-table source into combinator range.
///
/// A closure that never returns `nil` never terminates — the same as writing
/// the `for` loop by hand. Eager evaluation cannot bound an unbounded source,
/// which is the one thing the lazy design would have bought.
fn iter_collect(args: &[Value]) -> Result<Vec<Value>, String> {
    expect_arity("Iter.collect", args, 1)?;
    let step = &args[0];
    let mut out: Vec<Value> = Vec::new();
    loop {
        match call(step, Vec::new(), "Iter.collect")? {
            Value::Nil => break,
            v => out.push(v),
        }
    }
    Ok(vec![table_value(out)])
}

// ─── core ───────────────────────────────────────────────────────────────────

fn iter_map(args: &[Value]) -> Result<Vec<Value>, String> {
    expect_arity("Iter.map", args, 2)?;
    let src = elements("Iter.map", args, 0)?;
    let mut out = Vec::with_capacity(src.len());
    for element in src {
        out.push(call(&args[1], vec![element], "Iter.map")?);
    }
    Ok(vec![table_value(out)])
}

fn iter_filter(args: &[Value]) -> Result<Vec<Value>, String> {
    expect_arity("Iter.filter", args, 2)?;
    let src = elements("Iter.filter", args, 0)?;
    let mut out = Vec::new();
    for element in src {
        if call(&args[1], vec![element.clone()], "Iter.filter")?.is_truthy() {
            out.push(element);
        }
    }
    Ok(vec![table_value(out)])
}

fn iter_reduce(args: &[Value]) -> Result<Vec<Value>, String> {
    expect_arity("Iter.reduce", args, 3)?;
    let src = elements("Iter.reduce", args, 0)?;
    let mut acc = args[1].clone();
    for element in src {
        acc = call(&args[2], vec![acc, element], "Iter.reduce")?;
    }
    Ok(vec![acc])
}

fn iter_for_each(args: &[Value]) -> Result<Vec<Value>, String> {
    expect_arity("Iter.forEach", args, 2)?;
    let src = elements("Iter.forEach", args, 0)?;
    for element in src {
        call(&args[1], vec![element], "Iter.forEach")?;
    }
    Ok(vec![Value::Nil])
}

// ─── search ─────────────────────────────────────────────────────────────────

fn iter_find(args: &[Value]) -> Result<Vec<Value>, String> {
    expect_arity("Iter.find", args, 2)?;
    let src = elements("Iter.find", args, 0)?;
    for element in src {
        if call(&args[1], vec![element.clone()], "Iter.find")?.is_truthy() {
            return Ok(vec![element]);
        }
    }
    Ok(vec![Value::Nil])
}

fn iter_find_index(args: &[Value]) -> Result<Vec<Value>, String> {
    expect_arity("Iter.findIndex", args, 2)?;
    let src = elements("Iter.findIndex", args, 0)?;
    for (i, element) in src.into_iter().enumerate() {
        if call(&args[1], vec![element], "Iter.findIndex")?.is_truthy() {
            return Ok(vec![Value::Int(i as i64 + 1)]);
        }
    }
    Ok(vec![Value::Nil])
}

/// `any` on an empty table is `false`, `all` on an empty table is `true`.
///
/// The vacuous-truth convention, which is what makes them compose: `all` over
/// a filtered-to-nothing list should not report a violation it never saw.
fn iter_any(args: &[Value]) -> Result<Vec<Value>, String> {
    expect_arity("Iter.any", args, 2)?;
    let src = elements("Iter.any", args, 0)?;
    for element in src {
        if call(&args[1], vec![element], "Iter.any")?.is_truthy() {
            return Ok(vec![Value::Bool(true)]);
        }
    }
    Ok(vec![Value::Bool(false)])
}

fn iter_all(args: &[Value]) -> Result<Vec<Value>, String> {
    expect_arity("Iter.all", args, 2)?;
    let src = elements("Iter.all", args, 0)?;
    for element in src {
        if !call(&args[1], vec![element], "Iter.all")?.is_truthy() {
            return Ok(vec![Value::Bool(false)]);
        }
    }
    Ok(vec![Value::Bool(true)])
}

fn iter_count(args: &[Value]) -> Result<Vec<Value>, String> {
    expect_arity("Iter.count", args, 2)?;
    let src = elements("Iter.count", args, 0)?;
    let mut n: i64 = 0;
    for element in src {
        if call(&args[1], vec![element], "Iter.count")?.is_truthy() {
            n += 1;
        }
    }
    Ok(vec![Value::Int(n)])
}

// ─── slicing ────────────────────────────────────────────────────────────────

/// `take`/`skip` clamp rather than error. Asking for more than there is has
/// an obvious answer — all of it, and nothing, respectively — and a program
/// paging through a list should not have to check the length first.
fn iter_take(args: &[Value]) -> Result<Vec<Value>, String> {
    expect_arity("Iter.take", args, 2)?;
    let src = elements("Iter.take", args, 0)?;
    let n = expect_int("Iter.take", args, 1)?.max(0) as usize;
    Ok(vec![table_value(src.into_iter().take(n).collect())])
}

fn iter_skip(args: &[Value]) -> Result<Vec<Value>, String> {
    expect_arity("Iter.skip", args, 2)?;
    let src = elements("Iter.skip", args, 0)?;
    let n = expect_int("Iter.skip", args, 1)?.max(0) as usize;
    Ok(vec![table_value(src.into_iter().skip(n).collect())])
}

fn iter_first(args: &[Value]) -> Result<Vec<Value>, String> {
    expect_min_arity("Iter.first", args, 1)?;
    let src = elements("Iter.first", args, 0)?;
    Ok(vec![src.into_iter().next().unwrap_or(Value::Nil)])
}

fn iter_last(args: &[Value]) -> Result<Vec<Value>, String> {
    expect_min_arity("Iter.last", args, 1)?;
    let src = elements("Iter.last", args, 0)?;
    Ok(vec![src.into_iter().next_back().unwrap_or(Value::Nil)])
}

fn iter_chunk(args: &[Value]) -> Result<Vec<Value>, String> {
    expect_arity("Iter.chunk", args, 2)?;
    let src = elements("Iter.chunk", args, 0)?;
    let size = expect_int("Iter.chunk", args, 1)?;
    if size < 1 {
        return Err(format!("Iter.chunk: size must be at least 1, got {size}"));
    }
    let groups: Vec<Value> = src
        .chunks(size as usize)
        .map(|c| table_value(c.to_vec()))
        .collect();
    Ok(vec![table_value(groups)])
}

// ─── shaping ────────────────────────────────────────────────────────────────

/// Stops at the shorter of the two, which is what makes `zipWith` total:
/// there is no element to pair the surplus with, and inventing a `nil` would
/// hand the callback a value its parameter type forbids.
fn iter_zip_with(args: &[Value]) -> Result<Vec<Value>, String> {
    expect_arity("Iter.zipWith", args, 3)?;
    let left = elements("Iter.zipWith", args, 0)?;
    let right = elements("Iter.zipWith", args, 1)?;
    let mut out = Vec::with_capacity(left.len().min(right.len()));
    for (a, b) in left.into_iter().zip(right) {
        out.push(call(&args[2], vec![a, b], "Iter.zipWith")?);
    }
    Ok(vec![table_value(out)])
}

/// One level only. Flattening recursively would need a runtime type test per
/// element and would quietly do something different depending on the data;
/// `Iter.flatten(Iter.flatten(t))` says how deep to go.
fn iter_flatten(args: &[Value]) -> Result<Vec<Value>, String> {
    expect_min_arity("Iter.flatten", args, 1)?;
    let src = elements("Iter.flatten", args, 0)?;
    let mut out = Vec::new();
    for (i, group) in src.iter().enumerate() {
        match group {
            Value::Table(t) => out.extend(t.borrow().array.iter().cloned()),
            other => {
                return Err(format!(
                    "Iter.flatten: element {} is a `{}`, not a table",
                    i + 1,
                    other.type_name()
                ));
            }
        }
    }
    Ok(vec![table_value(out)])
}

fn iter_reverse(args: &[Value]) -> Result<Vec<Value>, String> {
    expect_min_arity("Iter.reverse", args, 1)?;
    let mut src = elements("Iter.reverse", args, 0)?;
    src.reverse();
    Ok(vec![table_value(src)])
}

/// First occurrence wins, original order preserved.
///
/// Quadratic, because `==` is the language's — an `OpEq` overload is a method
/// call, not a hash. For the sizes this is reached for that is the right
/// trade; a caller deduplicating a large list of plain values can sort first
/// and scan.
fn iter_unique(args: &[Value]) -> Result<Vec<Value>, String> {
    expect_min_arity("Iter.unique", args, 1)?;
    let src = elements("Iter.unique", args, 0)?;
    let mut out: Vec<Value> = Vec::new();
    'next: for element in src {
        for seen in &out {
            if eq(seen, &element, "Iter.unique")? {
                continue 'next;
            }
        }
        out.push(element);
    }
    Ok(vec![table_value(out)])
}

/// Sort ascending by an extracted key.
///
/// The key is computed once per element rather than on every comparison — a
/// Schwartzian transform, which matters here because the extractor is a
/// Saule-level call and a sort makes O(n log n) comparisons.
///
/// The sort is stable: elements with equal keys keep their input order.
fn iter_sort_by(args: &[Value]) -> Result<Vec<Value>, String> {
    expect_arity("Iter.sortBy", args, 2)?;
    let src = elements("Iter.sortBy", args, 0)?;
    let mut keyed: Vec<(Value, Value)> = Vec::with_capacity(src.len());
    for element in src {
        let key = call(&args[1], vec![element.clone()], "Iter.sortBy")?;
        keyed.push((key, element));
    }

    // Bottom-up merge sort, for the same reason `Table.sort` uses one: the
    // comparison can fail (it may run an `OpCompare` method), and `sort_by`
    // has nowhere to put an error. Taking the left element on a tie is what
    // makes it stable.
    let n = keyed.len();
    let mut buf: Vec<(Value, Value)> = Vec::with_capacity(n);
    let mut width = 1;
    while width < n {
        let mut lo = 0;
        while lo < n {
            let mid = (lo + width).min(n);
            let hi = (lo + 2 * width).min(n);
            if mid < hi {
                buf.clear();
                let (mut i, mut j) = (lo, mid);
                while i < mid && j < hi {
                    if lt(&keyed[j].0, &keyed[i].0, "Iter.sortBy")? {
                        buf.push(keyed[j].clone());
                        j += 1;
                    } else {
                        buf.push(keyed[i].clone());
                        i += 1;
                    }
                }
                buf.extend_from_slice(&keyed[i..mid]);
                buf.extend_from_slice(&keyed[j..hi]);
                keyed[lo..hi].clone_from_slice(&buf);
            }
            lo += 2 * width;
        }
        width *= 2;
    }

    Ok(vec![table_value(
        keyed.into_iter().map(|(_, element)| element).collect(),
    )])
}

/// Group by an extracted key into `table<K, table<V>>`.
///
/// The key must be a `string`, `integer` or `boolean` — those are the types a
/// table can be keyed by. A key of any other type is reported at the element
/// that produced it, which is more use than a bare "invalid key".
fn iter_group_by(args: &[Value]) -> Result<Vec<Value>, String> {
    expect_arity("Iter.groupBy", args, 2)?;
    let src = elements("Iter.groupBy", args, 0)?;
    let out = Rc::new(RefCell::new(TableObject::new()));
    for (i, element) in src.into_iter().enumerate() {
        let key = call(&args[1], vec![element.clone()], "Iter.groupBy")?;
        if !matches!(key, Value::Str(_) | Value::Int(_) | Value::Bool(_)) {
            return Err(format!(
                "Iter.groupBy: the key for element {} is a `{}`; a table can only be keyed by \
                 `string`, `integer` or `boolean`",
                i + 1,
                key.type_name()
            ));
        }
        // Append into the bucket, creating it on first sight. The bucket is
        // fetched and released before the element goes in so the borrow never
        // spans another table operation.
        let bucket = out.borrow().get(&key);
        let bucket = match bucket {
            Value::Table(t) => t,
            _ => {
                let fresh = Rc::new(RefCell::new(TableObject::new()));
                out.borrow_mut()
                    .set(&key, Value::Table(fresh.clone()))
                    .map_err(|e| format!("Iter.groupBy: {e}"))?;
                fresh
            }
        };
        bucket.borrow_mut().array.push(element);
    }
    Ok(vec![Value::Table(out)])
}
