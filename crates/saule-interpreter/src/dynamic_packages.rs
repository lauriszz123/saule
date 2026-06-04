//! Dynamically-loaded native packages.
//!
//! Where [`crate::native_packages`] handles packages that are *statically
//! linked* into the interpreter (the stdlib), this module handles packages
//! that live **outside** the binary as shared libraries and are described by
//! a TOML manifest. The pipeline is:
//!
//! ```text
//! ~/.saule/native_manifests/<pkg>.toml   ── describes exports + symbol names
//! ~/.saule/native_packages/<pkg>.{dll,so,dylib}  ── the compiled code
//! ```
//!
//! 1. [`discover`] (run once from [`crate::init`]) scans the manifest
//!    directory, parses every `*.toml`, and records the resulting
//!    [`Manifest`]s in a process-global registry. **No binary is loaded yet.**
//! 2. [`register_sigs`] (driven by the typeck initializer, per thread) walks
//!    the discovered manifests and registers each method's type signature so
//!    `Graphics.circle(...)` type-checks *before* the binary is ever loaded.
//! 3. On the first `import X from "<pkg>"`, the module loader resolves the
//!    import to a sentinel path ([`sentinel_path`]) and calls
//!    [`build_exports`], which lazily loads the shared library via
//!    `libloading`, resolves the symbols named in the manifest, and wraps
//!    each one in a [`Value::NativeClosure`].
//!
//! The interpreter never needs the package at compile time and the package
//! never links the interpreter — the only shared contract is
//! [`saule_native_abi`].

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Once, RwLock};

use libloading::Library;
use saule_ast::Type;
use saule_native_abi::{tag, CValue, NativeSymbolFn};

use crate::error::RuntimeError;
use crate::module::ModuleExports;
use crate::value::{ClassObject, NativeClosure, Value};

/// A single exported method of a class, resolved from the manifest.
#[derive(Debug, Clone)]
struct MethodSpec {
    /// Saule-visible name (`circle`).
    name: String,
    /// Symbol exported by the shared library (`saule_engine_graphics_circle`).
    symbol: String,
    /// Parameter types parsed from the manifest `sig`.
    params: Vec<Type>,
    /// Return types parsed from the manifest `sig`.
    returns: Vec<Type>,
}

/// A class (static module) exposed by a package.
#[derive(Debug, Clone)]
struct ClassSpec {
    name: String,
    #[allow(dead_code)]
    doc: Option<String>,
    methods: Vec<MethodSpec>,
}

/// A parsed package manifest.
#[derive(Debug, Clone)]
struct Manifest {
    /// Import name (`engine`).
    name: String,
    #[allow(dead_code)]
    version: String,
    /// Candidate binary filenames in preference-neutral order, e.g.
    /// `["engine.so", "engine.dll", "engine.dylib"]`. [`pick_binary`]
    /// chooses the OS-appropriate one that actually exists.
    binaries: Vec<String>,
    exports: Vec<ClassSpec>,
}

// ─── Global state ───────────────────────────────────────────────────────────

/// Discovered manifests keyed by import name. Populated once by [`discover`].
static MANIFESTS: RwLock<Option<HashMap<String, Arc<Manifest>>>> = RwLock::new(None);

/// Loaded shared libraries, keyed by package name. Each library is kept
/// alive for the life of the process (the [`NativeClosure`]s built from it
/// hold raw function pointers into it).
static LIBS: RwLock<Option<HashMap<String, Arc<Library>>>> = RwLock::new(None);

static DISCOVER_ONCE: Once = Once::new();

// ─── Filesystem layout ──────────────────────────────────────────────────────

