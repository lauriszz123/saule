//! A package's description — assembled from the metadata records compiled
//! into its library — and the signature-string grammar (`fn<T>(a: T) -> R`)
//! its members are written in.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use saule_ast::Type;

use super::embedded::RawRecord;

/// How a member is reached from Saule. Mirrors the receiver kinds the SDK's
/// macros write into each record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Receiver {
    /// `Class.member(args)`.
    Static,
    /// `object.member(args)` — the object is passed as argument 0.
    Instance,
    /// `Class(args)` — builds an object.
    Constructor,
    /// `object.member` — a property read; the object is argument 0.
    Getter,
    /// `object.member = value` — a property write; the object is argument 0.
    Setter,
}

impl Receiver {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "static" => Receiver::Static,
            "instance" => Receiver::Instance,
            "constructor" => Receiver::Constructor,
            "getter" => Receiver::Getter,
            "setter" => Receiver::Setter,
            _ => return None,
        })
    }
}

/// A single exported member of a class.
#[derive(Debug, Clone)]
pub(crate) struct MethodSpec {
    /// Saule-visible name (`circle`). `init` for a constructor.
    pub(crate) name: String,
    /// Symbol exported by the shared library (`saule_export_Graphics_circle`).
    /// Kept without `native-packages` too, so a package describes itself
    /// identically on every target; there is just nothing to resolve it in.
    #[cfg_attr(not(feature = "native-packages"), allow(dead_code))]
    pub(crate) symbol: String,
    pub(crate) receiver: Receiver,
    /// Generic type-parameter names from the sig's `fn<...>` prefix.
    pub(crate) type_params: Vec<String>,
    /// Parameter types parsed from the sig, *excluding* the receiver.
    pub(crate) params: Vec<Type>,
    /// Parameter names parsed from the sig.
    pub(crate) param_names: Vec<String>,
    /// Return types parsed from the sig.
    pub(crate) returns: Vec<Type>,
    /// The author's doc comment, shown by the editor.
    pub(crate) doc: Option<String>,
}

/// A class exposed by a package: a namespace of static functions, or — when
/// `instantiable` — a type whose objects programs hold.
#[derive(Debug, Clone)]
pub(crate) struct ClassSpec {
    pub(crate) name: String,
    pub(crate) doc: Option<String>,
    /// Declared with `#[saule_class]`: it has objects, so it may have a
    /// constructor, instance methods and properties.
    pub(crate) instantiable: bool,
    /// Sorted by name, so everything built from a manifest is deterministic.
    pub(crate) methods: Vec<MethodSpec>,
}

impl ClassSpec {
    pub(crate) fn members(&self, receiver: Receiver) -> impl Iterator<Item = &MethodSpec> {
        self.methods.iter().filter(move |m| m.receiver == receiver)
    }

    #[cfg_attr(not(feature = "native-packages"), allow(dead_code))]
    pub(crate) fn constructor(&self) -> Option<&MethodSpec> {
        self.members(Receiver::Constructor).next()
    }
}

/// A fieldless enum exposed by a package.
#[derive(Debug, Clone)]
pub(crate) struct EnumSpec {
    pub(crate) name: String,
    pub(crate) doc: Option<String>,
    pub(crate) variants: Vec<String>,
    /// One per variant; empty where a variant has no doc comment.
    pub(crate) variant_docs: Vec<String>,
}

/// An installed package, as described by its embedded metadata.
#[derive(Debug, Clone)]
pub(crate) struct Manifest {
    /// Import name (`engine`).
    pub(crate) name: String,
    #[allow(dead_code)]
    pub(crate) version: String,
    #[allow(dead_code)]
    pub(crate) doc: Option<String>,
    /// The library file the metadata was read from — and the one to load.
    #[cfg_attr(not(feature = "native-packages"), allow(dead_code))]
    pub(crate) path: PathBuf,
    /// Sorted by name.
    pub(crate) exports: Vec<ClassSpec>,
    /// Sorted by name.
    pub(crate) enums: Vec<EnumSpec>,
}

