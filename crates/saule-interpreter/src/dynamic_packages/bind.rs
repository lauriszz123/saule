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
    Ok(ClassObject {
        name: spec.name.clone(),
        parent: None,
        field_defs: Vec::new(),
        methods: Default::default(),
        static_fields: RefCell::new(static_fields),
        static_methods: Default::default(),
        constructor: None,
    })
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