fn saule_home() -> PathBuf {
    std::env::var_os("SAULE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".saule")
}

fn manifests_dir() -> PathBuf {
    saule_home().join("native_manifests")
}

fn packages_dir() -> PathBuf {
    saule_home().join("native_packages")
}

// ─── Discovery ──────────────────────────────────────────────────────────────

/// Scan `~/.saule/native_manifests/` and record every valid manifest. Idempotent
/// — only the first call does work. A malformed manifest is logged to stderr
/// and skipped rather than aborting startup.
pub fn discover() {
    DISCOVER_ONCE.call_once(|| {
        let mut map = HashMap::new();
        let dir = manifests_dir();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                    continue;
                }
                match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| parse_manifest(&t)) {
                    Ok(manifest) => {
                        map.insert(manifest.name.clone(), Arc::new(manifest));
                    }
                    Err(err) => {
                        eprintln!("saule: ignoring native manifest `{}`: {err}", path.display());
                    }
                }
            }
        }
        *MANIFESTS.write().expect("dynamic manifest registry poisoned") = Some(map);
    });
}

/// Register every discovered package's method signatures with `saule-typeck`.
/// Wired into the typeck initializer so it runs once per thread that
/// type-checks — see [`crate::stdlib::register_all_sigs`].
pub fn register_sigs() {
    let guard = MANIFESTS.read().expect("dynamic manifest registry poisoned");
    let Some(map) = guard.as_ref() else { return };
    for manifest in map.values() {
        for class in &manifest.exports {
            for method in &class.methods {
                let qname = format!("{}.{}", class.name, method.name);
                saule_typeck::sigs::register(&qname, method.params.clone(), method.returns.clone());
            }
        }
    }
}

/// Look up a discovered package by import name.
fn lookup(name: &str) -> Option<Arc<Manifest>> {
    let guard = MANIFESTS.read().expect("dynamic manifest registry poisoned");
    guard.as_ref().and_then(|m| m.get(name).cloned())
}

/// Is `name` a discovered dynamic package?
pub fn is_dynamic_package(name: &str) -> bool {
    lookup(name).is_some()
}

// ─── Module-loader integration ──────────────────────────────────────────────

/// Mint the sentinel `PathBuf` used as the module-cache key for a dynamic
/// package. Mirrors [`crate::native_packages::sentinel_path`] but with a
/// distinct prefix so the two kinds never collide.
pub fn sentinel_path(name: &str) -> PathBuf {
    PathBuf::from(format!("__saule_dynamic__/{name}"))
}

/// Inverse of [`sentinel_path`]; `None` for any non-dynamic path.
pub fn name_from_sentinel(path: &Path) -> Option<&str> {
    path.to_str()?.strip_prefix("__saule_dynamic__/")
}

/// Build the importable surface of a dynamic package, loading its shared
/// library on first use. Called by the module loader when it sees a dynamic
/// sentinel path.
pub fn build_exports(name: &str, import_span: std::ops::Range<usize>) -> Result<ModuleExports, RuntimeError> {
    let manifest = lookup(name).ok_or_else(|| RuntimeError::ImportError {
        message: format!("native package `{name}` is no longer registered"),
        span: import_span.clone(),
    })?;

    let lib = load_library(&manifest).map_err(|message| RuntimeError::ImportError {
        message,
        span: import_span.clone(),
    })?;

    let mut exports = ModuleExports::default();
    for class in &manifest.exports {
        let class_obj = build_class(class, &lib).map_err(|message| RuntimeError::ImportError {
            message,
            span: import_span.clone(),
        })?;
        exports
            .values
            .insert(class.name.clone(), Value::Class(Rc::new(class_obj)));
    }
    Ok(exports)
}

