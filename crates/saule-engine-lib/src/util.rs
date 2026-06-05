//! `Util` — a small demo module exercising the `STable` / `SFunction` bridge.
//!
//! Unlike the graphics/window/timer modules (which only pass scalars across
//! the ABI), these functions accept and return *reference* values — host-owned
//! `table`s and callables — to show that the handle bridge works end to end.

use saule_sdk::prelude::*;

/// `Util.sum(t)` — add up the array part of a table of integers. The typed
/// `STable<SInteger>` parameter renders as `table<integer>`, so the Saule type
/// checker rejects a call passing a table of any other element type.
#[saule_export(class = "Util", name = "sum")]
fn util_sum(t: STable<SInteger>) -> Result<SInteger, String> {
    let mut total = 0i64;
    for v in t.to_vec()? {
        match v {
            SValue::Int(i) => total += i,
            SValue::Float(f) => total += f as i64,
            other => return Err(format!("Util.sum: expected numbers, got {other:?}")),
        }
    }
    Ok(SInteger::new(total))
}

/// `Util.keys(t)` — return a new array-table of the table's keys.
#[saule_export(class = "Util", name = "keys")]
fn util_keys(t: STable) -> Result<STable, String> {
    t.keys()
}

/// `Util.map(t, f)` — apply `f` to every array element, collecting the
/// results into a fresh table.
#[saule_export(class = "Util", name = "map")]
fn util_map(t: STable, f: SFunction) -> Result<STable, String> {
    let out = STable::new();
    for v in t.to_vec()? {
        let mapped = f.call(&[v])?;
        out.push(mapped)?;
    }
    Ok(out)
}

/// `Util.range(n)` — build a fresh table `[1, 2, …, n]` on the host.
#[saule_export(class = "Util", name = "range")]
fn util_range(n: SInteger) -> Result<STable<SInteger>, String> {
    let t = STable::new();
    for i in 1..=n.to_i64() {
        t.push(i)?;
    }
    Ok(t)
}

/// `Util.filter(t, f)` — keep array elements for which `f(element)` is truthy.
/// Generic in the element type: `filter(table<T>, function) -> table<T>`.
#[saule_export(class = "Util", name = "filter")]
fn util_filter(t: STable<T>, f: SFunction) -> Result<STable<T>, String> {
    let out = STable::new();
    for v in t.to_vec()? {
        if is_truthy(&f.call(&[v.clone()])?) {
            out.push(v)?;
        }
    }
    Ok(out)
}

/// `Util.reduce(t, f, init)` — left-fold the array part: `acc = f(acc, element)`
/// starting from `init`, returning the final accumulator. The accumulator type
/// `U` is threaded from `init` to the result: `reduce(table<T>, function, U) -> U`.
#[saule_export(class = "Util", name = "reduce")]
fn util_reduce(t: STable<T>, f: SFunction, init: SElem<U>) -> Result<SElem<U>, String> {
    let mut acc = init.into_value();
    for v in t.to_vec()? {
        acc = f.call(&[acc, v])?;
    }
    Ok(SElem::new(acc))
}

/// `Util.find(t, f)` — the first array element for which `f(element)` is
/// truthy, or `nil` if none match. Returns the element type:
/// `find(table<T>, function) -> T?`.
#[saule_export(class = "Util", name = "find")]
fn util_find(t: STable<T>, f: SFunction) -> Result<Option<SElem<T>>, String> {
    for v in t.to_vec()? {
        if is_truthy(&f.call(&[v.clone()])?) {
            return Ok(Some(SElem::new(v)));
        }
    }
    Ok(None)
}

/// `Util.forEach(t, f)` — call `f(element)` for every array element, ignoring
/// the results. Returns `nil`.
#[saule_export(class = "Util", name = "forEach")]
fn util_for_each(t: STable, f: SFunction) -> Result<(), String> {
    for v in t.to_vec()? {
        f.call(&[v])?;
    }
    Ok(())
}

