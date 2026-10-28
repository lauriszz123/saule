//! Byte-string standard library — exposed as the static class `Bytes`.
//!
//! `String` is text: `String.len` counts codepoints, `String.sub` indexes by
//! them, and `String.char` builds UTF-8 from codepoints, so `String.char(0x80)`
//! is two bytes rather than one. That makes it the wrong tool for a PNG, a
//! digest or a wire protocol, and there was previously no right one.
//!
//! `Bytes` is the counterpart: every index here is a **byte** index, and
//! nothing converts to or from text implicitly. [`bytes_to_string`] returns
//! `string?` because there is no total function from bytes to text, which is
//! the mistake `Os.capture`'s `from_utf8_lossy` makes for you.

use crate::fxhash::fxmap;
use std::cell::RefCell;
use std::rc::Rc;

use crate::native_packages::NativePackage;
use crate::prelude::Prelude;
use crate::stdlib::{expect_arity, expect_min_arity};
use crate::value::{ClassObject, SauleBytes, SauleStr, TableObject};
use crate::value::{NativeFn, Value};

/// `import Bytes from "bytes"`. Auto-prelude'd, like `String`, so a bare
/// `Bytes.fromString(…)` works without an import.
pub static BYTES_PACKAGE: NativePackage = NativePackage {
    name: "bytes",
    version: saule_version::VERSION,
    install,
    exports: &["Bytes"],
    register_sigs,
    builtins: empty_builtins,
    auto_prelude: true,
};

fn empty_builtins() -> saule_semantic::builtins::Builtins {
    saule_semantic::builtins::Builtins::default()
}

pub fn install(env: &Rc<RefCell<Prelude>>) {
    let mut static_fields = fxmap();

    static_fields.insert("of".to_string(), native("Bytes.of", bytes_of));
    static_fields.insert("empty".to_string(), native("Bytes.empty", bytes_empty));
    static_fields.insert(
        "fromString".to_string(),
        native("Bytes.fromString", bytes_from_string),
    );
    static_fields.insert(
        "toString".to_string(),
        native("Bytes.toString", bytes_to_string),
    );
    static_fields.insert("len".to_string(), native("Bytes.len", bytes_len));
    static_fields.insert("byte".to_string(), native("Bytes.byte", bytes_byte));
    static_fields.insert(
        "toArray".to_string(),
        native("Bytes.toArray", bytes_to_array),
    );
    static_fields.insert(
        "fromArray".to_string(),
        native("Bytes.fromArray", bytes_from_array),
    );
    static_fields.insert("sub".to_string(), native("Bytes.sub", bytes_sub));
    static_fields.insert("concat".to_string(), native("Bytes.concat", bytes_concat));
    static_fields.insert(
        "indexOf".to_string(),
        native("Bytes.indexOf", bytes_index_of),
    );
    static_fields.insert("toHex".to_string(), native("Bytes.toHex", bytes_to_hex));
    static_fields.insert(
        "fromHex".to_string(),
        native("Bytes.fromHex", bytes_from_hex),
    );

    let class = ClassObject {
        name: "Bytes".to_string(),
        parent: None,
        // Statics only — a stdlib namespace class is never instantiated.
        layout: Default::default(),
        methods: Default::default(),
        static_fields: RefCell::new(static_fields),
        slot_statics: None,
        static_methods: Default::default(),
    };
    env.borrow_mut()
        .define("Bytes".to_string(), Value::Class(Rc::new(class)));
}