/// Build semantic class metadata for a dynamic package's exported classes,
/// applying the importing statement's aliases. Mirrors how *static* native
/// packages contribute to a [`saule_semantic::ModuleSeed`] so the semantic
/// analyzer (and therefore the LSP) recognises `Window`, `Graphics`, … as
/// classes with static methods instead of flagging them as undefined.
///
/// Returns an empty vec if `name` isn't a discovered dynamic package.
pub fn seed_classes(
    name: &str,
    names: &saule_ast::ImportNames,
) -> Vec<(String, saule_semantic::ClassInfo)> {
    let Some(manifest) = lookup(name) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for class in &manifest.exports {
        let local = match names {
            saule_ast::ImportNames::All => Some(class.name.clone()),
            saule_ast::ImportNames::List(items) => items
                .iter()
                .find(|(orig, _)| orig == &class.name)
                .map(|(orig, alias)| alias.clone().unwrap_or_else(|| orig.clone())),
        };
        let Some(alias) = local else { continue };
        out.push((alias, class_info(class)));
    }
    out
}

/// Convert a manifest [`ClassSpec`] into a semantic [`saule_semantic::ClassInfo`],
/// recording each exported method as a public, static signature.
fn class_info(class: &ClassSpec) -> saule_semantic::ClassInfo {
    let mut info = saule_semantic::ClassInfo::default();
    for m in &class.methods {
        info.members.insert(m.name.clone(), false); // public
        let params = m
            .params
            .iter()
            .enumerate()
            .map(|(i, ty)| saule_ast::Param {
                name: format!("arg{i}"),
                ty: ty.clone(),
                default: None,
                variadic: false,
                span: 0..0,
            })
            .collect();
        let return_ty = match m.returns.as_slice() {
            [single] => Some(single.clone()),
            _ => None,
        };
        info.methods.insert(
            m.name.clone(),
            saule_semantic::MethodSig {
                is_static: true,
                is_private: false,
                type_params: Vec::new(),
                params,
                return_ty,
            },
        );
    }
    info
}

// ─── Library loading ────────────────────────────────────────────────────────

fn load_library(manifest: &Manifest) -> Result<Arc<Library>, String> {
    if let Some(map) = LIBS.read().expect("dynamic lib cache poisoned").as_ref() {
        if let Some(lib) = map.get(&manifest.name) {
            return Ok(lib.clone());
        }
    }

    let dir = packages_dir();
    let path = pick_binary(&dir, &manifest.binaries).ok_or_else(|| {
        format!(
            "no loadable binary for package `{}` found in `{}` (tried: {})",
            manifest.name,
            dir.display(),
            manifest.binaries.join(", ")
        )
    })?;

    // SAFETY: loading arbitrary native code is inherently unsafe; the user
    // opted in by placing the binary under ~/.saule/native_packages.
    let lib = unsafe { Library::new(&path) }
        .map_err(|e| format!("failed to load `{}`: {e}", path.display()))?;
    let lib = Arc::new(lib);

    LIBS.write()
        .expect("dynamic lib cache poisoned")
        .get_or_insert_with(HashMap::new)
        .insert(manifest.name.clone(), lib.clone());
    Ok(lib)
}

/// Choose the binary to load: prefer the file whose extension matches the
/// host OS, otherwise fall back to the first listed candidate that exists.
fn pick_binary(dir: &Path, candidates: &[String]) -> Option<PathBuf> {
    let preferred_ext = if cfg!(windows) {
        "dll"
    } else if cfg!(target_os = "macos") {
        "dylib"
    } else {
        "so"
    };

    let mut fallback: Option<PathBuf> = None;
    for cand in candidates {
        let p = dir.join(cand.trim());
        if !p.is_file() {
            continue;
        }
        if p.extension().and_then(|e| e.to_str()) == Some(preferred_ext) {
            return Some(p);
        }
        fallback.get_or_insert(p);
    }
    fallback
}

// ─── Class / method construction ────────────────────────────────────────────

fn build_class(spec: &ClassSpec, lib: &Arc<Library>) -> Result<ClassObject, String> {
    let mut static_fields = HashMap::new();
    for method in &spec.methods {
        let qname = format!("{}.{}", spec.name, method.name);
        let value = make_native(qname, lib.clone(), &method.symbol)?;
        static_fields.insert(method.name.clone(), value);
    }
    Ok(ClassObject {
        name: spec.name.clone(),
        parent: None,
        field_defs: Vec::new(),
        methods: HashMap::new(),
        static_fields: RefCell::new(static_fields),
        static_methods: HashMap::new(),
        constructor: None,
    })
}

