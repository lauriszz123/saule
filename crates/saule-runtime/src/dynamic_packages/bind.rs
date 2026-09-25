//! Loading a package's shared library and binding its exports into
//! interpreter values — classes, objects, enums and native closures.
//!
//! Everything a program can name from a package is built from its
//! [`Manifest`] alone ([`build_exports_deferred`]): the library is loaded at
//! the `import` ([`preload`]), and each native closure resolves its symbol
//! on first call. Objects and enum values are made of `Rc`s, so the values
//! built for a package live in a [`PackageRt`] of which each thread has one
//! per package — which is also what makes `BlendMode.Add` returned by a
//! native the *same* variant a program wrote, since variants compare by
//! identity.

use crate::error::RuntimeError;
use crate::module::ModuleExports;

// Everything below is reachable only from the library-loading half of this
// module. It is gated with the code that uses it, or a build without the
// feature (wasm, chiefly) fails to resolve `libloading` and warns on the rest.
#[cfg(feature = "native-packages")]
use {
    crate::fxhash::{FxHashMap, fxmap},
    crate::native_host::{self, PackageCtx},
    crate::value::{
        ClassObject, EnumObject, EnumVariantObject, ForeignClass, NativeClosure, SauleStr, Value,
        foreign::NATIVE_CONSTRUCTOR,
    },
    libloading::Library,
    saule_ast::Type,
    saule_native_abi::{CValue, NativeSymbolFn, ObjectReleaseFn},
    std::cell::{Cell, RefCell},
    std::collections::HashMap,
    std::path::Path,
    std::rc::{Rc, Weak},
    std::sync::Arc,
};

use super::*;

// ─── The per-thread package runtime ─────────────────────────────────────────

/// The values built for one package on one thread: its exports, its enums,
/// and the context object conversions need. Built on first use and kept for
/// the life of the thread, so every import of the package shares them.
#[cfg(feature = "native-packages")]
pub(crate) struct PackageRt {
    manifest: Arc<Manifest>,
    ctx: Rc<PackageCtx>,
    enums: FxHashMap<String, Rc<EnumObject>>,
    exports: std::collections::HashMap<String, Value>,
}

#[cfg(feature = "native-packages")]
thread_local! {
    static RUNTIMES: RefCell<HashMap<String, Rc<PackageRt>>> = RefCell::new(HashMap::new());
}

/// This thread's runtime for package `name`, building it on first use.
#[cfg(feature = "native-packages")]
fn runtime(name: &str) -> Option<Rc<PackageRt>> {
    if let Some(rt) = RUNTIMES.with(|r| r.borrow().get(name).cloned()) {
        return Some(rt);
    }
    let manifest = lookup(name)?;
    // The closures inside hold the runtime weakly: it owns them, and a strong
    // reference back would be a cycle.
    let rt = Rc::new_cyclic(|weak| build_runtime(manifest, weak));
    RUNTIMES.with(|r| r.borrow_mut().insert(name.to_string(), rt.clone()));
    Some(rt)
}