/// Register native signatures for the typechecker (lazy, via `sigs::lookup`).
pub fn register_sigs() {
    use crate::stdlib::sigs::{register, register_v, t_named, t_nullable, t_table_map};
    let by = || t_named("bytes");
    let s = || t_named("string");
    let i = || t_named("integer");

    // `of(...byte) -> bytes`. Typed `byte` rather than `integer`, so
    // `Bytes.of(0x89, 0x50)` is checked where it is written instead of at
    // run time: a literal out of range is now a compile error, and a
    // computed value has to say `as byte` and handle the `nil`. The runtime
    // check below stays as the backstop for a value arriving through `any`.
    register_v("Bytes.of", vec![], t_named("byte"), vec![by()]);
    register("Bytes.empty", vec![], vec![by()]);
    register("Bytes.fromString", vec![s()], vec![by()]);
    // Nullable on purpose: not every buffer is text. See the module docs.
    register("Bytes.toString", vec![by()], vec![t_nullable(s())]);
    register("Bytes.len", vec![by()], vec![i()]);
    // `byte` rather than `integer`: a byte read out of a buffer is in
    // `0..=255` by construction, and saying so in the type means a caller
    // never has to re-check what the buffer already guarantees. It widens
    // to `integer` on its own, so this is not a breaking change.
    register(
        "Bytes.byte",
        vec![by(), i()],
        vec![t_nullable(t_named("byte"))],
    );
    // The bridge to and from the elementwise form, for the times a loop
    // over `byte[]` is what you want. Separate functions rather than an
    // implicit conversion, because the two representations have very
    // different costs: a `bytes` is one byte per byte, a `byte[]` is a
    // boxed value per byte.
    register(
        "Bytes.toArray",
        vec![by()],
        vec![t_table_map(i(), t_named("byte"))],
    );
    register(
        "Bytes.fromArray",
        vec![t_table_map(i(), t_named("byte"))],
        vec![t_nullable(by())],
    );
    register("Bytes.sub", vec![by(), i(), t_nullable(i())], vec![by()]);
    register_v("Bytes.concat", vec![], by(), vec![by()]);
    register(
        "Bytes.indexOf",
        vec![by(), by(), t_nullable(i())],
        vec![t_nullable(i())],
    );
    register("Bytes.toHex", vec![by()], vec![s()]);
    register("Bytes.fromHex", vec![s()], vec![t_nullable(by())]);
}

fn native(name: &'static str, func: fn(&[Value]) -> Result<Value, String>) -> Value {
    Value::Native(Rc::new(NativeFn { name, func }))
}

// ─── helpers ────────────────────────────────────────────────────────────────

/// Borrow a `bytes` argument. Returns the `Rc`, not a copy of the buffer —
/// the same reason `String`'s helper does.
fn expect_bytes(name: &str, args: &[Value], idx: usize) -> Result<SauleBytes, String> {
    match args.get(idx) {
        Some(Value::Bytes(b)) => Ok(b.clone()),
        Some(other) => Err(format!(
            "{name} expects bytes at argument {}, got `{}`",
            idx + 1,
            other.type_name()
        )),
        None => Err(format!("{name} missing argument {}", idx + 1)),
    }
}

fn expect_string(name: &str, args: &[Value], idx: usize) -> Result<SauleStr, String> {
    match args.get(idx) {
        Some(Value::Str(s)) => Ok(s.clone()),
        Some(other) => Err(format!(
            "{name} expects a string at argument {}, got `{}`",
            idx + 1,
            other.type_name()
        )),
        None => Err(format!("{name} missing argument {}", idx + 1)),
    }
}

fn expect_int(name: &str, args: &[Value], idx: usize) -> Result<i64, String> {
    match args.get(idx) {
        Some(Value::Int(i)) => Ok(*i),
        Some(other) => Err(format!(
            "{name} expects an integer at argument {}, got `{}`",
            idx + 1,
            other.type_name()
        )),
        None => Err(format!("{name} missing argument {}", idx + 1)),
    }
}

fn bytes_value(b: Vec<u8>) -> Value {
    Value::Bytes(SauleBytes::new(b))
}

// ─── natives ────────────────────────────────────────────────────────────────

/// `Bytes.of(0x80, 0x00, 0xff)` — the escape hatch `String.char` cannot be.
///
/// Out-of-range arguments are an error rather than a truncation: a program
/// writing `Bytes.of(256)` has a bug in its arithmetic, and silently storing
/// `0` would hide it inside a buffer nobody reads with their eyes.
fn bytes_of(args: &[Value]) -> Result<Value, String> {
    let mut out = Vec::with_capacity(args.len());
    for idx in 0..args.len() {
        let n = expect_int("Bytes.of", args, idx)?;
        if !(0..=255).contains(&n) {
            return Err(format!(
                "Bytes.of expects each byte in 0..=255, got {n} at argument {}",
                idx + 1
            ));
        }
        out.push(n as u8);
    }
    Ok(bytes_value(out))
}

fn bytes_empty(args: &[Value]) -> Result<Value, String> {
    expect_arity("Bytes.empty", args, 0)?;
    Ok(bytes_value(Vec::new()))
}

/// The string's UTF-8 encoding. Always succeeds — every Saule string is
/// already valid UTF-8, which is what makes this direction total.
fn bytes_from_string(args: &[Value]) -> Result<Value, String> {
    expect_arity("Bytes.fromString", args, 1)?;
    let s = expect_string("Bytes.fromString", args, 0)?;
    Ok(bytes_value(s.as_bytes().to_vec()))
}