/// Resolve `symbol` in `lib` and wrap it in a [`Value::NativeClosure`] that
/// marshals Saule values across the ABI on every call. The closure captures
/// an `Arc<Library>` so the resolved function pointer stays valid.
fn make_native(qname: String, lib: Arc<Library>, symbol: &str) -> Result<Value, String> {
    // SAFETY: the symbol must have the ABI's frozen signature; a mismatch is
    // the package author's bug. We copy the function pointer out and keep the
    // library alive via the captured `Arc` below.
    let raw: NativeSymbolFn = unsafe {
        let sym: libloading::Symbol<NativeSymbolFn> = lib
            .get(symbol.as_bytes())
            .map_err(|e| format!("symbol `{symbol}` not found: {e}"))?;
        *sym
    };

    // `NativeClosure::name` is `&'static str`; method names are bounded and
    // loaded once, so leaking is acceptable.
    let name: &'static str = Box::leak(qname.into_boxed_str());

    let func = Box::new(move |args: &[Value]| -> Result<Vec<Value>, String> {
        // Keep the library alive for the duration of the call (and for as
        // long as this closure lives).
        let _keep = &lib;
        call_native(raw, args).map(|v| vec![v])
    });

    Ok(Value::NativeClosure(Rc::new(NativeClosure { name, func })))
}

/// Marshal `args` into [`CValue`]s, invoke `raw`, and translate the result
/// back into a [`Value`].
fn call_native(raw: NativeSymbolFn, args: &[Value]) -> Result<Value, String> {
    let mut cargs = Vec::with_capacity(args.len());
    for (i, v) in args.iter().enumerate() {
        cargs.push(value_to_cvalue(v).ok_or_else(|| {
            format!(
                "cannot pass argument #{} ({}) across the native boundary",
                i + 1,
                v.type_name()
            )
        })?);
    }

    let mut out = CValue::nil();
    // SAFETY: `cargs` is a valid, contiguous, initialised slice; `out` is a
    // valid writable slot. The string payloads in `cargs` borrow from `args`,
    // which outlives the call.
    let code = unsafe { raw(cargs.as_ptr(), cargs.len(), &mut out) };

    if code != 0 {
        // SAFETY: on failure the callee writes an ERR/STR value into `out`.
        let msg = unsafe { out.as_str() }
            .unwrap_or("native package call failed")
            .to_string();
        return Err(msg);
    }
    Ok(cvalue_to_value(&out))
}

/// Borrowed conversion `Value -> CValue`. Returns `None` for values that have
/// no ABI representation (tables, instances, functions, …). String payloads
/// borrow from `v`, so the result is only valid while `v` is alive.
fn value_to_cvalue(v: &Value) -> Option<CValue> {
    Some(match v {
        Value::Nil => CValue::nil(),
        Value::Bool(b) => CValue::boolean(*b),
        Value::Int(i) => CValue::integer(*i),
        Value::Float(f) => CValue::float(*f),
        Value::Str(s) => CValue::string_borrowed(s.as_bytes()),
        _ => return None,
    })
}

/// Owned conversion `CValue -> Value`. Copies string payloads immediately so
/// the result outlives the callee's thread-local return buffer.
fn cvalue_to_value(c: &CValue) -> Value {
    match c.tag {
        tag::BOOL => Value::Bool(c.boolean != 0),
        tag::INT => Value::Int(c.integer),
        tag::FLOAT => Value::Float(c.float),
        // SAFETY: STR tag implies a valid `(ptr, len)` pair from the callee.
        tag::STR => Value::Str(Rc::new(unsafe { c.as_str() }.unwrap_or("").to_string())),
        _ => Value::Nil,
    }
}

// ─── Manifest parsing ───────────────────────────────────────────────────────

