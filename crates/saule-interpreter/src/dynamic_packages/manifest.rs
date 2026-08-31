//! The package manifest format and the signature-string grammar
//! (`fn<T>(a: T) -> R`) its entries are written in.

use saule_ast::Type;

/// A single exported method of a class, resolved from the manifest.
#[derive(Debug, Clone)]
pub(crate) struct MethodSpec {
    /// Saule-visible name (`circle`).
    pub(crate) name: String,
    /// Symbol exported by the shared library (`saule_engine_graphics_circle`).
    /// Still parsed without `native-packages` so manifests round-trip
    /// identically on every target; there is just nothing to resolve it in.
    #[cfg_attr(not(feature = "native-packages"), allow(dead_code))]
    pub(crate) symbol: String,
    /// Generic type-parameter names from the sig's `fn<...>` prefix.
    pub(crate) type_params: Vec<String>,
    /// Parameter types parsed from the manifest `sig`.
    pub(crate) params: Vec<Type>,
    /// Parameter names parsed from the manifest `sig`.
    pub(crate) param_names: Vec<String>,
    /// Return types parsed from the manifest `sig`.
    pub(crate) returns: Vec<Type>,
}

/// A class (static module) exposed by a package.
#[derive(Debug, Clone)]
pub(crate) struct ClassSpec {
    pub(crate) name: String,
    #[allow(dead_code)]
    pub(crate) doc: Option<String>,
    pub(crate) methods: Vec<MethodSpec>,
}

/// A parsed package manifest.
#[derive(Debug, Clone)]
pub(crate) struct Manifest {
    /// Import name (`engine`).
    pub(crate) name: String,
    #[allow(dead_code)]
    pub(crate) version: String,
    /// Candidate binary filenames in preference-neutral order, e.g.
    /// `["engine.so", "engine.dll", "engine.dylib"]`. [`pick_binary`]
    /// chooses the OS-appropriate one that actually exists.
    #[cfg_attr(not(feature = "native-packages"), allow(dead_code))]
    pub(crate) binaries: Vec<String>,
    pub(crate) exports: Vec<ClassSpec>,
}

// ─── Global state ───────────────────────────────────────────────────────────

pub(crate) fn parse_manifest(text: &str) -> Result<Manifest, String> {
    let value: toml::Value = text.parse().map_err(|e| format!("invalid TOML: {e}"))?;

    let pkg = value
        .get("package")
        .and_then(|v| v.as_table())
        .ok_or("missing [package] table")?;
    let name = pkg
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("`package.name` is required")?
        .to_string();
    let version = pkg
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("0.0.0")
        .to_string();
    let binaries: Vec<String> = pkg
        .get("binary")
        .and_then(|v| v.as_str())
        .ok_or("`package.binary` is required")?
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if binaries.is_empty() {
        return Err("`package.binary` lists no filenames".into());
    }

    let mut exports = Vec::new();
    if let Some(exports_tbl) = value.get("exports").and_then(|v| v.as_table()) {
        for (class_name, class_val) in exports_tbl {
            let class_tbl = class_val
                .as_table()
                .ok_or_else(|| format!("`exports.{class_name}` must be a table"))?;
            let doc = class_tbl
                .get("doc")
                .and_then(|v| v.as_str())
                .map(str::to_string);

            let mut methods = Vec::new();
            if let Some(arr) = class_tbl.get("methods").and_then(|v| v.as_array()) {
                for entry in arr {
                    let mt = entry.as_table().ok_or_else(|| {
                        format!("`exports.{class_name}.methods` entries must be tables")
                    })?;
                    let mname = mt
                        .get("name")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| format!("a method in `{class_name}` is missing `name`"))?
                        .to_string();
                    let sig = mt
                        .get("sig")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| format!("`{class_name}.{mname}` is missing `sig`"))?;
                    let symbol = mt
                        .get("native_symbol")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| {
                            format!("`{class_name}.{mname}` is missing `native_symbol`")
                        })?
                        .to_string();
                    let (type_params, param_names, params, returns) = parse_sig(sig)
                        .map_err(|e| format!("`{class_name}.{mname}` has an invalid sig: {e}"))?;
                    methods.push(MethodSpec {
                        name: mname,
                        symbol,
                        type_params,
                        param_names,
                        params,
                        returns,
                    });
                }
            }
            exports.push(ClassSpec {
                name: class_name.clone(),
                doc,
                methods,
            });
        }
    }

    Ok(Manifest {
        name,
        version,
        binaries,
        exports,
    })
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
/// reaching it means the manifest spells one out, which is a manifest written
/// against a language that no longer exists: it predates `SFunction` having to
/// declare what it accepts. Registering it would put a type that unifies with
/// no lambda in front of every call into the package, so the manifest is
/// rejected instead — the package fails to load with a message that says what
/// to write, rather than type-checking wrongly forever.
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