/// The buffer as text, or `nil` when it is not valid UTF-8.
fn bytes_to_string(args: &[Value]) -> Result<Value, String> {
    expect_arity("Bytes.toString", args, 1)?;
    let b = expect_bytes("Bytes.toString", args, 0)?;
    match b.to_str() {
        Some(s) => Ok(Value::Str(SauleStr::new(s.to_owned()))),
        None => Ok(Value::Nil),
    }
}

fn bytes_len(args: &[Value]) -> Result<Value, String> {
    expect_arity("Bytes.len", args, 1)?;
    let b = expect_bytes("Bytes.len", args, 0)?;
    Ok(Value::Int(b.len() as i64))
}

/// `Bytes.byte(b, i)` — the byte at 1-based `i`, or `nil` out of range.
///
/// Nullable rather than an error so that a scanner can walk off the end and
/// stop, which is how every such loop is written.
fn bytes_byte(args: &[Value]) -> Result<Value, String> {
    expect_arity("Bytes.byte", args, 2)?;
    let b = expect_bytes("Bytes.byte", args, 0)?;
    let i = expect_int("Bytes.byte", args, 1)?;
    if i < 1 {
        return Ok(Value::Nil);
    }
    match b.as_slice().get((i as usize) - 1) {
        Some(byte) => Ok(Value::Int(*byte as i64)),
        None => Ok(Value::Nil),
    }
}

/// `Bytes.toArray(b)` — the buffer as a `byte[]`, one element per byte.
///
/// The expensive direction, and deliberately explicit about it: every byte
/// becomes a boxed value, so the table costs many times what the buffer
/// does. Worth it for a loop that genuinely wants elements; not worth it to
/// move bytes around, which is what `bytes` is already for.
fn bytes_to_array(args: &[Value]) -> Result<Value, String> {
    expect_arity("Bytes.toArray", args, 1)?;
    let b = expect_bytes("Bytes.toArray", args, 0)?;
    let items: Vec<Value> = b.as_slice().iter().map(|n| Value::Int(*n as i64)).collect();
    Ok(Value::Table(Rc::new(RefCell::new(
        TableObject::from_array(items),
    ))))
}

/// `Bytes.fromArray(a)` — pack a `byte[]` back into a buffer.
///
/// `nil` when an element is out of `0..=255`. A `byte[]` built by the
/// checker cannot contain one, but a `table` that reached here through
/// `any` can, and silently truncating is the corruption `byte` exists to
/// prevent.
fn bytes_from_array(args: &[Value]) -> Result<Value, String> {
    expect_arity("Bytes.fromArray", args, 1)?;
    let Some(Value::Table(t)) = args.first() else {
        return Err(format!(
            "Bytes.fromArray expects a table at argument 1, got `{}`",
            args.first().map_or("nothing", |v| v.type_name())
        ));
    };
    let t = t.borrow();
    let mut out = Vec::with_capacity(t.array.len());
    for v in &t.array {
        match v {
            Value::Int(n) => match u8::try_from(*n) {
                Ok(b) => out.push(b),
                Err(_) => return Ok(Value::Nil),
            },
            _ => return Ok(Value::Nil),
        }
    }
    Ok(Value::Bytes(SauleBytes::new(out)))
}

/// `Bytes.sub(b, i, j)` — 1-based and inclusive, negatives counting from the
/// end, clamped at both ends. The same rules as `String.sub`, so the two
/// read alike; the only difference is that the index is a byte.
fn bytes_sub(args: &[Value]) -> Result<Value, String> {
    expect_min_arity("Bytes.sub", args, 2)?;
    let b = expect_bytes("Bytes.sub", args, 0)?;
    let n = b.len() as i64;
    let i = expect_int("Bytes.sub", args, 1)?;
    let j: i64 = if args.len() >= 3 && args[2] != Value::Nil {
        expect_int("Bytes.sub", args, 2)?
    } else {
        n
    };

    let mut start = if i < 0 { (n + i + 1).max(1) } else { i.max(1) };
    let mut end = if j < 0 { n + j + 1 } else { j };
    if end > n {
        end = n;
    }
    if start < 1 {
        start = 1;
    }
    if start > end {
        return Ok(bytes_value(Vec::new()));
    }
    Ok(bytes_value(
        b.as_slice()[(start as usize - 1)..(end as usize)].to_vec(),
    ))
}

