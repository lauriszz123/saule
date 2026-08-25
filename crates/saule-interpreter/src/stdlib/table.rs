//! `Table` static class — array-part helpers.
//!
//! All functions operate on the array side of a `table` value. The shapes
//! follow Saule's strict-typing aesthetic — the optional `pos` parameter
//! always comes *last* (unlike Lua's `table.insert(list, pos, value)`).
//!
//! * `Table.insert(list, value, pos?)` — append, or insert-at-position.
//! * `Table.remove(list, pos?)`        — remove last, or remove-at-position; returns the removed value (nullable).
//! * `Table.sort(list, comp)`           — in-place sort using the user comparator.
//! * `Table.concat(list, sep?, i?, j?)` — join array elements with a separator.
//!
//! Overloading is done by arg count (Lua-style) rather than by named
//! parameters, because native callables don't yet route named args. The
//! ergonomic equivalents are preserved:
//!
//!   `Table.insert(list, x)`        -- append
//!   `Table.insert(list, x, 1)`     -- prepend
//!   `Table.remove(list)`           -- pop last
//!   `Table.remove(list, 1)`        -- shift first
//!   `Table.concat(list)`           -- join with ""
//!   `Table.concat(list, ", ")`     -- join with separator
//!   `Table.concat(list, ", ", 2)`  -- from index 2 to end
//!   `Table.concat(list, ", ", 2, 4)` -- range

use crate::fxhash::fxmap;
use std::cell::RefCell;
use std::rc::Rc;

use crate::env::Environment;
use crate::native_packages::NativePackage;
use crate::stdlib::expect_min_arity;
use crate::value::{ClassObject, NativeClosure, TableObject, Value};
use crate::value::SauleStr;

/// `import Table from "table"`. Auto-prelude'd so bare `Table.insert(…)`
/// also works.
pub static TABLE_PACKAGE: NativePackage = NativePackage {
    name: "table",
    version: saule_version::VERSION,
    install,
    exports: &["Table"],
    register_sigs,
    builtins: empty_builtins,
    auto_prelude: true,
};

fn empty_builtins() -> saule_semantic::builtins::Builtins {
    saule_semantic::builtins::Builtins::default()
}

