//! Loading a package's shared library and binding its exports into
//! interpreter values — classes, methods, and native closures.

use crate::error::RuntimeError;
use crate::module::ModuleExports;

// Everything below is reachable only from the library-loading half of this
// module — see the block comment above `load_library`. It is gated with the
// code that uses it, or a build without the feature (wasm, chiefly) fails to
// resolve `libloading` and warns on the rest.
#[cfg(feature = "native-packages")]
use {
    crate::fxhash::fxmap,
    crate::value::{ClassObject, NativeClosure, Value},
    libloading::Library,
    saule_native_abi::{CValue, NativeSymbolFn},
    std::cell::RefCell,
    std::collections::HashMap,
    std::path::{Path, PathBuf},
    std::rc::Rc,
    std::sync::Arc,
};

use super::*;

/// Build the importable surface of a dynamic package, loading its shared
/// library on first use. Called by the module loader when it sees a dynamic
/// sentinel path.
#[cfg(feature = "native-packages")]
pub fn build_exports(
    name: &str,
    import_span: std::ops::Range<usize>,
) -> Result<ModuleExports, RuntimeError> {
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

/// Build the importable surface of a dynamic package from its **manifest
/// alone**, with every method deferring its symbol lookup to the first call.
///
/// This is what lets the bytecode compiler fold a dynamic package's exports
/// into constants the way it already folds a static one's. The manifest
/// carries every name, symbol and arity the compiler needs and is parsed at
/// [`discover`] time, so building this surface loads nothing: no `dlopen`,
/// no symbol resolution, no side effect a *compile* must not have.
///
/// The library is still loaded before any of these closures can run —
/// `saule-vm`'s `run_program` calls [`preload`] immediately before the body
/// of the module that imported it, which is where the tree-walker resolves
/// the same `import`. The lazy resolve inside each closure is therefore a
/// cache hit in practice; it is written to work anyway so that a closure
/// which somehow outlives its preload fails with a diagnostic rather than a
/// dangling pointer.
///
/// `None` when `name` is not a discovered package, or on a build with no
/// dynamic loading at all — callers refuse and fall back.
#[cfg(feature = "native-packages")]
pub fn build_exports_deferred(name: &str) -> Option<ModuleExports> {
    let manifest = lookup(name)?;
    let mut exports = ModuleExports::default();
    for class in &manifest.exports {
        let class_obj = build_class_deferred(class, name);
        exports
            .values
            .insert(class.name.clone(), Value::Class(Rc::new(class_obj)));
    }
    Some(exports)
}

/// See the `native-packages` version. Without dynamic loading there is no
/// surface to defer to, so callers refuse and let the tree-walker report.
#[cfg(not(feature = "native-packages"))]
pub fn build_exports_deferred(_name: &str) -> Option<ModuleExports> {
    None
}

/// Load a package's shared library and check that every symbol its manifest
/// names resolves — the side-effecting half of [`build_exports`], without
/// building any values.
///
/// Used by `saule-vm`, which folds a package's exports at compile time via
/// [`build_exports_deferred`] and needs the load itself to happen at *run*
/// time, at the point the tree-walker would have done it. Checking every
/// symbol (not just the ones the importing module names — the same set
/// [`build_exports`] resolves) is what makes a broken package fail
/// identically under both engines.
#[cfg(feature = "native-packages")]
pub fn preload(name: &str, import_span: std::ops::Range<usize>) -> Result<(), RuntimeError> {
    let manifest = lookup(name).ok_or_else(|| RuntimeError::ImportError {
        message: format!("native package `{name}` is no longer registered"),
        span: import_span.clone(),
    })?;

    let lib = load_library(&manifest).map_err(|message| RuntimeError::ImportError {
        message,
        span: import_span.clone(),
    })?;

    for class in &manifest.exports {
        for method in &class.methods {
            resolve_symbol(&lib, &method.symbol).map_err(|message| RuntimeError::ImportError {
                message,
                span: import_span.clone(),
            })?;
        }
    }
    Ok(())
}

/// Stand-in for builds without the `native-packages` feature. Defers to
/// [`build_exports`] so the "cannot be loaded in this build" wording is
/// written once.
#[cfg(not(feature = "native-packages"))]
pub fn preload(name: &str, import_span: std::ops::Range<usize>) -> Result<(), RuntimeError> {
    build_exports(name, import_span).map(|_| ())
}

/// Stand-in for builds without the `native-packages` feature — wasm, chiefly.
///
/// A package's *manifest* is still discovered and its type signatures still
/// register, so a program that imports one type-checks the same way it does
/// natively. It just cannot be run, and says so plainly rather than failing
/// later with a confusing missing-symbol error.
#[cfg(not(feature = "native-packages"))]
pub fn build_exports(
    name: &str,
    import_span: std::ops::Range<usize>,
) -> Result<ModuleExports, RuntimeError> {
    Err(RuntimeError::ImportError {
        message: format!(
            "native package `{name}` cannot be loaded in this build: \
             it needs a dynamically-loadable library, which this target \
             does not support"
        ),
        span: import_span,
    })
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
pub(crate) fn class_info(class: &ClassSpec) -> saule_semantic::ClassInfo {
    let mut info = saule_semantic::ClassInfo::default();
    for m in &class.methods {
        info.members.insert(m.name.clone(), false); // public
        let params = m
            .params
            .iter()
            .enumerate()
            .map(|(i, ty)| saule_ast::Param {
                name: m
                    .param_names
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| format!("arg{i}")),
                ty: ty.clone(),
                default: None,
                variadic: false,
                span: 0..0,
            })
            .collect();
        let return_ty = match m.returns.as_slice() {
            [single] => Some(single.clone()),
            [] => None,
            // Multiple declared returns surface as a tuple so the type
            // checker can destructure `local a, b = Class.method()`.
            multi => Some(saule_ast::Type::Tuple(multi.to_vec())),
        };
        info.methods.insert(
            m.name.clone(),
            saule_semantic::MethodSig {
                is_static: true,
                is_private: false,
                type_params: m.type_params.clone(),
                params,
                return_ty,
            },
        );
    }
    info
}

// ─── Library loading ────────────────────────────────────────────────────────
//
// Everything from here to the end of `call_native` exists only to dlopen a
// package and marshal calls across the C ABI, so it is gated as one block.
// The manifest half of this module above stays compiled on every target —
// `module.rs`, `stdlib/mod.rs` and the LSP all depend on it.

#[cfg(feature = "native-packages")]
pub(crate) fn load_library(manifest: &Manifest) -> Result<Arc<Library>, String> {
    if let Some(map) = LIBS.read().expect("dynamic lib cache poisoned").as_ref()
        && let Some(lib) = map.get(&manifest.name)
    {
        return Ok(lib.clone());
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

    // Hand the package its host-callback table so it can manipulate
    // host-owned `table` / function values by handle. No-op for packages
    // that only deal in scalars (the symbol is optional).
    // SAFETY: the library was just loaded for a native package; the symbol,
    // if present, has the frozen `SetHostFn` signature.
    unsafe { crate::native_host::install_host(&lib) };

    LIBS.write()
        .expect("dynamic lib cache poisoned")
        .get_or_insert_with(HashMap::new)
        .insert(manifest.name.clone(), lib.clone());
    Ok(lib)
}

/// Choose the binary to load: prefer the file whose extension matches the
/// host OS, otherwise fall back to the first listed candidate that exists.
#[cfg(feature = "native-packages")]
pub(crate) fn pick_binary(dir: &Path, candidates: &[String]) -> Option<PathBuf> {
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

#[cfg(feature = "native-packages")]
pub(crate) fn build_class(spec: &ClassSpec, lib: &Arc<Library>) -> Result<ClassObject, String> {
    let mut static_fields = fxmap();
    for method in &spec.methods {
        let qname = format!("{}.{}", spec.name, method.name);
        let value = make_native(
            qname,
            lib.clone(),
            &method.symbol,
            method.param_names.clone(),
            method.returns.len(),
        )?;
        static_fields.insert(method.name.clone(), value);
    }
    Ok(class_object(spec, static_fields))
}

/// [`build_class`] with no library in hand: each method resolves its symbol
/// on first call instead of now. Used by the bytecode compiler, which folds
/// a package's exports into constants and must not `dlopen` while doing it.
///
/// Infallible by construction — nothing here can fail, because nothing here
/// touches the binary. Every failure the eager path reports at import time
/// is reported by [`preload`] instead, at the same point in the program.
#[cfg(feature = "native-packages")]
pub(crate) fn build_class_deferred(spec: &ClassSpec, package: &str) -> ClassObject {
    let mut static_fields = fxmap();
    for method in &spec.methods {
        let value = make_native_deferred(
            format!("{}.{}", spec.name, method.name),
            package.to_string(),
            method.symbol.clone(),
            method.param_names.clone(),
            method.returns.len(),
        );
        static_fields.insert(method.name.clone(), value);
    }
    class_object(spec, static_fields)
}

/// The [`ClassObject`] shape both builders produce, written once.
#[cfg(feature = "native-packages")]
fn class_object(
    spec: &ClassSpec,
    static_fields: crate::fxhash::FxHashMap<String, Value>,
) -> ClassObject {
    ClassObject {
        name: spec.name.clone(),
        parent: None,
        field_defs: Vec::new(),
        // A native package exposes statics only — it is never instantiated,
        // so there are no instance slots to lay out.
        layout: Default::default(),
        methods: Default::default(),
        static_fields: RefCell::new(static_fields),
        static_methods: Default::default(),
        constructor: None,
    }
}

/// Resolve `symbol` in `lib` and wrap it in a [`Value::NativeClosure`] that
/// marshals Saule values across the ABI on every call. The closure captures
/// an `Arc<Library>` so the resolved function pointer stays valid.
///
/// `param_names` lets callers pass arguments by name (`Window.create(800,
/// 600, title: "x")`); the call site reorders them into positional slots.
/// `ret_arity` is the number of declared return values: when it exceeds one
/// the native packs them into a host array-`table`, which this closure
/// spreads back into a multi-value result.
#[cfg(feature = "native-packages")]
pub(crate) fn make_native(
    qname: String,
    lib: Arc<Library>,
    symbol: &str,
    param_names: Vec<String>,
    ret_arity: usize,
) -> Result<Value, String> {
    let raw = resolve_symbol(&lib, symbol)?;

    // `NativeClosure::name` is `&'static str`; method names are bounded and
    // loaded once, so leaking is acceptable.
    let name: &'static str = Box::leak(qname.into_boxed_str());

    let func = Box::new(move |args: &[Value]| -> Result<Vec<Value>, String> {
        // Keep the library alive for the duration of the call (and for as
        // long as this closure lives).
        let _keep = &lib;
        let result = call_native(raw, args)?;
        Ok(if ret_arity > 1 {
            spread_multi_return(result, ret_arity)
        } else {
            vec![result]
        })
    });

    Ok(Value::NativeClosure(Rc::new(NativeClosure {
        name,
        func,
        param_names,
    })))
}

/// Copy a symbol's function pointer out of `lib`.
///
/// Shared by the eager and deferred binders so a missing symbol reports the
/// same message wherever it is noticed.
#[cfg(feature = "native-packages")]
pub(crate) fn resolve_symbol(lib: &Library, symbol: &str) -> Result<NativeSymbolFn, String> {
    // SAFETY: the symbol must have the ABI's frozen signature; a mismatch is
    // the package author's bug. The pointer is copied out; what keeps it
    // valid is that `LIBS` holds the library for the life of the process,
    // and that every caller also captures an `Arc` to it.
    unsafe {
        let sym: libloading::Symbol<NativeSymbolFn> = lib
            .get(symbol.as_bytes())
            .map_err(|e| format!("symbol `{symbol}` not found: {e}"))?;
        Ok(*sym)
    }
}

/// [`make_native`] with no library in hand: the package name and symbol are
/// captured, and the `dlopen` plus symbol lookup happen on the first call
/// and are then remembered.
///
/// This is what makes a dynamic package foldable at compile time. Building
/// the closure loads nothing, so a compile — `saule disasm`, a check, a run
/// that refuses later on for some other reason — never executes a line of
/// the package's code.
#[cfg(feature = "native-packages")]
pub(crate) fn make_native_deferred(
    qname: String,
    package: String,
    symbol: String,
    param_names: Vec<String>,
    ret_arity: usize,
) -> Value {
    // `NativeClosure::name` is `&'static str`; see `make_native`.
    let name: &'static str = Box::leak(qname.into_boxed_str());

    // The `Arc` is kept alongside the pointer for the same reason
    // `make_native` keeps one: it states the lifetime the pointer depends
    // on rather than relying on `LIBS` never being cleared.
    let resolved: RefCell<Option<(Arc<Library>, NativeSymbolFn)>> = RefCell::new(None);

    let func = Box::new(move |args: &[Value]| -> Result<Vec<Value>, String> {
        // Copy the pointer out and drop the borrow *before* calling. A
        // package can call back into Saule through the host callbacks
        // (`native_host`), and that call can reach this same closure —
        // holding the `RefCell` across the call would turn ordinary
        // re-entrancy into a panic.
        let raw = {
            let mut slot = resolved.borrow_mut();
            if slot.is_none() {
                let manifest = lookup(&package).ok_or_else(|| {
                    format!("native package `{package}` is no longer registered")
                })?;
                let lib = load_library(&manifest)?;
                let raw = resolve_symbol(&lib, &symbol)?;
                *slot = Some((lib, raw));
            }
            slot.as_ref().expect("just installed").1
        };

        let result = call_native(raw, args)?;
        Ok(if ret_arity > 1 {
            spread_multi_return(result, ret_arity)
        } else {
            vec![result]
        })
    });

    Value::NativeClosure(Rc::new(NativeClosure {
        name,
        func,
        param_names,
    }))
}

/// Spread a multi-return native's result into `arity` values. The native
/// encodes its returns as a host array-`table` (the single-valued ABI can't
/// carry several values directly); the first `arity` array slots become the
/// result tuple. A non-table result (a misbehaving package) degrades to that
/// value followed by `nil`s.
#[cfg(feature = "native-packages")]
pub(crate) fn spread_multi_return(value: Value, arity: usize) -> Vec<Value> {
    match value {
        Value::Table(t) => {
            let t = t.borrow();
            (1..=arity as i64).map(|i| t.get(&Value::Int(i))).collect()
        }
        other => {
            let mut out = Vec::with_capacity(arity);
            out.push(other);
            out.resize(arity, Value::Nil);
            out
        }
    }
}

/// Marshal `args` into [`CValue`]s, invoke `raw`, and translate the result
/// back into a [`Value`].
///
/// The call is bracketed by [`crate::native_host::enter`] / `exit` so any
/// `table` / function arguments (and values the package creates via the host
/// callbacks) live in the handle registry for the duration of the call.
#[cfg(feature = "native-packages")]
pub(crate) fn call_native(raw: NativeSymbolFn, args: &[Value]) -> Result<Value, String> {
    use crate::native_host;

    native_host::enter();
    let result = (|| {
        let mut cargs = Vec::with_capacity(args.len());
        for (i, v) in args.iter().enumerate() {
            cargs.push(native_host::value_to_cvalue(v).ok_or_else(|| {
                format!(
                    "cannot pass argument #{} ({}) across the native boundary",
                    i + 1,
                    v.type_name()
                )
            })?);
        }

        let mut out = CValue::nil();
        // SAFETY: `cargs` is a valid, contiguous, initialised slice; `out` is a
        // valid writable slot. String payloads in `cargs` borrow from `args`,
        // which outlives the call.
        let code = unsafe { raw(cargs.as_ptr(), cargs.len(), &mut out) };

        if code != 0 {
            // SAFETY: on failure the callee writes an ERR/STR value into `out`.
            let msg = unsafe { out.as_str() }
                .unwrap_or("native package call failed")
                .to_string();
            return Err(msg);
        }
        // Resolve any returned handle *before* the scope is torn down.
        Ok(native_host::cvalue_to_value(&out))
    })();
    native_host::exit();
    result
}

// ─── Manifest parsing ───────────────────────────────────────────────────────