fn bytes_concat(args: &[Value]) -> Result<Value, String> {
    let mut total = 0usize;
    let mut parts = Vec::with_capacity(args.len());
    for idx in 0..args.len() {
        let b = expect_bytes("Bytes.concat", args, idx)?;
        total += b.len();
        parts.push(b);
    }
    let mut out = Vec::with_capacity(total);
    for p in &parts {
        out.extend_from_slice(p.as_slice());
    }
    Ok(bytes_value(out))
}

/// `Bytes.indexOf(haystack, needle, from)` — 1-based position, or `nil`.
///
/// An empty needle matches at `from`, which is where an empty string is
/// found in every other search function.
fn bytes_index_of(args: &[Value]) -> Result<Value, String> {
    expect_min_arity("Bytes.indexOf", args, 2)?;
    let hay = expect_bytes("Bytes.indexOf", args, 0)?;
    let needle = expect_bytes("Bytes.indexOf", args, 1)?;
    let from = if args.len() >= 3 && args[2] != Value::Nil {
        expect_int("Bytes.indexOf", args, 2)?.max(1)
    } else {
        1
    };

    let h = hay.as_slice();
    let nd = needle.as_slice();
    let start = (from as usize) - 1;
    if start > h.len() {
        return Ok(Value::Nil);
    }
    if nd.is_empty() {
        return Ok(Value::Int(from));
    }
    if nd.len() > h.len() {
        return Ok(Value::Nil);
    }
    for at in start..=(h.len() - nd.len()) {
        if &h[at..at + nd.len()] == nd {
            return Ok(Value::Int((at + 1) as i64));
        }
    }
    Ok(Value::Nil)
}