/// `Util.count(t, f)` — how many array elements satisfy the predicate `f`.
#[saule_export(class = "Util", name = "count")]
fn util_count(t: STable, f: SFunction) -> Result<SInteger, String> {
    let mut n = 0i64;
    for v in t.to_vec()? {
        if is_truthy(&f.call(&[v])?) {
            n += 1;
        }
    }
    Ok(SInteger::new(n))
}

/// `Util.reverse(t)` — a new table with the array part in reverse order.
/// Preserves the element type: `reverse(table<T>) -> table<T>`.
#[saule_export(class = "Util", name = "reverse")]
fn util_reverse(t: STable<T>) -> Result<STable<T>, String> {
    let out = STable::new();
    for v in t.to_vec()?.into_iter().rev() {
        out.push(v)?;
    }
    Ok(out)
}

/// `Util.concat(a, b)` — a new table holding `a`'s array elements followed by
/// `b`'s. Both inputs share the element type: `concat(table<T>, table<T>) -> table<T>`.
#[saule_export(class = "Util", name = "concat")]
fn util_concat(a: STable<T>, b: STable<T>) -> Result<STable<T>, String> {
    let out = STable::new();
    for v in a.to_vec()? {
        out.push(v)?;
    }
    for v in b.to_vec()? {
        out.push(v)?;
    }
    Ok(out)
}

/// `Util.slice(t, start, len)` — a new table with up to `len` elements taken
/// from `t` beginning at the 1-based index `start`. Preserves the element
/// type: `slice(table<T>, integer, integer) -> table<T>`.
#[saule_export(class = "Util", name = "slice")]
fn util_slice(t: STable<T>, start: SInteger, len: SInteger) -> Result<STable<T>, String> {
    let out = STable::new();
    let skip = (start.to_i64().max(1) - 1) as usize;
    let take = len.to_i64().max(0) as usize;
    for v in t.to_vec()?.into_iter().skip(skip).take(take) {
        out.push(v)?;
    }
    Ok(out)
}

/// `Util.contains(t, value)` — whether any array element equals `value`. The
/// needle must match the element type: `contains(table<T>, T) -> boolean`.
#[saule_export(class = "Util", name = "contains")]
fn util_contains(t: STable<T>, value: SElem<T>) -> Result<SBool, String> {
    for v in t.to_vec()? {
        if values_equal(&v, value.value()) {
            return Ok(SBool::new(true));
        }
    }
    Ok(SBool::new(false))
}

/// `Util.indexOf(t, value)` — the 1-based index of the first array element
/// equal to `value`, or `0` if it is absent. The needle must match the
/// element type: `indexOf(table<T>, T) -> integer`.
#[saule_export(class = "Util", name = "indexOf")]
fn util_index_of(t: STable<T>, value: SElem<T>) -> Result<SInteger, String> {
    for (i, v) in t.to_vec()?.iter().enumerate() {
        if values_equal(v, value.value()) {
            return Ok(SInteger::new(i as i64 + 1));
        }
    }
    Ok(SInteger::new(0))
}

/// Saule truthiness: only `nil` and `false` are falsy.
fn is_truthy(v: &SValue) -> bool {
    match v {
        SValue::Nil => false,
        SValue::Bool(b) => *b,
        _ => true,
    }
}

/// Structural value equality. Numbers compare across `integer`/`float`;
/// tables and functions compare by identity (handle).
fn values_equal(a: &SValue, b: &SValue) -> bool {
    match (a, b) {
        (SValue::Nil, SValue::Nil) => true,
        (SValue::Bool(x), SValue::Bool(y)) => x == y,
        (SValue::Int(x), SValue::Int(y)) => x == y,
        (SValue::Float(x), SValue::Float(y)) => x == y,
        (SValue::Int(x), SValue::Float(y)) | (SValue::Float(y), SValue::Int(x)) => *x as f64 == *y,
        (SValue::Str(x), SValue::Str(y)) => x == y,
        (SValue::Table(x), SValue::Table(y)) => x.handle() == y.handle(),
        (SValue::Func(x), SValue::Func(y)) => x.handle() == y.handle(),
        _ => false,
    }
}