/// Assemble and check a package from the records read out of `path`.
///
/// Every inconsistency is an error rather than something to paper over: a
/// package whose description contradicts itself would otherwise type-check
/// programs against a surface that does not exist.
pub(crate) fn manifest_from_records(records: &[RawRecord], path: &Path) -> Result<Manifest, String> {
    let mut package: Option<(String, String, Option<String>)> = None;
    let mut classes: BTreeMap<String, ClassSpec> = BTreeMap::new();
    let mut enums: BTreeMap<String, EnumSpec> = BTreeMap::new();
    let mut methods: Vec<(String, MethodSpec)> = Vec::new();

    for rec in records {
        let table: toml::Table = rec.payload.parse().map_err(|e| {
            format!("has a metadata record `{}` that is not valid TOML: {e}", rec.symbol)
        })?;
        let field = |key: &str| table.get(key).and_then(|v| v.as_str()).map(str::to_string);
        let required = |key: &str| {
            field(key)
                .ok_or_else(|| format!("has a metadata record `{}` with no `{key}`", rec.symbol))
        };
        match required("kind")?.as_str() {
            "package" => {
                if package.is_some() {
                    return Err("declares the package twice (`saule_package!` more than once)"
                        .to_string());
                }
                check_abi(table.get("abi_version").and_then(|v| v.as_integer()))?;
                package = Some((required("name")?, required("version")?, field("doc")));
            }
            "class" => {
                let name = required("name")?;
                let instantiable = table
                    .get("instantiable")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if classes.contains_key(&name) {
                    return Err(format!("declares class `{name}` twice"));
                }
                classes.insert(
                    name.clone(),
                    ClassSpec {
                        name,
                        doc: field("doc"),
                        instantiable,
                        methods: Vec::new(),
                    },
                );
            }
            "enum" => {
                let name = required("name")?;
                let strings = |key: &str| -> Vec<String> {
                    table
                        .get(key)
                        .and_then(|v| v.as_array())
                        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                        .unwrap_or_default()
                };
                let variants = strings("variants");
                if variants.is_empty() {
                    return Err(format!("declares enum `{name}` with no variants"));
                }
                enums.insert(
                    name.clone(),
                    EnumSpec {
                        name,
                        doc: field("doc"),
                        variants,
                        variant_docs: strings("variant_docs"),
                    },
                );
            }
            "method" => {
                let class = required("class")?;
                let name = required("name")?;
                let receiver_str = required("receiver")?;
                let receiver = Receiver::parse(&receiver_str).ok_or_else(|| {
                    format!(
                        "declares `{class}.{name}` with an unknown receiver kind `{receiver_str}`"
                    )
                })?;
                let sig = required("sig")?;
                let (type_params, param_names, params, returns) =
                    parse_sig(&sig).map_err(|e| {
                        format!("declares `{class}.{name}` with an invalid signature: {e}")
                    })?;
                methods.push((
                    class,
                    MethodSpec {
                        name,
                        symbol: required("symbol")?,
                        receiver,
                        type_params,
                        params,
                        param_names,
                        returns,
                        doc: field("doc"),
                    },
                ));
            }
            // A kind a newer SDK writes and this toolchain does not know.
            // Ignoring it keeps an old toolchain able to use the parts of a
            // newer package it does understand; the ABI check above is what
            // guards against the parts it would get *wrong*.
            _ => {}
        }
    }

    let (name, version, doc) = package.ok_or(
        "has no package record — declare the package with `saule_package!` in the crate root",
    )?;

    for (class, method) in methods {
        // A function in a namespace nobody listed in `saule_package!` is
        // still a member of that namespace; it just has no doc comment.
        let spec = classes.entry(class.clone()).or_insert_with(|| ClassSpec {
            name: class.clone(),
            doc: None,
            instantiable: false,
            methods: Vec::new(),
        });
        if method.receiver != Receiver::Static && !spec.instantiable {
            return Err(format!(
                "declares `{class}.{}` as needing an object, but `{class}` is not a \
                 `#[saule_class]`",
                method.name
            ));
        }
        spec.methods.push(method);
    }

    for spec in classes.values_mut() {
        spec.methods.sort_by(|a, b| a.name.cmp(&b.name));
        check_members(spec)?;
        if enums.contains_key(&spec.name) {
            return Err(format!("declares `{}` as both a class and an enum", spec.name));
        }
    }

    Ok(Manifest {
        name,
        version,
        doc,
        path: path.to_path_buf(),
        exports: classes.into_values().collect(),
        enums: enums.into_values().collect(),
    })
}

/// A package's declared ABI must be this toolchain's.
///
/// The early half of the ABI check. The authoritative one is the
/// `saule_abi_version` symbol, which cannot drift from the code because the
/// compiler put it there. This one runs without loading anything, so a
/// stale package is refused before a program is type-checked against it —
/// rather than after the user has written code against signatures that were
/// never going to run.
fn check_abi(declared: Option<i64>) -> Result<(), String> {
    let ours = saule_native_abi::ABI_VERSION;
    match declared {
        Some(v) if v == i64::from(ours) => Ok(()),
        Some(v) => Err(format!(
            "was built against native ABI version {v}, but this toolchain speaks \
             version {ours}. Rebuild the package against a matching `saule-sdk`."
        )),
        None => Err(format!(
            "declares no ABI version, so it predates native ABI version {ours}. \
             Rebuild the package against the current `saule-sdk`."
        )),
    }
}

