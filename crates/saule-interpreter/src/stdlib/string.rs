//! String standard library — exposed as the static class `String`.

use crate::fxhash::fxmap;
use std::cell::RefCell;
use std::rc::Rc;

use crate::env::Environment;
use crate::native_packages::NativePackage;
use crate::stdlib::{expect_arity, expect_min_arity};
use crate::value::{ClassObject, NativeClosure, Value};
use crate::value::SauleStr;

/// `import String from "string"`. Auto-prelude'd so bare
/// `String.format(…)` also works.
pub static STRING_PACKAGE: NativePackage = NativePackage {
    name: "string",
    version: saule_version::VERSION,
    install,
    exports: &["String"],
    register_sigs,
    builtins: empty_builtins,
    auto_prelude: true,
};

fn empty_builtins() -> saule_semantic::builtins::Builtins {
    saule_semantic::builtins::Builtins::default()
}

pub fn install(env: &Rc<RefCell<Environment>>) {
    let mut static_fields = fxmap();
    static_fields.insert("byte".to_string(), native("String.byte", str_byte));
    static_fields.insert("char".to_string(), native("String.char", str_char));
    static_fields.insert("format".to_string(), native("String.format", str_format));
    static_fields.insert("len".to_string(), native("String.len", str_len));
    static_fields.insert("sub".to_string(), native("String.sub", str_sub));
    static_fields.insert("rep".to_string(), native("String.rep", str_rep));
    static_fields.insert("starts".to_string(), native("String.starts", str_starts));
    static_fields.insert("ends".to_string(), native("String.ends", str_ends));
    static_fields.insert("find".to_string(), native_multi("String.find", str_find));
    static_fields.insert("lower".to_string(), native("String.lower", str_lower));
    static_fields.insert("upper".to_string(), native("String.upper", str_upper));
    static_fields.insert("iter".to_string(), native("String.iter", str_iter));

    let class = ClassObject {
        name: "String".to_string(),
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
        .define("String".to_string(), Value::Class(Rc::new(class)));
}

/// Register native signatures for the typechecker (lazy, via `sigs::lookup`).
pub fn register_sigs() {
    use crate::stdlib::sigs::{register, register_v, t_any, t_named, t_nullable};
    let s = || t_named("string");
    let i = || t_named("integer");
    let b = || t_named("boolean");
    let any = || t_named("any");
    register(
        "String.byte",
        vec![s(), t_nullable(i())],
        vec![t_nullable(i())],
    );
    // `char(...integer) -> string` — every arg must be an integer codepoint.
    register_v("String.char", vec![], i(), vec![s()]);
    // `format(fmt, ...)` — fmt is a string; rest can be anything (the spec
    // decides per-placeholder).
    register_v("String.format", vec![s()], any(), vec![s()]);
    register("String.len", vec![s()], vec![i()]);
    register("String.sub", vec![s(), i(), t_nullable(i())], vec![s()]);
    register("String.rep", vec![s(), i()], vec![s()]);
    register("String.starts", vec![s(), s()], vec![b()]);
    register("String.ends", vec![s(), s()], vec![b()]);
    register(
        "String.find",
        vec![s(), s(), t_nullable(i())],
        vec![t_nullable(i()), t_nullable(i())],
    );
    register("String.lower", vec![s()], vec![s()]);
    register("String.upper", vec![s()], vec![s()]);
    // `String.iter(s) -> fn(): (string?, integer?)` — step closure for
    // `for c, i in String.iter(s)`.
    use crate::stdlib::sigs::t_function;
    use saule_ast::Type;
    register(
        "String.iter",
        vec![s()],
        vec![t_function(
            vec![],
            Type::Tuple(vec![t_nullable(s()), t_nullable(i())]),
        )],
    );
    let _ = t_any;
}

fn native(name: &'static str, func: fn(&[Value]) -> Result<Value, String>) -> Value {
    Value::Native(Rc::new(crate::value::NativeFn { name, func }))
}

/// Wrap a multi-return native function as a `NativeClosure` so the call
/// site can destructure `local a, b = f(...)`.
fn native_multi(name: &'static str, func: fn(&[Value]) -> Result<Vec<Value>, String>) -> Value {
    Value::NativeClosure(Rc::new(NativeClosure {
        name,
        func: Box::new(func),
        param_names: Vec::new(),
    }))
}

// ─── helpers ────────────────────────────────────────────────────────────────

/// Borrow a string argument.
///
/// Returns the `Rc`, not a copy of it. Cloning the `String` here meant every
/// `String.*` call allocated and memcpy'd its whole first argument before it
/// looked at the index it was asked about — so a loop scanning a 320k-char
/// document copied 320k bytes per character read, on top of whatever the
/// function itself did.
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

// ─── character indexing ─────────────────────────────────────────────────────

/// Number of characters in `s`, for a caller that has no `Rc` to memoise on.
///
/// Saule indexes strings by character, not byte, so this is the length every
/// `String.*` function resolves its arguments against. For an ASCII string —
/// which is nearly all of them — the byte length *is* the character count.
#[inline]
pub(crate) fn char_len(s: &str) -> usize {
    if s.is_ascii() {
        s.len()
    } else {
        s.chars().count()
    }
}

thread_local! {
    /// One-entry memo of `(string, is_ascii, char_len)`.
    ///
    /// Character-indexed access into UTF-8 is O(n) unless the answer is
    /// already known, and for an immutable string the answer never changes.
    /// A loop that scans a document asks about the **same** string on every
    /// iteration, which is what kept such loops quadratic even after the
    /// per-call `Vec<char>` and the per-call `String` clone were gone: the
    /// `is_ascii` probe alone re-read 320k bytes twenty thousand times.
    ///
    /// The `Rc` is held, not just its address. Keeping the allocation alive
    /// is what makes pointer identity a sound key — a freed string cannot be
    /// replaced by a different one at the same address while the memo still
    /// refers to it. One entry is enough because the pattern this exists for
    /// is a loop over a single string; anything else simply misses and pays
    /// what it used to.
    static STR_MEMO: RefCell<Option<(SauleStr, bool, usize)>> =
        const { RefCell::new(None) };
}

thread_local! {
    /// The 128 one-character ASCII strings, and the empty string, built once.
    ///
    /// Scanning text means producing one-character strings by the million —
    /// `String.sub(src, i, i)` is the inner loop of every tokeniser, parser
    /// and state machine written in the language. Allocating a fresh `String`
    /// for each one put `malloc`/`free` at ~15% of the JSON benchmark, with
    /// another ~19% in the `Value` clone and drop traffic around it.
    ///
    /// These are immutable and shared, so handing out a clone of the `Rc` is
    /// indistinguishable from a fresh allocation to every observer: Saule
    /// strings have no identity operator and no mutation. This is the same
    /// bargain Lua makes by interning its short strings.
    static ASCII_STRS: [SauleStr; 128] =
        std::array::from_fn(|i| SauleStr::new((i as u8 as char).to_string()));
    static EMPTY_STR: SauleStr = SauleStr::new(String::new());
}

/// The shared one-character string for ASCII byte `b`.
#[inline]
fn interned_ascii(b: u8) -> SauleStr {
    debug_assert!(b < 128);
    ASCII_STRS.with(|t| t[b as usize].clone())
}

/// Longest string worth interning.
///
/// Above this the hash costs more than the allocation it saves. Lua draws
/// the same line at 40 bytes; the exact figure matters less than having one.
const INTERN_MAX: usize = 32;

/// Cap on distinct interned strings.
///
/// The table holds strong references, so without a bound a program that
/// produces endless distinct short strings would retain every one. Past the
/// cap new strings are still returned, just not remembered — the table keeps
/// whatever it learned early, which for a scanner is the vocabulary it will
/// see for the rest of the run.
const INTERN_CAP: usize = 4096;

/// A table entry, keyed by its own text so a lookup needs no allocation.
struct Interned(SauleStr);

impl std::borrow::Borrow<str> for Interned {
    fn borrow(&self) -> &str {
        self.0.as_str()
    }
}
impl PartialEq for Interned {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl Eq for Interned {}
impl std::hash::Hash for Interned {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        // Must hash as the `str` it is looked up by, or `get(&str)` misses.
        self.0.as_str().hash(h);
    }
}

thread_local! {
    static INTERN: RefCell<std::collections::HashSet<Interned, crate::fxhash::FxBuildHasher>> =
        RefCell::new(std::collections::HashSet::default());
}

/// One canonical `Rc` per distinct short string.
///
/// **Deliberately not applied to everything that builds a string.** Interning
/// trades an allocation for a hash and a probe, which only pays when the text
/// recurs: measured over 20M creations, drawing from a small vocabulary is
/// 2.0x faster interned, while creating all-distinct strings is 34% *slower*.
/// So this is called from the places whose output repeats — slicing a token
/// out of a document, case-folding — and not from `..`, whose whole purpose
/// is building strings that did not exist before.
fn intern(s: &str) -> SauleStr {
    if s.len() > INTERN_MAX {
        return SauleStr::new(s.to_string());
    }
    INTERN.with(|t| {
        let mut t = t.borrow_mut();
        if let Some(hit) = t.get(s) {
            return hit.0.clone();
        }
        let rc = SauleStr::new(s.to_string());
        if t.len() < INTERN_CAP {
            t.insert(Interned(rc.clone()));
        }
        rc
    })
}

/// The shared empty string.
#[inline]
fn interned_empty() -> SauleStr {
    EMPTY_STR.with(SauleStr::clone)
}

/// `(is_ascii, char_len)` for `s`, computed once per string.
fn str_facts(s: &SauleStr) -> (bool, usize) {
    STR_MEMO.with(|m| {
        let mut m = m.borrow_mut();
        if let Some((cached, ascii, n)) = m.as_ref()
            && SauleStr::ptr_eq(cached, s)
        {
            return (*ascii, *n);
        }
        let ascii = s.is_ascii();
        let n = if ascii { s.len() } else { s.chars().count() };
        *m = Some((s.clone(), ascii, n));
        (ascii, n)
    })
}

/// Character count of `s`, memoised.
#[inline]
pub(crate) fn char_len_rc(s: &SauleStr) -> usize {
    str_facts(s).1
}

/// Byte offset of character index `ci` (0-based), clamped to `s.len()`.
///
/// The reason this exists at all: the obvious way to index a string by
/// character is `s.chars().collect::<Vec<char>>()[ci]`, and that allocates a
/// copy of the whole string **per call**. Every function here used to do it,
/// which made any loop that scans a string quadratic — 20k single-character
/// reads across a 320k-char document took 3.7s, against Lua's 0.01s.
#[inline]
fn byte_at(s: &SauleStr, ci: usize) -> usize {
    if str_facts(s).0 {
        ci.min(s.len())
    } else {
        s.char_indices().nth(ci).map_or(s.len(), |(b, _)| b)
    }
}

// ─── functions ──────────────────────────────────────────────────────────────

fn str_byte(args: &[Value]) -> Result<Value, String> {
    expect_min_arity("String.byte", args, 1)?;
    let s = expect_string("String.byte", args, 0)?;
    let pos: i64 = if args.len() >= 2 {
        expect_int("String.byte", args, 1)?
    } else {
        1
    };
    if pos < 1 {
        return Ok(Value::Nil);
    }
    // ASCII indexes straight into the bytes; anything else decodes forward to
    // the requested character and stops there rather than to the end.
    if str_facts(&s).0 {
        return match s.as_bytes().get((pos as usize) - 1) {
            Some(b) => Ok(Value::Int(*b as i64)),
            None => Ok(Value::Nil),
        };
    }
    match s.chars().nth((pos as usize) - 1) {
        Some(c) => Ok(Value::Int(c as i64)),
        None => Ok(Value::Nil),
    }
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
    Ok(Value::Str(SauleStr::new(out)))
}

fn str_len(args: &[Value]) -> Result<Value, String> {
    expect_arity("String.len", args, 1)?;
    let s = expect_string("String.len", args, 0)?;
    Ok(Value::Int(char_len_rc(&s) as i64))
}

fn str_sub(args: &[Value]) -> Result<Value, String> {
    expect_min_arity("String.sub", args, 2)?;
    let s = expect_string("String.sub", args, 0)?;
    let n = char_len_rc(&s) as i64;
    let i = expect_int("String.sub", args, 1)?;
    let j: i64 = if args.len() >= 3 {
        expect_int("String.sub", args, 2)?
    } else {
        n
    };

    // Resolve to 0-based [start, end) range with Lua semantics.
    let mut start = if i < 0 { (n + i + 1).max(1) } else { i.max(1) };
    let mut end = if j < 0 { n + j + 1 } else { j };
    if end > n {
        end = n;
    }
    if start < 1 {
        start = 1;
    }
    if start > end {
        return Ok(Value::Str(interned_empty()));
    }
    // Slice the original bytes rather than rebuilding from characters: the
    // range is already known to fall on character boundaries.
    let from = byte_at(&s, start as usize - 1);
    let to = byte_at(&s, end as usize);
    // `sub(s, i, i)` in a loop is how every hand-written scanner reads text,
    // and a fresh allocation per character is what made that pattern cost
    // more in malloc than in the VM. One-byte results come from the shared
    // table instead.
    if to == from + 1 {
        let b = s.as_bytes()[from];
        if b < 128 {
            return Ok(Value::Str(interned_ascii(b)));
        }
    }
    // Slices of a document repeat — the tokens of a text, the keywords and
    // punctuation of a source file — which is exactly the case interning
    // wins on.
    Ok(Value::Str(intern(&s[from..to])))
}

fn str_rep(args: &[Value]) -> Result<Value, String> {
    expect_arity("String.rep", args, 2)?;
    let s = expect_string("String.rep", args, 0)?;
    let n = expect_int("String.rep", args, 1)?;
    if n <= 0 {
        return Ok(Value::Str(SauleStr::new(String::new())));
    }
    Ok(Value::Str(SauleStr::new(s.repeat(n as usize))))
}

fn str_starts(args: &[Value]) -> Result<Value, String> {
    expect_arity("String.starts", args, 2)?;
    let s = expect_string("String.starts", args, 0)?;
    let prefix = expect_string("String.starts", args, 1)?;
    Ok(Value::Bool(s.starts_with(prefix.as_str())))
}

fn str_ends(args: &[Value]) -> Result<Value, String> {
    expect_arity("String.ends", args, 2)?;
    let s = expect_string("String.ends", args, 0)?;
    let suffix = expect_string("String.ends", args, 1)?;
    Ok(Value::Bool(s.ends_with(suffix.as_str())))
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
    let start = resolve_index(init, char_len_rc(&s));

    // Search the tail in place. Collecting it into a fresh `String` copied the
    // haystack on every call, which is what made scanning loops quadratic.
    let hay = &s[byte_at(&s, start)..];
    let Some(byte_off) = hay.find(&*pat) else {
        return Ok(vec![Value::Nil]);
    };
    let char_off_in_hay = char_len(&hay[..byte_off]);
    let pat_len = char_len(&pat);
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
    Ok(Value::Str(SauleStr::new(s.to_lowercase())))
}

fn str_upper(args: &[Value]) -> Result<Value, String> {
    expect_arity("String.upper", args, 1)?;
    let s = expect_string("String.upper", args, 0)?;
    Ok(Value::Str(SauleStr::new(s.to_uppercase())))
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
                Value::Str(SauleStr::new(c.to_string())),
                Value::Int(idx as i64),
            ])
        }),
        param_names: Vec::new(),
    })))
}

// ─── format: minimal printf-style ───────────────────────────────────────────
//
// Supports `%s`, `%d`, `%i`, `%f`, `%x`, `%X`, `%o`, `%c`, `%%`.
// Optional width/precision: `%5d`, `%.2f`, `%-10s`, `%05d`.

fn str_format(args: &[Value]) -> Result<Value, String> {
    Ok(Value::Str(SauleStr::new(format_args_impl(args)?)))
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
            format!("String.format: not enough arguments for format string (missing arg {arg_idx})")
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