pub fn install(env: &Rc<RefCell<Environment>>) {
    let mut static_fields = fxmap();
    static_fields.insert(
        "insert".to_string(),
        native_multi("Table.insert", tbl_insert),
    );
    static_fields.insert(
        "remove".to_string(),
        native_multi("Table.remove", tbl_remove),
    );
    static_fields.insert("sort".to_string(), native_multi("Table.sort", tbl_sort));
    static_fields.insert(
        "concat".to_string(),
        native_multi("Table.concat", tbl_concat),
    );

    let class = ClassObject {
        name: "Table".to_string(),
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
        .define("Table".to_string(), Value::Class(Rc::new(class)));
}

/// Register native signatures for the typechecker (lazy, via `sigs::lookup`).
pub fn register_sigs() {
    use crate::stdlib::sigs::{register, register_g, t_any, t_named, t_nullable};
    use saule_ast::Type;
    let any = || t_any();
    let i = || t_named("integer");
    let s = || t_named("string");
    let nil = || t_named("nil");
    // First arg of every Table.* function must be a table.
    let table_any = || Type::Table {
        key: None,
        value: Box::new(t_any()),
    };
    // `table<V>` — the element-typed table used by the generic `Table.*`
    // sigs below. `V` is unified against the actual receiver's element type
    // so e.g. `Table.insert(self.storage, "")` rejects `string` when
    // `self.storage: table<Entry>`.
    let table_v = || Type::Table {
        key: None,
        value: Box::new(t_named("V")),
    };

    // `Table.insert<V>(list: table<V>, value: V, pos: integer?)` — appends,
    // or inserts at `pos`. Generic so the element type is enforced.
    register_g(
        "Table.insert",
        vec!["V"],
        vec![table_v(), t_named("V"), t_nullable(i())],
        vec![nil()],
    );
    // `Table.remove<V>(list: table<V>, pos: integer?) -> V?` — removes and
    // returns the element, or `nil` when the slot is empty / out of range.
    register_g(
        "Table.remove",
        vec!["V"],
        vec![table_v(), t_nullable(i())],
        vec![t_nullable(t_named("V"))],
    );
    // `Table.sort<V>(list: table<V>, cmp: fn(V, V) -> boolean) -> nil` —
    // generic so the comparator's parameter types are tied to the table's
    // element type. (Lambda parameter inference is still a separate work
    // item; the binding at least propagates `V` from the receiver.)
    use crate::stdlib::sigs::t_function;
    register_g(
        "Table.sort",
        vec!["V"],
        vec![
            table_v(),
            t_function(vec![t_named("V"), t_named("V")], t_named("boolean")),
        ],
        vec![nil()],
    );
    // `Table.concat(list: table<string>, sep?, from?, to?) -> string` —
    // element type is fixed: only string tables can be joined directly.
    // (Numeric tables must be mapped through `tostring` first.)
    register(
        "Table.concat",
        vec![
            Type::Table {
                key: None,
                value: Box::new(s()),
            },
            t_nullable(s()),
            t_nullable(i()),
            t_nullable(i()),
        ],
        vec![s()],
    );
    let _ = (any, table_any);
}

fn native_multi(name: &'static str, func: fn(&[Value]) -> Result<Vec<Value>, String>) -> Value {
    Value::NativeClosure(Rc::new(NativeClosure {
        name,
        func: Box::new(func),
        param_names: Vec::new(),
    }))
}

// ─── helpers ────────────────────────────────────────────────────────────────

fn expect_table(
    name: &str,
    args: &[Value],
    idx: usize,
) -> Result<Rc<RefCell<TableObject>>, String> {
    match args.get(idx) {
        Some(Value::Table(t)) => Ok(t.clone()),
        Some(other) => Err(format!(
            "{name} expects a table at argument {}, got `{}`",
            idx + 1,
            other.type_name()
        )),
        None => Err(format!("{name} missing argument {}", idx + 1)),
    }
}

fn expect_int_arg(name: &str, args: &[Value], idx: usize) -> Result<i64, String> {
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

fn expect_string_arg(name: &str, args: &[Value], idx: usize) -> Result<String, String> {
    match args.get(idx) {
        Some(Value::Str(s)) => Ok((**s).clone()),
        Some(other) => Err(format!(
            "{name} expects a string at argument {}, got `{}`",
            idx + 1,
            other.type_name()
        )),
        None => Err(format!("{name} missing argument {}", idx + 1)),
    }
}

// ─── Table.insert ────────────────────────────────────────────────────────────
//
// Two arities (Saule convention: optional `pos` comes *last*):
//   Table.insert(list, value)         -- append
//   Table.insert(list, value, pos)    -- shift right and insert at pos (1-based)

fn tbl_insert(args: &[Value]) -> Result<Vec<Value>, String> {
    expect_min_arity("Table.insert", args, 2)?;
    let table = expect_table("Table.insert", args, 0)?;

    match args.len() {
        2 => {
            let value = args[1].clone();
            table.borrow_mut().array.push(value);
            Ok(vec![Value::Nil])
        }
        3 => {
            let value = args[1].clone();
            let pos = expect_int_arg("Table.insert", args, 2)?;
            let mut t = table.borrow_mut();
            let len = t.array.len() as i64;
            if pos < 1 || pos > len + 1 {
                return Err(format!(
                    "Table.insert: position {pos} out of range for length {len}"
                ));
            }
            t.array.insert((pos - 1) as usize, value);
            Ok(vec![Value::Nil])
        }
        n => Err(format!("Table.insert expects 2 or 3 arguments, got {n}")),
    }
}

// ─── Table.remove ────────────────────────────────────────────────────────────
//
//   Table.remove(list)        -- remove + return the last element (or nil if empty)
//   Table.remove(list, pos)   -- remove + return the element at pos (or nil if oor)

fn tbl_remove(args: &[Value]) -> Result<Vec<Value>, String> {
    expect_min_arity("Table.remove", args, 1)?;
    let table = expect_table("Table.remove", args, 0)?;

    // Two flavours, dispatched by the type of the second argument:
    //   Table.remove(t)             -> pop last from the array
    //   Table.remove(t, n: integer) -> remove array slot n
    //   Table.remove(t, k: string | boolean) -> delete a map entry
    if args.len() >= 2 && !matches!(&args[1], Value::Int(_) | Value::Nil) {
        let mut t = table.borrow_mut();
        return Ok(vec![t.remove(&args[1])]);
    }

    let mut t = table.borrow_mut();
    let len = t.array.len();

    let pos: i64 = if args.len() >= 2 {
        expect_int_arg("Table.remove", args, 1)?
    } else if len == 0 {
        return Ok(vec![Value::Nil]);
    } else {
        len as i64
    };

    if len == 0 || pos < 1 || (pos as usize) > len {
        return Ok(vec![Value::Nil]);
    }
    let removed = t.array.remove((pos - 1) as usize);
    Ok(vec![removed])
}

// ─── Table.sort ──────────────────────────────────────────────────────────────
//
//   Table.sort(list, comp)
//   - `comp(a, b)` should return true if `a` must come before `b`.
//   - Comparator is required; for default ordering wrap it explicitly.

fn tbl_sort(args: &[Value]) -> Result<Vec<Value>, String> {
    expect_min_arity("Table.sort", args, 2)?;
    let table = expect_table("Table.sort", args, 0)?;
    let comp = args[1].clone();

    // Copy out, sort, write back — keeps the comparator from re-entering the
    // borrowed table while the sort is in flight.
    let mut elements: Vec<Value> = table.borrow().array.clone();

    // Bottom-up merge sort, driven by `cmp(a, b)` alone.
    //
    // `sort_by` wants a three-way `Ordering`, which a boolean "a before b"
    // predicate can only produce by asking twice — once as `cmp(a, b)` and,
    // when that is false, again as `cmp(b, a)` to tell Greater from Equal.
    // That is a Saule-level call per extra probe, and it fired on rougly half
    // of every comparison in the sort. Merging needs only the one question
    // ("does the right element belong before the left?"), so this asks it
    // once, and taking the left element on a tie keeps the sort stable.
    let mut buf: Vec<Value> = Vec::with_capacity(elements.len());
    // Scratch for the comparator's two arguments, allocated once for the
    // whole sort rather than once per comparison.
    let mut argbuf: Vec<Value> = Vec::with_capacity(2);
    let n = elements.len();
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
                    if invoke_comp(&comp, &elements[j], &elements[i], &mut argbuf)? {
                        buf.push(elements[j].clone());
                        j += 1;
                    } else {
                        buf.push(elements[i].clone());
                        i += 1;
                    }
                }
                buf.extend_from_slice(&elements[i..mid]);
                buf.extend_from_slice(&elements[j..hi]);
                elements[lo..hi].clone_from_slice(&buf);
            }
            lo += 2 * width;
        }
        width *= 2;
    }

    table.borrow_mut().array = elements;
    Ok(vec![Value::Nil])
}