/// A class's members must not collide, and it has at most one constructor.
fn check_members(spec: &ClassSpec) -> Result<(), String> {
    let class = &spec.name;
    if spec.members(Receiver::Constructor).count() > 1 {
        return Err(format!("declares more than one constructor for `{class}`"));
    }
    // What `object.name` could mean: an instance method or a property. A
    // getter and a setter of the same name are one property.
    let mut seen: BTreeMap<&str, Receiver> = BTreeMap::new();
    for m in &spec.methods {
        let slot = match m.receiver {
            Receiver::Instance => Receiver::Instance,
            Receiver::Getter | Receiver::Setter => Receiver::Getter,
            _ => continue,
        };
        if let Some(prev) = seen.insert(&m.name, slot)
            && prev != slot
        {
            return Err(format!(
                "declares `{class}.{}` as both a method and a property",
                m.name
            ));
        }
    }
    Ok(())
}

/// A parsed signature: `(type_params, param_names, params, returns)`.
pub(crate) type ParsedSig = (Vec<String>, Vec<String>, Vec<Type>, Vec<Type>);

/// Parse a `fn<T>(a: T, b: U) -> R` signature string into a [`ParsedSig`]
/// using the typeck type builders. The optional `<...>` prefix lists
/// generic type-parameter names. A `nil` (or absent) return becomes
/// `[nil]`; a parenthesised `(A, B)` return becomes a multi-return.
pub(crate) fn parse_sig(sig: &str) -> Result<ParsedSig, String> {
    let s = sig.trim();
    let s = s.strip_prefix("fn").unwrap_or(s).trim_start();

    // Optional `<T, U>` generic prefix: collect the type-parameter names.
    let mut type_params = Vec::new();
    let s = if let Some(rest) = s.strip_prefix('<') {
        let gt = rest
            .find('>')
            .ok_or("unbalanced `<...>` in type parameters")?;
        for p in split_top_level(&rest[..gt]) {
            type_params.push(p);
        }
        rest[gt + 1..].trim_start()
    } else {
        s
    };

    let open = s.find('(').ok_or("expected '(' after `fn`")?;
    // Find the parameter list's matching ')', tracking nesting so a
    // parenthesised tuple return type isn't mistaken for it.
    let mut depth = 0i32;
    let mut close = None;
    for (i, ch) in s[open..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open + i);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close.ok_or("unbalanced parentheses in parameter list")?;

    let params_str = &s[open + 1..close];
    let mut param_names = Vec::new();
    let mut params = Vec::new();
    for (i, p) in split_top_level(params_str).iter().enumerate() {
        let (name, ty) = parse_param(p, i);
        param_names.push(name);
        params.push(parse_type(ty));
    }

    let rest = s[close + 1..].trim();
    let ret_str = rest.strip_prefix("->").map(str::trim).unwrap_or("");
    let returns = parse_return(ret_str);

    if let Some(i) = params.iter().chain(returns.iter()).position(names_function) {
        return Err(format!(
            "`function` is not a type (slot {i}); a callback declares the calls \
             it accepts, e.g. `fn(T) -> T`"
        ));
    }

    Ok((type_params, param_names, params, returns))
}

/// Whether a parsed type mentions the bare name `function` anywhere.
///
/// A callback's type is its signature — `fn(T) -> T` — and `parse_type` builds
/// exactly that from an `fn(...)` token. Nothing constructs the bare name, so
/// reaching it means the signature spells one out, which is a package written
/// against a language that no longer exists: it predates `SFunction` having to
/// declare what it accepts. Registering it would put a type that unifies with
/// no lambda in front of every call into the package, so the package is
/// rejected instead — it fails to load with a message that says what to
/// write, rather than type-checking wrongly forever.
fn names_function(ty: &Type) -> bool {
    match ty {
        Type::Named(n) => n == "function",
        Type::Nullable(inner) => names_function(inner),
        Type::Table { key, value } => {
            key.as_deref().is_some_and(names_function) || names_function(value)
        }
        Type::Tuple(items) => items.iter().any(names_function),
        Type::Function { params, ret } => params.iter().any(names_function) || names_function(ret),
        Type::Generic(g) => g.args.iter().any(names_function),
    }
}

/// Extract `(name, type)` from a signature parameter token.
/// Unnamed parameters are given a synthetic `arg{idx}` name.
pub(crate) fn parse_param(p: &str, idx: usize) -> (String, &str) {
    match p.split_once(':') {
        Some((name, ty)) => {
            let name = name.trim();
            let name = if name.is_empty() {
                format!("arg{idx}")
            } else {
                name.to_string()
            };
            (name, ty.trim())
        }
        None => (format!("arg{idx}"), p.trim()),
    }
}

