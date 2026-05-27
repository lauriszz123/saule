//! String standard library — exposed as the static class `String`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::env::Environment;
use crate::stdlib::{expect_arity, expect_min_arity};
use crate::value::{ClassObject, NativeClosure, Value};

pub fn install(env: &Rc<RefCell<Environment>>) {
    let mut static_fields = HashMap::new();
    static_fields.insert("byte".to_string(),   native("String.byte",   str_byte));
    static_fields.insert("char".to_string(),   native("String.char",   str_char));
    static_fields.insert("format".to_string(), native("String.format", str_format));
    static_fields.insert("len".to_string(),    native("String.len",    str_len));
    static_fields.insert("sub".to_string(),    native("String.sub",    str_sub));
    static_fields.insert("rep".to_string(),    native("String.rep",    str_rep));
    static_fields.insert("starts".to_string(), native("String.starts", str_starts));
    static_fields.insert("ends".to_string(),   native("String.ends",   str_ends));
    static_fields.insert("find".to_string(),   native_multi("String.find", str_find));
    static_fields.insert("lower".to_string(),  native("String.lower",  str_lower));
    static_fields.insert("upper".to_string(),  native("String.upper",  str_upper));
    static_fields.insert("iter".to_string(),   native("String.iter",   str_iter));

    let class = ClassObject {
        name: "String".to_string(),
        parent: None,
        field_defs: Vec::new(),
        methods: HashMap::new(),
        static_fields: RefCell::new(static_fields),
        static_methods: HashMap::new(),
        constructor: None,
    };
    env.borrow_mut()
        .define("String".to_string(), Value::Class(Rc::new(class)));
}

fn native(name: &'static str, func: fn(&[Value]) -> Result<Value, String>) -> Value {
    Value::Native(Rc::new(crate::value::NativeFn { name, func }))
}

/// Wrap a multi-return native function as a `NativeClosure` so the call
/// site can destructure `local a, b = f(...)`.
fn native_multi(
    name: &'static str,
    func: fn(&[Value]) -> Result<Vec<Value>, String>,
) -> Value {
    Value::NativeClosure(Rc::new(NativeClosure {
        name,
        func: Box::new(move |args| func(args)),
    }))
}

// ─── helpers ────────────────────────────────────────────────────────────────