#[cfg(feature = "native-packages")]
fn build_runtime(manifest: Arc<Manifest>, weak: &Weak<PackageRt>) -> PackageRt {
    let package: Rc<str> = Rc::from(manifest.name.as_str());

    let mut enums = fxmap();
    for spec in &manifest.enums {
        enums.insert(spec.name.clone(), build_enum(spec));
    }

    // Objects' side of each `#[saule_class]`: its instance members.
    let mut classes = fxmap();
    for spec in manifest.exports.iter().filter(|c| c.instantiable) {
        let mut methods = fxmap();
        let mut getters = fxmap();
        let mut setters = fxmap();
        for m in &spec.methods {
            let table = match m.receiver {
                Receiver::Instance => &mut methods,
                Receiver::Getter => &mut getters,
                Receiver::Setter => &mut setters,
                Receiver::Static | Receiver::Constructor => continue,
            };
            table.insert(m.name.clone(), make_native(weak, &spec.name, m));
        }
        classes.insert(
            spec.name.clone(),
            Rc::new(ForeignClass {
                name: spec.name.clone(),
                package: package.clone(),
                methods,
                getters,
                setters,
            }),
        );
    }

    // The side a program names: `Graphics.circle(…)`, `Image(…)`,
    // `Image.load(…)`, `BlendMode.Add`.
    let mut exports = std::collections::HashMap::new();
    for spec in &manifest.exports {
        let mut statics = fxmap();
        for m in spec.members(Receiver::Static) {
            statics.insert(m.name.clone(), make_native(weak, &spec.name, m));
        }
        if let Some(ctor) = spec.constructor() {
            statics.insert(NATIVE_CONSTRUCTOR.to_string(), make_native(weak, &spec.name, ctor));
        }
        exports.insert(
            spec.name.clone(),
            Value::Class(Rc::new(ClassObject {
                name: spec.name.clone(),
                parent: None,
                // A native class's objects live in the package; nothing about
                // them is laid out here.
                layout: Default::default(),
                methods: Default::default(),
                static_fields: RefCell::new(statics),
                slot_statics: None,
                static_methods: Default::default(),
            })),
        );
    }
    for (name, e) in &enums {
        exports.insert(name.clone(), Value::Enum(e.clone()));
    }

    PackageRt {
        ctx: Rc::new(PackageCtx {
            name: package,
            classes,
            release: Cell::new(None),
        }),
        manifest,
        enums,
        exports,
    }
}

/// A package enum as a runtime value. Each variant's value is its own name,
/// which is also how it crosses the boundary.
#[cfg(feature = "native-packages")]
fn build_enum(spec: &EnumSpec) -> Rc<EnumObject> {
    let mut variants = fxmap();
    let mut by_tag = Vec::with_capacity(spec.variants.len());
    let mut tags = fxmap();
    for (tag, vname) in spec.variants.iter().enumerate() {
        let variant = Rc::new(EnumVariantObject {
            enum_name: SauleStr::new(spec.name.clone()),
            variant_name: SauleStr::new(vname.clone()),
            tag: tag as u32,
            value: std::cell::OnceCell::from(Value::Str(SauleStr::new(vname.clone()))),
            enum_obj: RefCell::new(None),
        });
        variants.insert(vname.clone(), variant.clone());
        by_tag.push(Some(variant));
        tags.insert(vname.clone(), tag as u32);
    }
    let e = Rc::new(EnumObject {
        name: spec.name.clone(),
        variants: variants.clone(),
        by_tag,
        tags,
        tuple_variants: Default::default(),
        methods: Default::default(),
    });
    for v in variants.values() {
        *v.enum_obj.borrow_mut() = Some(e.clone());
    }
    e
}

// ─── Exports ────────────────────────────────────────────────────────────────

/// Build the importable surface of a dynamic package, loading its shared
/// library now. The program driver folds exports through
/// [`build_exports_deferred`] instead, and reaches this only for the error
/// it gives when a package cannot be loaded at all.
#[cfg(feature = "native-packages")]
pub fn build_exports(
    name: &str,
    import_span: std::ops::Range<usize>,
) -> Result<ModuleExports, RuntimeError> {
    preload(name, import_span.clone())?;
    build_exports_deferred(name).ok_or_else(|| RuntimeError::ImportError {
        message: format!("native package `{name}` is no longer registered"),
        span: import_span,
    })
}

/// Build the importable surface of a dynamic package from its **metadata
/// alone**, with every native deferring its symbol lookup to the first call.
///
/// This is what lets the bytecode compiler fold a dynamic package's exports
/// into constants the way it already folds a static one's. The metadata
/// carries every name, symbol and arity the compiler needs and was read at
/// [`discover`] time, so building this surface loads nothing: no `dlopen`,
/// no symbol resolution, no side effect a *compile* must not have.
///
/// The library is still loaded before any of these closures can run —
/// `saule-vm`'s `run_program` calls [`preload`] immediately before the body
/// of the module that imported it. The lazy resolve inside each closure is
/// therefore a cache hit in practice; it is written to work anyway so that a
/// closure which somehow outlives its preload fails with a diagnostic rather
/// than a dangling pointer.
///
/// `None` when `name` is not a discovered package, or on a build with no
/// dynamic loading at all — callers report [`build_exports`]' error.
#[cfg(feature = "native-packages")]
pub fn build_exports_deferred(name: &str) -> Option<ModuleExports> {
    let rt = runtime(name)?;
    Some(ModuleExports {
        values: rt.exports.clone(),
    })
}