fn parse_manifest(text: &str) -> Result<Manifest, String> {
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
                    let mt = entry
                        .as_table()
                        .ok_or_else(|| format!("`exports.{class_name}.methods` entries must be tables"))?;
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
                        .ok_or_else(|| format!("`{class_name}.{mname}` is missing `native_symbol`"))?
                        .to_string();
                    let (params, returns) = parse_sig(sig)
                        .map_err(|e| format!("`{class_name}.{mname}` has an invalid sig: {e}"))?;
                    methods.push(MethodSpec {
                        name: mname,
                        symbol,
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

/// Parse a `fn(a: T, b: U) -> R` signature string into `(params, returns)`
/// using the typeck type builders. Parameter *names* are ignored; only the
/// types matter. A `nil` (or absent) return becomes `[nil]`; a parenthesised
/// `(A, B)` return becomes a multi-return.
fn parse_sig(sig: &str) -> Result<(Vec<Type>, Vec<Type>), String> {
    let s = sig.trim();
    let s = s.strip_prefix("fn").unwrap_or(s).trim_start();

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
    let params = split_top_level(params_str)
        .iter()
        .map(|p| parse_type(param_type(p)))
        .collect();

    let rest = s[close + 1..].trim();
    let ret_str = rest.strip_prefix("->").map(str::trim).unwrap_or("");
    let returns = parse_return(ret_str);

    Ok((params, returns))
}

/// Extract the type portion of a `name: type` parameter (or the whole token
/// if there's no name).
fn param_type(p: &str) -> &str {
    match p.split_once(':') {
        Some((_, ty)) => ty.trim(),
        None => p.trim(),
    }
}

fn parse_return(ret: &str) -> Vec<Type> {
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

/// Parse a single type token: a trailing `?` makes it nullable.
fn parse_type(tok: &str) -> Type {
    let t = tok.trim();
    match t.strip_suffix('?') {
        Some(base) => saule_typeck::sigs::t_nullable(saule_typeck::sigs::t_named(base.trim())),
        None => saule_typeck::sigs::t_named(t),
    }
}

/// Split on commas that are not nested inside `<...>` or `(...)`. Empty
/// segments (e.g. an empty parameter list) are dropped.
fn split_top_level(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for ch in s.chars() {
        match ch {
            '<' | '(' => {
                depth += 1;
                cur.push(ch);
            }
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
    }
    let t = cur.trim();
    if !t.is_empty() {
        out.push(t.to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_manifest() {
        let text = r#"
            [package]
            name = "engine"
            version = "0.1.0"
            binary = "engine.so, engine.dll, engine.dylib"

            [exports.Graphics]
            type = "class"
            doc = "2D graphics"

              [[exports.Graphics.methods]]
              name = "circle"
              sig = "fn(mode: string, x: float, y: float, radius: float) -> nil"
              native_symbol = "saule_engine_graphics_circle"
        "#;
        let m = parse_manifest(text).expect("manifest should parse");
        assert_eq!(m.name, "engine");
        assert_eq!(m.binaries, ["engine.so", "engine.dll", "engine.dylib"]);
        assert_eq!(m.exports.len(), 1);
        let g = &m.exports[0];
        assert_eq!(g.name, "Graphics");
        assert_eq!(g.methods.len(), 1);
        assert_eq!(g.methods[0].name, "circle");
        assert_eq!(g.methods[0].symbol, "saule_engine_graphics_circle");
        assert_eq!(g.methods[0].params.len(), 4);
        assert_eq!(g.methods[0].returns.len(), 1);
    }

    #[test]
    fn splits_nested_commas() {
        let parts = split_top_level("a: table<K, V>, b: float");
        assert_eq!(parts, ["a: table<K, V>", "b: float"]);
    }

    #[test]
    fn parses_tuple_return() {
        let (_p, r) = parse_sig("fn() -> (integer, integer)").unwrap();
        assert_eq!(r.len(), 2);
    }
}