pub(crate) fn parse_return(ret: &str) -> Vec<Type> {
    let ret = ret.trim();
    if ret.is_empty() || ret == "nil" {
        return vec![saule_typeck::sigs::t_named("nil")];
    }
    if let Some(inner) = ret.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
        return split_top_level(inner)
            .iter()
            .map(|t| parse_type(t))
            .collect();
    }
    vec![parse_type(ret)]
}

/// Parse a single type token: a trailing `?` makes it nullable; a
/// `table<...>` token becomes a typed array (`table<T>`) or map
/// (`table<K, V>`); `fn(A, B) -> R` becomes a function type, and a
/// parenthesised token is either grouping or a tuple.
pub(crate) fn parse_type(tok: &str) -> Type {
    let t = tok.trim();
    // `fn(A, B) -> R` comes first, before the nullable suffix: a trailing `?`
    // on one of these belongs to the *return* type, and making a function
    // itself nullable needs the parenthesised `(fn() -> nil)?` below.
    //
    // Native signatures grew function types once `SFunction` had to declare
    // what it accepts; before that a callback erased to a bare name and this
    // arm was never reached.
    if let Some(rest) = t.strip_prefix("fn") {
        let rest = rest.trim_start();
        if let Some(inner) = balanced_inner(rest) {
            let params = split_top_level(inner)
                .iter()
                .map(|p| parse_type(p))
                .collect();
            // Everything past the parameter list's `)`. The arrow is optional
            // so a malformed entry degrades to `-> nil` rather than to a named
            // type spelled `fn(...)`.
            let after = rest[inner.len() + 2..].trim();
            let ret = after.strip_prefix("->").map(str::trim).unwrap_or("");
            let ret = if ret.is_empty() {
                saule_typeck::sigs::t_named("nil")
            } else {
                parse_type(ret)
            };
            return saule_typeck::sigs::t_function(params, ret);
        }
    }
    // A fully parenthesised token: grouping when it holds one type — which is
    // what `(fn() -> nil)?` uses to put the `?` on the function rather than on
    // its return — and a tuple when it holds several.
    if let Some(inner) = balanced_inner(t).filter(|i| i.len() + 2 == t.len()) {
        let parts = split_top_level(inner);
        return match parts.as_slice() {
            [one] => parse_type(one),
            many => Type::Tuple(many.iter().map(|p| parse_type(p)).collect()),
        };
    }
    if let Some(base) = t.strip_suffix('?') {
        return saule_typeck::sigs::t_nullable(parse_type(base));
    }
    // `table<T>` / `table<K, V>` — anything else falls through to a named type.
    if let Some(inner) = t.strip_prefix("table<").and_then(|r| r.strip_suffix('>')) {
        let parts = split_top_level(inner);
        return match parts.as_slice() {
            [v] => saule_typeck::sigs::t_table(parse_type(v)),
            [k, v] => saule_typeck::sigs::t_table_map(parse_type(k), parse_type(v)),
            // Malformed (`table<>` or 3+ args) — degrade to an untyped table.
            _ => saule_typeck::sigs::t_table(saule_typeck::sigs::t_any()),
        };
    }
    saule_typeck::sigs::t_named(t)
}

/// The contents of the `(...)` starting at `s`'s first character, or `None`
/// when `s` doesn't begin with `(` or the parentheses don't balance.
fn balanced_inner(s: &str) -> Option<&str> {
    if !s.starts_with('(') {
        return None;
    }
    let mut depth = 0i32;
    for (i, ch) in s.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[1..i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Split on commas that are not nested inside `<...>` or `(...)`. Empty
/// segments (e.g. an empty parameter list) are dropped.
///
/// The `>` of an `->` is not a closing bracket. Counting it as one drove the
/// depth negative, and then every later comma looked nested — which merged
/// `f: fn(U, T) -> U, init: U` into a single parameter.
pub(crate) fn split_top_level(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    let mut prev = '\0';
    for ch in s.chars() {
        match ch {
            '<' | '(' => {
                depth += 1;
                cur.push(ch);
            }
            '>' if prev == '-' => cur.push(ch),
            '>' | ')' => {
                depth -= 1;
                cur.push(ch);
            }
            ',' if depth == 0 => {
                let t = cur.trim();
                if !t.is_empty() {
                    out.push(t.to_string());
                }
                cur.clear();
            }
            _ => cur.push(ch),
        }
        prev = ch;
    }
    let t = cur.trim();
    if !t.is_empty() {
        out.push(t.to_string());
    }
    out
}