/// See the `native-packages` version. Without dynamic loading there is no
/// surface to defer to, so callers report [`build_exports`]' error.
#[cfg(not(feature = "native-packages"))]
pub fn build_exports_deferred(_name: &str) -> Option<ModuleExports> {
    None
}

/// Load a package's shared library and check that everything its metadata
/// names resolves — the side-effecting half of an `import`, without building
/// any values.
///
/// Used by `saule-vm`, which folds a package's exports at compile time via
/// [`build_exports_deferred`] and needs the load itself to happen at *run*
/// time, at the `import`. Checking every symbol (not just the ones the
/// importing module names) is what makes a broken package fail at its
/// `import`, not at the first call that happens to reach a missing symbol.
#[cfg(feature = "native-packages")]
pub fn preload(name: &str, import_span: std::ops::Range<usize>) -> Result<(), RuntimeError> {
    let err = |message: String| RuntimeError::ImportError {
        message,
        span: import_span.clone(),
    };
    let rt = runtime(name)
        .ok_or_else(|| err(format!("native package `{name}` is no longer registered")))?;
    let lib = bind_library(&rt).map_err(err)?;
    for class in &rt.manifest.exports {
        for method in &class.methods {
            resolve_symbol(&lib, &method.symbol).map_err(err)?;
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
/// A package's *metadata* is still discovered and its type signatures still
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

/// Is `name`'s library loaded in this process? For tests that prove a
/// compile never loads one.
#[cfg(feature = "native-packages")]
pub fn is_loaded(name: &str) -> bool {
    LIBS.read()
        .expect("dynamic lib cache poisoned")
        .as_ref()
        .is_some_and(|m| m.contains_key(name))
}

// ─── Type information for the checker and the editor ────────────────────────

/// Build semantic class metadata for a dynamic package's exported classes,
/// applying the importing statement's aliases. Mirrors how *static* native
/// packages contribute to a [`saule_semantic::ModuleSeed`] so the semantic
/// analyzer (and therefore the LSP) knows `Graphics` and `Image` as classes —
/// `Image` with its constructor, methods and properties — instead of
/// flagging them as undefined.
///
/// Returns an empty vec if `name` isn't a discovered dynamic package.
pub fn seed_classes(
    name: &str,
    names: &saule_ast::ImportNames,
) -> Vec<(String, saule_semantic::ClassInfo)> {
    let Some(manifest) = lookup(name) else {
        return Vec::new();
    };
    manifest
        .exports
        .iter()
        .filter_map(|class| Some((local_name(&class.name, names)?, class_info(class))))
        .collect()
}

/// The enums a dynamic package exports, for the same seed as
/// [`seed_classes`]. What makes `local m: BlendMode` a type the checker
/// knows, and `BlendMode.Ad` an unknown variant.
pub fn seed_enums(
    name: &str,
    names: &saule_ast::ImportNames,
) -> Vec<(String, saule_semantic::EnumInfo)> {
    let Some(manifest) = lookup(name) else {
        return Vec::new();
    };
    manifest
        .enums
        .iter()
        .filter_map(|e| {
            let mut info = saule_semantic::EnumInfo::default();
            for v in &e.variants {
                info.add_variant(v.clone(), saule_semantic::VariantInfo::default());
            }
            Some((local_name(&e.name, names)?, info))
        })
        .collect()
}

/// The name an export is bound to by an import: itself for `import *`, its
/// alias (or itself) when listed, `None` when not imported.
fn local_name(export: &str, names: &saule_ast::ImportNames) -> Option<String> {
    match names {
        saule_ast::ImportNames::All => Some(export.to_string()),
        saule_ast::ImportNames::List(items) => items
            .iter()
            .find(|(orig, _)| orig == export)
            .map(|(orig, alias)| alias.clone().unwrap_or_else(|| orig.clone())),
    }
}

pub(crate) fn class_info(class: &ClassSpec) -> saule_semantic::ClassInfo {
    let mut info = saule_semantic::ClassInfo::default();
    for m in &class.methods {
        info.members.insert(m.name.clone(), false); // public
        match m.receiver {
            // A property is a field as far as the checker is concerned:
            // `img.width` reads an `integer`, `img.width = 3` writes one.
            Receiver::Getter => {
                if let Some(ty) = m.returns.first() {
                    info.field_types.insert(m.name.clone(), ty.clone());
                }
                continue;
            }
            Receiver::Setter => {
                if let Some(ty) = m.params.first() {
                    info.field_types
                        .entry(m.name.clone())
                        .or_insert_with(|| ty.clone());
                }
                continue;
            }
            _ => {}
        }
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
        let return_ty = match m.receiver {
            // `init` returns nothing, as a Saule constructor does; the
            // checker knows `Image(…)` is an `Image` from the class itself.
            Receiver::Constructor => None,
            _ => match m.returns.as_slice() {
                [single] => Some(single.clone()),
                [] => None,
                // Multiple declared returns surface as a tuple so the type
                // checker can destructure `local a, b = Class.method()`.
                multi => Some(saule_ast::Type::Tuple(multi.to_vec())),
            },
        };
        info.methods.insert(
            m.name.clone(),
            saule_semantic::MethodSig {
                is_static: m.receiver == Receiver::Static,
                is_private: false,
                type_params: m.type_params.clone(),
                params,
                return_ty,
            },
        );
    }
    info
}

// ─── Loading ────────────────────────────────────────────────────────────────

/// Load a package's shared library (once per process) and make sure this
/// thread's runtime for it knows how to give objects back.
#[cfg(feature = "native-packages")]
fn bind_library(rt: &PackageRt) -> Result<Arc<Library>, String> {
    let lib = load_library(&rt.manifest)?;
    if rt.ctx.release.get().is_none() {
        rt.ctx.release.set(Some(release_symbol(&lib, &rt.manifest)?));
    }
    Ok(lib)
}

/// Load a package's shared library, once per process.
#[cfg(feature = "native-packages")]
pub(crate) fn load_library(manifest: &Manifest) -> Result<Arc<Library>, String> {
    if let Some(map) = LIBS.read().expect("dynamic lib cache poisoned").as_ref()
        && let Some(lib) = map.get(&manifest.name)
    {
        return Ok(lib.clone());
    }

    let path = &manifest.path;
    // SAFETY: loading arbitrary native code is inherently unsafe; the user
    // opted in by placing the binary under ~/.saule/native_packages.
    let lib = unsafe { Library::new(path) }
        .map_err(|e| format!("failed to load `{}`: {e}", path.display()))?;

    // Before anything else touches this library. Every later step — installing
    // the host table, resolving a method symbol, making a call — assumes the
    // package agrees with us about `CValue`, `HostApi` and the tag
    // discriminants, and a package that disagrees does not fail cleanly: it
    // reads a struct that is the wrong size or a tag it has never heard of,
    // and corrupts memory at the first call. So the version is the one thing
    // checked while nothing is at stake.
    check_abi_version(&lib, &manifest.name, path)?;

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

/// Check that a freshly-loaded library was compiled against the ABI this
/// interpreter speaks, before any other symbol is resolved or called.
///
/// Two distinct failures, because they need different advice:
///
/// * **The symbol is missing.** The library predates ABI versioning
///   entirely, so there is no version to compare and nothing to say about
///   which side is older — only that it must be rebuilt.
/// * **The version disagrees.** Both numbers are known, so both are named.
///   Which side is stale is the author's to work out, but the numbers make
///   it obvious in practice: a package older than the toolchain is the
///   common case, and the reverse means the toolchain needs updating.
#[cfg(feature = "native-packages")]
fn check_abi_version(lib: &Library, package: &str, path: &Path) -> Result<(), String> {
    // SAFETY: `AbiVersionFn` takes no arguments and returns a `u32`. That
    // signature is deliberately the least version-dependent thing in the
    // ABI — it passes no struct whose layout could have changed — so calling
    // it is sound even when everything else about this library disagrees
    // with us, which is precisely the case being tested for.
    let found = unsafe {
        let sym: libloading::Symbol<saule_native_abi::AbiVersionFn> = lib
            .get(saule_native_abi::ABI_VERSION_SYMBOL.as_bytes())
            .map_err(|_| {
                format!(
                    "native package `{package}` (`{}`) does not export `{}`, so it was \
                     built against a Saule ABI older than version {}. Rebuild it against \
                     the current `saule-sdk`.",
                    path.display(),
                    saule_native_abi::ABI_VERSION_SYMBOL,
                    saule_native_abi::ABI_VERSION,
                )
            })?;
        sym()
    };

    if found != saule_native_abi::ABI_VERSION {
        return Err(format!(
            "native package `{package}` (`{}`) was built against Saule native ABI \
             version {found}, but this toolchain speaks version {}. Rebuild the \
             package against a matching `saule-sdk`, or install a toolchain that \
             matches the package.",
            path.display(),
            saule_native_abi::ABI_VERSION,
        ));
    }
    Ok(())
}

/// The package's `saule_object_release`. Every package built with the SDK
/// exports one; a package with objects cannot work without it.
#[cfg(feature = "native-packages")]
fn release_symbol(lib: &Library, manifest: &Manifest) -> Result<ObjectReleaseFn, String> {
    // SAFETY: the symbol, when present, has the frozen `ObjectReleaseFn`
    // signature; the pointer is copied out and `LIBS` keeps the library
    // loaded for the life of the process.
    unsafe {
        lib.get::<ObjectReleaseFn>(saule_native_abi::OBJECT_RELEASE_SYMBOL.as_bytes())
            .map(|s| *s)
            .map_err(|_| {
                format!(
                    "native package `{}` (`{}`) does not export `{}`",
                    manifest.name,
                    manifest.path.display(),
                    saule_native_abi::OBJECT_RELEASE_SYMBOL
                )
            })
    }
}

/// Copy a symbol's function pointer out of `lib`.
#[cfg(feature = "native-packages")]
pub(crate) fn resolve_symbol(lib: &Library, symbol: &str) -> Result<NativeSymbolFn, String> {
    // SAFETY: the symbol must have the ABI's frozen signature; the macros that
    // exported it guarantee it, and the ABI check guarantees they are ours.
    // The pointer is copied out; what keeps it valid is that `LIBS` holds the
    // library for the life of the process.
    unsafe {
        let sym: libloading::Symbol<NativeSymbolFn> = lib
            .get(symbol.as_bytes())
            .map_err(|e| format!("symbol `{symbol}` not found: {e}"))?;
        Ok(*sym)
    }
}

// ─── Calling ────────────────────────────────────────────────────────────────

/// A closure calling one exported member. Nothing is loaded or resolved
/// here: the symbol is looked up on the first call and remembered.
///
/// This is what makes a dynamic package foldable at compile time. Building
/// the closure loads nothing, so a compile — `saule disasm`, a check, a run
/// that refuses later on for some other reason — never executes a line of
/// the package's code.
///
/// Members with a receiver take the object as argument 0, which is how
/// `call_method` and the property paths call them.
#[cfg(feature = "native-packages")]
fn make_native(rt: &Weak<PackageRt>, class: &str, spec: &MethodSpec) -> Value {
    let qname = match spec.receiver {
        Receiver::Constructor => class.to_string(),
        _ => format!("{class}.{}", spec.name),
    };
    // `NativeClosure::name` is `&'static str`; a package's members are bounded
    // and built once per thread, so leaking is acceptable.
    let name: &'static str = Box::leak(qname.into_boxed_str());
    let rt = rt.clone();
    let symbol = spec.symbol.clone();
    let returns = spec.returns.clone();
    let resolved: Cell<Option<NativeSymbolFn>> = Cell::new(None);

    let func = Box::new(move |args: &[Value]| -> Result<Vec<Value>, String> {
        let rt = rt
            .upgrade()
            .ok_or_else(|| format!("{name}: its package's runtime is gone"))?;
        // Copied out rather than borrowed across the call: a package can
        // call back into Saule, and that call can reach this same closure.
        let raw = match resolved.get() {
            Some(raw) => raw,
            None => {
                let lib = bind_library(&rt)?;
                let raw = resolve_symbol(&lib, &symbol)?;
                resolved.set(Some(raw));
                raw
            }
        };
        let result = call_native(&rt, raw, args)?;
        let values = if returns.len() > 1 {
            spread_multi_return(result, returns.len())
        } else {
            vec![result]
        };
        Ok(values
            .into_iter()
            .zip(returns.iter())
            .map(|(v, ty)| rt.as_declared(v, ty))
            .collect())
    });

    Value::NativeClosure(Rc::new(NativeClosure {
        name,
        func,
        param_names: spec.param_names.clone(),
    }))
}

#[cfg(feature = "native-packages")]
impl PackageRt {
    /// Give a native's result the type its signature declares, where the
    /// wire form differs: a package enum arrives as its variant's name and
    /// becomes the variant — at the top level, inside `T?`, and in a table
    /// of them.
    fn as_declared(&self, v: Value, ty: &Type) -> Value {
        match (ty, v) {
            (Type::Named(n), Value::Str(s)) => match self.enums.get(n) {
                Some(e) => e
                    .variants
                    .get(&**s)
                    .map(|variant| Value::EnumVariant(variant.clone()))
                    .unwrap_or(Value::Str(s)),
                None => Value::Str(s),
            },
            (Type::Nullable(inner), v) => self.as_declared(v, inner),
            (Type::Table { value, .. }, Value::Table(t)) if self.mentions_enum(value) => {
                {
                    let mut t = t.borrow_mut();
                    for cell in t.array.iter_mut() {
                        let v = std::mem::replace(cell, Value::Nil);
                        *cell = self.as_declared(v, value);
                    }
                    for cell in t.map.values_mut() {
                        let v = std::mem::replace(cell, Value::Nil);
                        *cell = self.as_declared(v, value);
                    }
                }
                Value::Table(t)
            }
            (_, v) => v,
        }
    }

    fn mentions_enum(&self, ty: &Type) -> bool {
        match ty {
            Type::Named(n) => self.enums.contains_key(n),
            Type::Nullable(inner) => self.mentions_enum(inner),
            Type::Table { value, .. } => self.mentions_enum(value),
            _ => false,
        }
    }
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
/// The call is bracketed by [`native_host::enter`] / `exit`, so `table` /
/// function / object arguments (and values the package creates via the host
/// callbacks) live in the handle registry for the duration of the call, and
/// by [`native_host::enter_package`] / `exit_package`, so an object crossing
/// either way is checked against — and wrapped for — this package.
#[cfg(feature = "native-packages")]
fn call_native(rt: &PackageRt, raw: NativeSymbolFn, args: &[Value]) -> Result<Value, String> {
    native_host::enter();
    native_host::enter_package(rt.ctx.clone());
    let result = (|| {
        let mut cargs = Vec::with_capacity(args.len());
        for (i, v) in args.iter().enumerate() {
            cargs.push(native_host::value_to_cvalue(v).ok_or_else(|| {
                match native_host::refusal(v) {
                    Some(why) => format!("cannot pass argument #{}: {why}", i + 1),
                    None => format!(
                        "cannot pass argument #{} ({}) across the native boundary",
                        i + 1,
                        v.type_name()
                    ),
                }
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
        // Resolve any returned handle — and adopt any returned object —
        // *before* the scope is torn down.
        Ok(native_host::cvalue_to_value(&out))
    })();
    native_host::exit_package();
    native_host::exit();
    result
}