fn expect_string(name: &str, args: &[Value], idx: usize) -> Result<String, String> {
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

// Lua-style 1-based index with negative-from-end. Clamps to `[1, len]`. The
// returned value is a 0-based byte offset suitable for slicing the `chars`
// vector (so it's "1-based char index minus 1").
fn resolve_index(i: i64, char_count: usize) -> usize {
    let n = char_count as i64;
    let idx = if i < 0 { n + i + 1 } else { i };
    if idx < 1 {
        0
    } else if idx > n {
        char_count
    } else {
        (idx - 1) as usize
    }
}

// ─── functions ──────────────────────────────────────────────────────────────

fn str_byte(args: &[Value]) -> Result<Value, String> {
    expect_min_arity("String.byte", args, 1)?;
    let s = expect_string("String.byte", args, 0)?;
    let chars: Vec<char> = s.chars().collect();
    let pos: i64 = if args.len() >= 2 {
        expect_int("String.byte", args, 1)?
    } else {
        1
    };
    if pos < 1 || (pos as usize) > chars.len() {
        return Ok(Value::Nil);
    }
    let c = chars[(pos as usize) - 1];
    Ok(Value::Int(c as i64))
}

fn str_char(args: &[Value]) -> Result<Value, String> {
    let mut out = String::with_capacity(args.len());
    for (i, arg) in args.iter().enumerate() {
        let code = match arg {
            Value::Int(n) => *n,
            other => {
                return Err(format!(
                    "String.char expects integer arguments, got `{}` at argument {}",
                    other.type_name(),
                    i + 1
                ));
            }
        };
        let Some(c) = u32::try_from(code).ok().and_then(char::from_u32) else {
            return Err(format!(
                "String.char: code {code} at argument {} is not a valid character",
                i + 1
            ));
        };
        out.push(c);
    }
    Ok(Value::Str(Rc::new(out)))
}

fn str_len(args: &[Value]) -> Result<Value, String> {
    expect_arity("String.len", args, 1)?;
    let s = expect_string("String.len", args, 0)?;
    Ok(Value::Int(s.chars().count() as i64))
}

fn str_sub(args: &[Value]) -> Result<Value, String> {
    expect_min_arity("String.sub", args, 2)?;
    let s = expect_string("String.sub", args, 0)?;
    let chars: Vec<char> = s.chars().collect();
    let i = expect_int("String.sub", args, 1)?;
    let j: i64 = if args.len() >= 3 {
        expect_int("String.sub", args, 2)?
    } else {
        chars.len() as i64
    };

    // Resolve to 0-based [start, end) range with Lua semantics.
    let n = chars.len() as i64;
    let mut start = if i < 0 { (n + i + 1).max(1) } else { i.max(1) };
    let mut end = if j < 0 { n + j + 1 } else { j };
    if end > n {
        end = n;
    }
    if start < 1 {
        start = 1;
    }
    if start > end {
        return Ok(Value::Str(Rc::new(String::new())));
    }
    let out: String = chars[(start as usize - 1)..(end as usize)].iter().collect();
    Ok(Value::Str(Rc::new(out)))
}

fn str_rep(args: &[Value]) -> Result<Value, String> {
    expect_arity("String.rep", args, 2)?;
    let s = expect_string("String.rep", args, 0)?;
    let n = expect_int("String.rep", args, 1)?;
    if n <= 0 {
        return Ok(Value::Str(Rc::new(String::new())));
    }
    Ok(Value::Str(Rc::new(s.repeat(n as usize))))
}

fn str_starts(args: &[Value]) -> Result<Value, String> {
    expect_arity("String.starts", args, 2)?;
    let s = expect_string("String.starts", args, 0)?;
    let prefix = expect_string("String.starts", args, 1)?;
    Ok(Value::Bool(s.starts_with(&prefix)))
}

fn str_ends(args: &[Value]) -> Result<Value, String> {
    expect_arity("String.ends", args, 2)?;
    let s = expect_string("String.ends", args, 0)?;
    let suffix = expect_string("String.ends", args, 1)?;
    Ok(Value::Bool(s.ends_with(&suffix)))
}

fn str_find(args: &[Value]) -> Result<Vec<Value>, String> {
    expect_min_arity("String.find", args, 2)?;
    let s = expect_string("String.find", args, 0)?;
    let pat = expect_string("String.find", args, 1)?;

    // 1-based char start index (Lua-style); default 1. Negative counts back
    // from the end.
    let init: i64 = if args.len() >= 3 {
        expect_int("String.find", args, 2)?
    } else {
        1
    };
    let chars: Vec<char> = s.chars().collect();
    let start = resolve_index(init, chars.len());

    let hay: String = chars[start..].iter().collect();
    let Some(byte_off) = hay.find(&pat) else {
        return Ok(vec![Value::Nil]);
    };
    let char_off_in_hay = hay[..byte_off].chars().count();
    let pat_len = pat.chars().count();
    let s_idx = (start + char_off_in_hay + 1) as i64;
    let e_idx = if pat_len == 0 {
        s_idx - 1
    } else {
        s_idx + pat_len as i64 - 1
    };
    Ok(vec![Value::Int(s_idx), Value::Int(e_idx)])
}

// (placeholder removed)
fn _unused_install_find() {}

fn str_lower(args: &[Value]) -> Result<Value, String> {
    expect_arity("String.lower", args, 1)?;
    let s = expect_string("String.lower", args, 0)?;
    Ok(Value::Str(Rc::new(s.to_lowercase())))
}

fn str_upper(args: &[Value]) -> Result<Value, String> {
    expect_arity("String.upper", args, 1)?;
    let s = expect_string("String.upper", args, 0)?;
    Ok(Value::Str(Rc::new(s.to_uppercase())))
}

// ─── iter: returns a NativeClosure yielding (char, index) per call ──────────

fn str_iter(args: &[Value]) -> Result<Value, String> {
    expect_arity("String.iter", args, 1)?;
    let s = expect_string("String.iter", args, 0)?;
    let chars: Rc<Vec<char>> = Rc::new(s.chars().collect());
    let cursor = Rc::new(RefCell::new(0usize));
    let chars_for_closure = chars.clone();
    Ok(Value::NativeClosure(Rc::new(NativeClosure {
        name: "String.iter#step",
        func: Box::new(move |_args: &[Value]| {
            let mut i = cursor.borrow_mut();
            if *i >= chars_for_closure.len() {
                return Ok(vec![Value::Nil, Value::Nil]);
            }
            let c = chars_for_closure[*i];
            let idx = *i + 1;
            *i += 1;
            Ok(vec![
                Value::Str(Rc::new(c.to_string())),
                Value::Int(idx as i64),
            ])
        }),
    })))
}

// ─── format: minimal printf-style ───────────────────────────────────────────
//
// Supports `%s`, `%d`, `%i`, `%f`, `%x`, `%X`, `%o`, `%c`, `%%`.
// Optional width/precision: `%5d`, `%.2f`, `%-10s`, `%05d`.

fn str_format(args: &[Value]) -> Result<Value, String> {
    Ok(Value::Str(Rc::new(format_args_impl(args)?)))
}

/// Shared formatter used by `String.format` and `printf`. Takes the same
/// argument shape: `args[0]` is the format string, `args[1..]` are the
/// substitutions.
pub(crate) fn format_args_impl(args: &[Value]) -> Result<String, String> {
    expect_min_arity("String.format", args, 1)?;
    let fmt = expect_string("String.format", args, 0)?;
    let mut out = String::with_capacity(fmt.len());
    let mut chars = fmt.chars().peekable();
    let mut arg_idx = 1usize;

    while let Some(ch) = chars.next() {
        if ch != '%' {
            out.push(ch);
            continue;
        }
        // Parse: %[-][0][width][.precision]<spec>
        let mut spec_flags = String::new();
        let mut spec_char: Option<char> = None;
        while let Some(&c) = chars.peek() {
            if matches!(c, '-' | '+' | '0' | ' ' | '#') {
                spec_flags.push(c);
                chars.next();
            } else {
                break;
            }
        }
        let mut width = String::new();
        while let Some(&c) = chars.peek()
            && c.is_ascii_digit()
        {
            width.push(c);
            chars.next();
        }
        let mut precision = String::new();
        if let Some(&'.') = chars.peek() {
            chars.next();
            while let Some(&c) = chars.peek()
                && c.is_ascii_digit()
            {
                precision.push(c);
                chars.next();
            }
        }
        if let Some(&c) = chars.peek() {
            spec_char = Some(c);
            chars.next();
        }
        let Some(spec) = spec_char else {
            return Err("String.format: trailing `%` without format spec".to_string());
        };

        if spec == '%' {
            out.push('%');
            continue;
        }

        let arg = args.get(arg_idx).ok_or_else(|| {
            format!(
                "String.format: not enough arguments for format string (missing arg {arg_idx})"
            )
        })?;
        arg_idx += 1;

        let formatted = format_one(spec, &spec_flags, &width, &precision, arg)?;
        out.push_str(&formatted);
    }
    Ok(out)
}

fn format_one(
    spec: char,
    flags: &str,
    width: &str,
    precision: &str,
    arg: &Value,
) -> Result<String, String> {
    let width: Option<usize> = if width.is_empty() {
        None
    } else {
        width.parse().ok()
    };
    let precision: Option<usize> = if precision.is_empty() {
        None
    } else {
        precision.parse().ok()
    };
    let left_align = flags.contains('-');
    let zero_pad = flags.contains('0') && !left_align;

    let core = match spec {
        's' => {
            let s = match arg {
                Value::Str(s) => (**s).clone(),
                other => other.to_display_string(),
            };
            if let Some(p) = precision {
                s.chars().take(p).collect::<String>()
            } else {
                s
            }
        }
        'd' | 'i' => {
            let n = as_int(arg, spec)?;
            format!("{n}")
        }
        'x' => format!("{:x}", as_int(arg, spec)?),
        'X' => format!("{:X}", as_int(arg, spec)?),
        'o' => format!("{:o}", as_int(arg, spec)?),
        'c' => {
            let n = as_int(arg, spec)?;
            let Some(c) = u32::try_from(n).ok().and_then(char::from_u32) else {
                return Err(format!("String.format `%c`: {n} is not a valid character"));
            };
            c.to_string()
        }
        'f' | 'g' | 'e' => {
            let x = as_float(arg, spec)?;
            match (spec, precision) {
                ('f', Some(p)) => format!("{x:.*}", p),
                ('f', None) => format!("{x:.6}"),
                ('e', Some(p)) => format!("{x:.*e}", p),
                ('e', None) => format!("{x:e}"),
                ('g', Some(_)) | ('g', None) => format!("{x}"),
                _ => unreachable!(),
            }
        }
        other => {
            return Err(format!("String.format: unsupported spec `%{other}`"));
        }
    };

    // Apply width padding.
    let Some(w) = width else { return Ok(core) };
    if core.chars().count() >= w {
        return Ok(core);
    }
    let pad = w - core.chars().count();
    let pad_char = if zero_pad && matches!(spec, 'd' | 'i' | 'x' | 'X' | 'o' | 'f' | 'e' | 'g') {
        '0'
    } else {
        ' '
    };
    Ok(if left_align {
        let mut s = core;
        s.extend(std::iter::repeat_n(pad_char, pad));
        s
    } else {
        let mut s = String::with_capacity(w);
        s.extend(std::iter::repeat_n(pad_char, pad));
        s.push_str(&core);
        s
    })
}

fn as_int(v: &Value, spec: char) -> Result<i64, String> {
    match v {
        Value::Int(i) => Ok(*i),
        Value::Float(f) => Ok(*f as i64),
        other => Err(format!(
            "String.format `%{spec}`: expected integer, got `{}`",
            other.type_name()
        )),
    }
}

fn as_float(v: &Value, spec: char) -> Result<f64, String> {
    match v {
        Value::Int(i) => Ok(*i as f64),
        Value::Float(f) => Ok(*f),
        other => Err(format!(
            "String.format `%{spec}`: expected number, got `{}`",
            other.type_name()
        )),
    }
}