/// Lowercase hex, two characters per byte. This is how a buffer gets looked
/// at — `print` deliberately shows only a length — and how one becomes a
/// table key, since `bytes` is not keyable itself.
fn bytes_to_hex(args: &[Value]) -> Result<Value, String> {
    expect_arity("Bytes.toHex", args, 1)?;
    let b = expect_bytes("Bytes.toHex", args, 0)?;
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(b.len() * 2);
    for byte in b.as_slice() {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    Ok(Value::Str(SauleStr::new(out)))
}

/// `nil` for anything that is not an even run of hex digits, so a typo in a
/// literal does not become a buffer that is quietly half right.
fn bytes_from_hex(args: &[Value]) -> Result<Value, String> {
    expect_arity("Bytes.fromHex", args, 1)?;
    let s = expect_string("Bytes.fromHex", args, 0)?;
    let text = s.as_str();
    if !text.len().is_multiple_of(2) {
        return Ok(Value::Nil);
    }
    let raw = text.as_bytes();
    let mut out = Vec::with_capacity(raw.len() / 2);
    for pair in raw.chunks(2) {
        let hi = (pair[0] as char).to_digit(16);
        let lo = (pair[1] as char).to_digit(16);
        match (hi, lo) {
            (Some(h), Some(l)) => out.push(((h << 4) | l) as u8),
            _ => return Ok(Value::Nil),
        }
    }
    Ok(bytes_value(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn by(b: &[u8]) -> Value {
        bytes_value(b.to_vec())
    }

    fn unwrap_bytes(v: Value) -> Vec<u8> {
        match v {
            Value::Bytes(b) => b.as_slice().to_vec(),
            other => panic!("expected bytes, got {}", other.type_name()),
        }
    }

    #[test]
    fn of_builds_raw_bytes_that_string_char_cannot() {
        let v = bytes_of(&[Value::Int(0x80), Value::Int(0x00), Value::Int(0xff)]).unwrap();
        assert_eq!(unwrap_bytes(v), vec![0x80, 0x00, 0xff]);
    }

    #[test]
    fn of_rejects_a_value_that_is_not_a_byte() {
        let err = bytes_of(&[Value::Int(256)]).unwrap_err();
        assert!(err.contains("0..=255"), "{err}");
    }

    #[test]
    fn a_string_round_trips_through_bytes() {
        let v = bytes_from_string(&[Value::Str(SauleStr::new("saulė".into()))]).unwrap();
        // Six bytes for five codepoints.
        assert_eq!(unwrap_bytes(v.clone()), "saulė".as_bytes().to_vec());
        assert_eq!(
            bytes_to_string(&[v]).unwrap(),
            Value::Str(SauleStr::new("saulė".into()))
        );
    }

    #[test]
    fn to_string_is_nil_for_a_buffer_that_is_not_text() {
        // 0xc3 0x28 is an invalid two-byte sequence.
        assert_eq!(bytes_to_string(&[by(&[0xc3, 0x28])]).unwrap(), Value::Nil);
    }

    #[test]
    fn len_counts_bytes() {
        assert_eq!(bytes_len(&[by("saulė".as_bytes())]).unwrap(), Value::Int(6));
    }

    #[test]
    fn byte_is_one_based_and_nil_outside() {
        let b = by(&[10, 20, 30]);
        assert_eq!(
            bytes_byte(&[b.clone(), Value::Int(1)]).unwrap(),
            Value::Int(10)
        );
        assert_eq!(
            bytes_byte(&[b.clone(), Value::Int(3)]).unwrap(),
            Value::Int(30)
        );
        assert_eq!(bytes_byte(&[b.clone(), Value::Int(4)]).unwrap(), Value::Nil);
        assert_eq!(bytes_byte(&[b, Value::Int(0)]).unwrap(), Value::Nil);
    }

    #[test]
    fn sub_follows_the_same_rules_as_string_sub() {
        let b = by(&[1, 2, 3, 4, 5]);
        assert_eq!(
            unwrap_bytes(bytes_sub(&[b.clone(), Value::Int(2), Value::Int(4)]).unwrap()),
            vec![2, 3, 4]
        );
        // No `j` means "to the end".
        assert_eq!(
            unwrap_bytes(bytes_sub(&[b.clone(), Value::Int(4)]).unwrap()),
            vec![4, 5]
        );
        // Negatives count from the end.
        assert_eq!(
            unwrap_bytes(bytes_sub(&[b.clone(), Value::Int(-2)]).unwrap()),
            vec![4, 5]
        );
        // Out of range clamps to empty rather than erroring.
        assert_eq!(
            unwrap_bytes(bytes_sub(&[b, Value::Int(9), Value::Int(12)]).unwrap()),
            Vec::<u8>::new()
        );
    }

    #[test]
    fn concat_joins_in_order() {
        let v = bytes_concat(&[by(&[1, 2]), by(&[]), by(&[3])]).unwrap();
        assert_eq!(unwrap_bytes(v), vec![1, 2, 3]);
        assert_eq!(unwrap_bytes(bytes_concat(&[]).unwrap()), Vec::<u8>::new());
    }

    #[test]
    fn index_of_finds_a_run_and_reports_one_based() {
        let hay = by(&[0, 1, 2, 3, 1, 2]);
        assert_eq!(
            bytes_index_of(&[hay.clone(), by(&[1, 2])]).unwrap(),
            Value::Int(2)
        );
        // `from` skips the first hit.
        assert_eq!(
            bytes_index_of(&[hay.clone(), by(&[1, 2]), Value::Int(3)]).unwrap(),
            Value::Int(5)
        );
        assert_eq!(bytes_index_of(&[hay, by(&[9])]).unwrap(), Value::Nil);
    }

    #[test]
    fn index_of_handles_a_needle_longer_than_the_haystack() {
        assert_eq!(
            bytes_index_of(&[by(&[1]), by(&[1, 2, 3])]).unwrap(),
            Value::Nil
        );
    }

    #[test]
    fn hex_round_trips() {
        let raw = &[0x00u8, 0x0f, 0x80, 0xff];
        let hex = bytes_to_hex(&[by(raw)]).unwrap();
        assert_eq!(hex, Value::Str(SauleStr::new("000f80ff".into())));
        assert_eq!(unwrap_bytes(bytes_from_hex(&[hex]).unwrap()), raw.to_vec());
    }

    #[test]
    fn bad_hex_is_nil_rather_than_half_a_buffer() {
        let odd = Value::Str(SauleStr::new("abc".into()));
        assert_eq!(bytes_from_hex(&[odd]).unwrap(), Value::Nil);

        let nonhex = Value::Str(SauleStr::new("00zz".into()));
        assert_eq!(bytes_from_hex(&[nonhex]).unwrap(), Value::Nil);
    }

    #[test]
    fn a_string_argument_where_bytes_are_wanted_names_both_types() {
        let err = bytes_len(&[Value::Str(SauleStr::new("x".into()))]).unwrap_err();
        assert!(err.contains("expects bytes"), "{err}");
        assert!(err.contains("string"), "{err}");
    }
}