/// Ask the comparator whether `a` precedes `b`.
///
/// `buf` is the caller's scratch, reused across the whole sort. A sort of
/// 200k elements asks this about 3.5 million times, and the generic path
/// underneath allocates twice per call — once for the `EvaluatedArg` vector
/// built here and again for the positional vector `call_value_multi` unpacks
/// it into — before the callee runs at all. A bytecode comparator wants
/// neither: `invoke` already takes a plain slice.
fn invoke_comp(comp: &Value, a: &Value, b: &Value, buf: &mut Vec<Value>) -> Result<bool, String> {
    use crate::eval::expr::{EvaluatedArg, call_value_multi};
    if !matches!(
        comp,
        Value::Function(_) | Value::Native(_) | Value::NativeClosure(_) | Value::VmFunction(_)
    ) {
        return Err(format!(
            "Table.sort: comparator must be a function, got `{}`",
            comp.type_name()
        ));
    }
    buf.clear();
    buf.push(a.clone());
    buf.push(b.clone());
    let result = match comp {
        // The common case: a lambda the VM compiled. Straight to the
        // re-entrant call, no argument repackaging on the way, and no
        // `Vec` around the one boolean it answers with.
        Value::VmFunction(f) => f.invoke_first(buf, 0..0),
        Value::Native(nf) => (nf.func)(buf)
            .map_err(|message| crate::error::RuntimeError::TypeError { message, span: 0..0 }),
        Value::NativeClosure(nc) => (nc.func)(buf)
            .map(|vs| vs.into_iter().next().unwrap_or(Value::Nil))
            .map_err(|message| crate::error::RuntimeError::TypeError { message, span: 0..0 }),
        // A tree-walker closure still goes the long way; it is the engine
        // that needs the named-argument machinery.
        other => {
            let args = vec![
                EvaluatedArg::Positional(a.clone()),
                EvaluatedArg::Positional(b.clone()),
            ];
            call_value_multi(other.clone(), &args, 0..0)
                .map(|vs| vs.into_iter().next().unwrap_or(Value::Nil))
        }
    }
    .map_err(|e| format!("Table.sort: comparator failed: {e}"))?;
    Ok(result.is_truthy())
}

// ─── Table.concat ────────────────────────────────────────────────────────────
//
//   Table.concat(list)                       -- join with ""
//   Table.concat(list, sep)                  -- join with sep
//   Table.concat(list, sep, i)               -- from index i to end
//   Table.concat(list, sep, i, j)            -- range [i, j]

fn tbl_concat(args: &[Value]) -> Result<Vec<Value>, String> {
    expect_min_arity("Table.concat", args, 1)?;
    let table = expect_table("Table.concat", args, 0)?;
    let sep = if args.len() >= 2 && !matches!(args[1], Value::Nil) {
        expect_string_arg("Table.concat", args, 1)?
    } else {
        String::new()
    };
    let t = table.borrow();
    let len = t.array.len() as i64;
    let i = if args.len() >= 3 && !matches!(args[2], Value::Nil) {
        expect_int_arg("Table.concat", args, 2)?
    } else {
        1
    };
    let j = if args.len() >= 4 && !matches!(args[3], Value::Nil) {
        expect_int_arg("Table.concat", args, 3)?
    } else {
        len
    };

    if i > j {
        return Ok(vec![Value::Str(SauleStr::new(String::new()))]);
    }
    if i < 1 || j > len {
        return Err(format!(
            "Table.concat: range [{i}, {j}] out of bounds for length {len}"
        ));
    }

    let mut out = String::new();
    for k in i..=j {
        if k > i {
            out.push_str(&sep);
        }
        out.push_str(&t.array[(k - 1) as usize].to_display_string());
    }
    Ok(vec![Value::Str(SauleStr::new(out))])
}
