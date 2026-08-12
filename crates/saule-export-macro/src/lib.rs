//! `#[saule_export]` — turn a safe Rust function into a Saule native-package
//! export. Annotate the function with the owning class and method name; the
//! macro infers the Saule type signature from the parameter and return types,
//! generates the `extern "C"` ABI shim, and registers the method in the
//! manifest registry.
//!
//! ```ignore
//! #[saule_export(class = "Window", name = "create")]
//! fn window_create(width: i64, height: i64, title: Option<String>) -> Result<(), String> {
//!     /* ... */
//!     Ok(())
//! }
//! ```
//!
//! From the signature above the macro derives
//! `fn(width: integer, height: integer, title: string?) -> nil`, exports a C
//! symbol that decodes the arguments and calls `window_create`, and submits a
//! manifest entry. The original function is left intact (and unit-testable).
//!
//! ## Type mapping
//! `i64 → integer`, `f64 → float`, `bool → boolean`, `String → string`,
//! `SValue → any`, `Option<T> → T?`, `() → nil`. The return type may be `T`,
//! `()`, `Result<T, E>` (an `Err` surfaces as a Saule runtime error), or a
//! tuple `(A, B, …)` for a multi-value return.
//!
//! ## Multi-return
//! A tuple return type renders as a Saule multi-return signature, e.g.
//! `fn divmod(a: i64, b: i64) -> (i64, i64)` becomes
//! `fn(a: integer, b: integer) -> (integer, integer)`. The values are packed
//! into a host table across the single-valued ABI and spread back into
//! separate bindings at the call site (`local q, r = Util.divmod(17, 5)`).
//! Tuples are accepted only in return position; parameters stay scalar.
//!
//! ## Callbacks
//! `SFunction` is an opaque handle: it carries no arity and no parameter
//! types, and Saule has no bare `function` type to erase them into. So a
//! callback parameter has to declare the signature calls are checked
//! against, via `sig(<param> = "…")`:
//!
//! ```ignore
//! #[saule_export(class = "Util", name = "map", sig(f = "fn(T) -> T"))]
//! fn util_map(t: STable<T>, f: SFunction) -> Result<STable<T>, String> { /* ... */ }
//! ```
//!
//! The key is the Rust parameter's name; `sig(return = "…")` types an
//! `SFunction` in return position. An `Option<SFunction>` wraps its declared
//! signature in parentheses — `(fn(T) -> T)?` — because `?` would otherwise
//! bind to the return type. Leaving the signature off is a compile error.
//!
//! ## Generics
//! The marker types `T`/`U`/`V`/`W` from `saule_sdk` are type variables:
//! `STable<T> → table<T>`, `SElem<T> → T`. A signature that mentions them
//! emits a `fn<T, …>(…)` prefix — including one written in `sig(...)` — so
//! e.g. `fn(t: STable<T>, f: SFunction) -> Option<SElem<T>>` with
//! `sig(f = "fn(T) -> boolean")` checks as
//! `fn<T>(t: table<T>, f: fn(T) -> boolean) -> T?`.
//!
//! Generated code references `::saule_sdk::__private::*`, so the annotated
//! crate must depend on `saule-sdk`.

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::{format_ident, quote};
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{
    Expr, ExprLit, FnArg, GenericArgument, ItemFn, Lit, Meta, MetaNameValue, Pat, PathArguments,
    ReturnType, Token, Type,
};

/// See the module-level docs.
#[proc_macro_attribute]
pub fn saule_export(attr: TokenStream, item: TokenStream) -> TokenStream {
    match expand(attr, item) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn expand(attr: TokenStream, item: TokenStream) -> Result<proc_macro2::TokenStream, syn::Error> {
    let func: ItemFn = syn::parse(item)?;
    let (class, name, sig_overrides) = parse_attr(attr)?;
    let sig_for = |slot: &str| -> Option<&str> {
        sig_overrides
            .iter()
            .find(|(k, _)| k == slot)
            .map(|(_, v)| v.as_str())
    };

    let fn_ident = func.sig.ident.clone();
    let qualified = format!("{class}.{name}");

    // ── Parameters: collect (ident, type, saule-type-string) ──────────────
    let mut param_idents = Vec::new();
    let mut param_types = Vec::new();
    let mut sig_params = Vec::new();
    let mut required = 0usize; // count of leading non-Option params
    let mut seen_optional = false;
    // Generic type-variable names referenced anywhere in the signature
    // (e.g. `T` from `STable<T>` / `SElem<T>`), in first-seen order. A
    // non-empty list emits a `fn<T, …>(…)` prefix.
    let mut tvars: Vec<String> = Vec::new();

    for input in &func.sig.inputs {
        let FnArg::Typed(pat_type) = input else {
            return Err(syn::Error::new_spanned(
                input,
                "#[saule_export] functions cannot take `self`",
            ));
        };
        let Pat::Ident(pat_ident) = &*pat_type.pat else {
            return Err(syn::Error::new_spanned(
                &pat_type.pat,
                "#[saule_export] parameters must be simple identifiers",
            ));
        };
        let pname = pat_ident.ident.to_string();
        let ty = (*pat_type.ty).clone();
        let (saule_ty, is_optional) = saule_type(&ty, &mut tvars, sig_for(&pname))?;

        if is_optional {
            seen_optional = true;
        } else {
            if seen_optional {
                return Err(syn::Error::new_spanned(
                    &pat_type.ty,
                    "required parameters cannot follow an optional (`Option<_>`) parameter",
                ));
            }
            required += 1;
        }

        sig_params.push(format!("{pname}: {saule_ty}"));
        param_idents.push(pat_ident.ident.clone());
        param_types.push(ty);
    }
    let total = func.sig.inputs.len();

    // ── Return type: unwrap Result, map value type, note if fallible ──────
    let (ret_value_ty, is_result) = unwrap_return(&func.sig.output);
    let ret_saule = match &ret_value_ty {
        Some(ty) => saule_return_type(ty, &mut tvars, sig_for("return"))?,
        None => "nil".to_string(),
    };

    // A `fn<T, U>(…)` prefix when the signature mentions type variables.
    let generics = if tvars.is_empty() {
        String::new()
    } else {
        format!("<{}>", tvars.join(", "))
    };
    let sig = format!("fn{generics}({}) -> {ret_saule}", sig_params.join(", "));

    // ── Names for the generated shim and its retention anchor ─────────────
    let shim_ident = format_ident!("saule_export_{}_{}", class, name);
    let shim_name = shim_ident.to_string();
    let anchor_ident = format_ident!("__SAULE_ANCHOR_{}_{}", class, name);

    // ── Arity check message ───────────────────────────────────────────────
    let arity_desc = if required == total {
        format!("{total}")
    } else {
        format!("{required} to {total}")
    };

    // ── Argument decoding ─────────────────────────────────────────────────
    let decodes =
        param_idents
            .iter()
            .zip(param_types.iter())
            .enumerate()
            .map(|(idx, (ident, ty))| {
                let pname = ident.to_string();
                quote! {
                    let #ident = <#ty as ::saule_sdk::__private::FromSaule>::from_saule(
                        __args, #idx, #qualified, #pname,
                    )?;
                }
            });

    // ── Call + encode the return value ────────────────────────────────────
    let call = quote! { #fn_ident( #( #param_idents ),* ) };
    let encode = if is_result {
        quote! {
            match #call {
                ::core::result::Result::Ok(__t) =>
                    ::core::result::Result::Ok(::saule_sdk::__private::IntoSaule::into_saule(__t)),
                ::core::result::Result::Err(__e) =>
                    ::core::result::Result::Err(::std::string::ToString::to_string(&__e)),
            }
        }
    } else {
        quote! {
            ::core::result::Result::Ok(::saule_sdk::__private::IntoSaule::into_saule(#call))
        }
    };

    Ok(quote! {
        #func

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn #shim_ident(
            __args_ptr: *const ::saule_sdk::__private::CValue,
            __argc: usize,
            __out: *mut ::saule_sdk::__private::CValue,
        ) -> i32 {
            let __args: &[::saule_sdk::__private::CValue] =
                if __args_ptr.is_null() || __argc == 0 {
                    &[]
                } else {
                    // SAFETY: the interpreter guarantees `__args_ptr` is valid
                    // for `__argc` initialised `CValue`s for the call.
                    unsafe { ::core::slice::from_raw_parts(__args_ptr, __argc) }
                };
            // SAFETY: the interpreter passes a valid, writable `out` pointer.
            let __out: &mut ::saule_sdk::__private::CValue = unsafe { &mut *__out };

            let __run = || -> ::core::result::Result<::saule_sdk::__private::CValue, ::std::string::String> {
                if __args.len() < #required || __args.len() > #total {
                    return ::core::result::Result::Err(::std::format!(
                        "{} expects {} argument(s), got {}",
                        #qualified, #arity_desc, __args.len(),
                    ));
                }
                #( #decodes )*
                #encode
            };

            match __run() {
                ::core::result::Result::Ok(__v) => { *__out = __v; 0 }
                ::core::result::Result::Err(__msg) => {
                    *__out = ::saule_sdk::__private::return_error(&__msg);
                    1
                }
            }
        }

        // Keep the shim (and the manifest submission in this object file) from
        // being culled by the linker when this crate is consumed as an rlib by
        // the manifest generator binary.
        #[used]
        static #anchor_ident: ::saule_sdk::__private::NativeSymbolFn = #shim_ident;

        ::saule_sdk::__private::inventory::submit! {
            ::saule_sdk::__private::ExportedMethod {
                class: #class,
                name: #name,
                sig: #sig,
                symbol: #shim_name,
            }
        }
    })
}

/// The `sig(...)` overrides, keyed by parameter name (plus the reserved key
/// `return` for the return slot).
type SigOverrides = Vec<(String, String)>;

/// Parse the `class = "..."`, `name = "..."`, `sig(param = "...")` attribute
/// arguments.
fn parse_attr(attr: TokenStream) -> Result<(String, String, SigOverrides), syn::Error> {
    let parser = Punctuated::<Meta, Token![,]>::parse_terminated;
    let args = parser.parse(attr)?;

    let mut class = None;
    let mut name = None;
    let mut sigs: SigOverrides = Vec::new();
    for meta in args {
        let key = meta
            .path()
            .get_ident()
            .map(|i| i.to_string())
            .unwrap_or_default();
        // `sig(f = "fn(T) -> T", g = "...")` — one entry per callback slot.
        if key == "sig" {
            let list = meta.require_list()?;
            let entries =
                list.parse_args_with(Punctuated::<MetaNameValue, Token![,]>::parse_terminated)?;
            for nv in entries {
                let slot = nv
                    .path
                    .get_ident()
                    .map(|i| i.to_string())
                    .unwrap_or_default();
                sigs.push((slot, str_value(&nv.value)?));
            }
            continue;
        }
        let nv = meta.require_name_value()?;
        let value = str_value(&nv.value)?;
        match key.as_str() {
            "class" => class = Some(value),
            "name" => name = Some(value),
            other => {
                return Err(syn::Error::new_spanned(
                    &nv.path,
                    format!(
                        "unknown `saule_export` key `{other}` (expected `class`, `name` or `sig`)"
                    ),
                ));
            }
        }
    }

    match (class, name) {
        (Some(c), Some(n)) => Ok((c, n, sigs)),
        _ => Err(syn::Error::new(
            Span::call_site(),
            "`saule_export` requires `class = \"...\"` and `name = \"...\"`",
        )),
    }
}

fn str_value(expr: &Expr) -> Result<String, syn::Error> {
    match expr {
        Expr::Lit(ExprLit {
            lit: Lit::Str(s), ..
        }) => Ok(s.value()),
        other => Err(syn::Error::new_spanned(other, "expected a string literal")),
    }
}

/// Map a return type to its Saule signature string. Unlike [`saule_type`],
/// this accepts a non-unit tuple `(A, B, …)` and renders it as a Saule
/// multi-return type `(a, b, …)` — the manifest parser turns that into a
/// `Type::Tuple` and the interpreter spreads the value across several
/// bindings. Element types still flow through [`saule_type`], so generic
/// markers and `Option<_>` work inside a tuple return.
fn saule_return_type(
    ty: &Type,
    tvars: &mut Vec<String>,
    fn_sig: Option<&str>,
) -> Result<String, syn::Error> {
    if let Type::Tuple(t) = ty {
        if t.elems.is_empty() {
            return Ok("nil".to_string());
        }
        let mut parts = Vec::with_capacity(t.elems.len());
        for elem in &t.elems {
            parts.push(saule_type(elem, tvars, fn_sig)?.0);
        }
        return Ok(format!("({})", parts.join(", ")));
    }
    Ok(saule_type(ty, tvars, fn_sig)?.0)
}

/// Map a Rust type to its Saule type string. Returns `(saule_type, is_optional)`
/// where `is_optional` is true for `Option<T>`.
///
/// `fn_sig` is the signature declared for this slot in `sig(...)`. It is the
/// only way an `SFunction` can be typed: the handle carries no arity and no
/// parameter types of its own, and Saule has no bare `function` type to fall
/// back on, so a callback parameter without one is a compile error.
fn saule_type(
    ty: &Type,
    tvars: &mut Vec<String>,
    fn_sig: Option<&str>,
) -> Result<(String, bool), syn::Error> {
    // The unit type `()` maps to `nil`.
    if let Type::Tuple(t) = ty {
        if t.elems.is_empty() {
            return Ok(("nil".to_string(), false));
        }
        return Err(syn::Error::new_spanned(
            ty,
            "tuple types are not supported by #[saule_export]",
        ));
    }

    // `&str` (and `&String`) map to `string`.
    if let Type::Reference(r) = ty {
        return saule_type(&r.elem, tvars, fn_sig);
    }

    let Type::Path(tp) = ty else {
        return Err(unsupported(ty));
    };
    let Some(seg) = tp.path.segments.last() else {
        return Err(unsupported(ty));
    };

    let ident = seg.ident.to_string();
    match ident.as_str() {
        "i64" | "i32" | "isize" | "u32" | "u64" | "usize" => Ok(("integer".to_string(), false)),
        "f64" | "f32" => Ok(("float".to_string(), false)),
        "bool" => Ok(("boolean".to_string(), false)),
        "String" | "str" => Ok(("string".to_string(), false)),
        // Saule-typed wrappers from `saule_sdk` (the `S*` bridge types). They
        // carry the same wire shape as their primitive, but give package
        // authors helper methods. `STable` / `SFunction` are host handles.
        "SInteger" => Ok(("integer".to_string(), false)),
        "SFloat" => Ok(("float".to_string(), false)),
        "SBool" => Ok(("boolean".to_string(), false)),
        "SString" => Ok(("string".to_string(), false)),
        // `STable` may carry an element type: `STable<i64>` → `table<integer>`,
        // `STable<T>` → `table<T>` (generic), `STable` (bare) → `table`.
        // The element type is the table's own, never the callback's, so the
        // `sig(...)` entry does not descend into it.
        "STable" => match option_inner(seg) {
            Some(inner) => {
                let (it, _) = saule_type(inner, tvars, None)?;
                Ok((format!("table<{it}>"), false))
            }
            None => Ok(("table".to_string(), false)),
        },
        "SFunction" => match fn_sig {
            Some(s) => {
                record_tvars(s, tvars);
                Ok((s.to_string(), false))
            }
            None => Err(syn::Error::new_spanned(
                ty,
                "`SFunction` is a bare handle — declare the signature Saule should \
                 check calls against, e.g. `#[saule_export(class = \"Util\", \
                 name = \"map\", sig(f = \"fn(T) -> T\"))]`",
            )),
        },
        // A value typed as a generic type parameter: `SElem<T>` → `T`.
        "SElem" => {
            let inner = option_inner(seg).ok_or_else(|| unsupported(ty))?;
            saule_type(inner, tvars, None)
        }
        // A dynamically-typed value crossing the ABI renders as `any`.
        "SValue" => Ok(("any".to_string(), false)),
        // The type-variable markers (`T`, `U`, `V`, `W`) render as their own
        // name and are recorded as signature type parameters.
        "T" | "U" | "V" | "W" => {
            if !tvars.iter().any(|v| v == &ident) {
                tvars.push(ident.clone());
            }
            Ok((ident, false))
        }
        "Option" => {
            let inner = option_inner(seg).ok_or_else(|| unsupported(ty))?;
            let (inner_ty, _) = saule_type(inner, tvars, fn_sig)?;
            // `?` binds to the return type inside a function type, so
            // `Option<SFunction>` has to parenthesise: `(fn() -> nil)?`,
            // not `fn() -> nil?`.
            if inner_ty.starts_with("fn(") || inner_ty.starts_with("fn<") {
                Ok((format!("({inner_ty})?"), true))
            } else {
                Ok((format!("{inner_ty}?"), true))
            }
        }
        _ => Err(unsupported(ty)),
    }
}

/// Record the generic markers (`T`/`U`/`V`/`W`) a hand-written `sig(...)`
/// string mentions, so the emitted signature carries the matching
/// `fn<T, …>` prefix even when no Rust parameter names them.
fn record_tvars(sig: &str, tvars: &mut Vec<String>) {
    for word in sig.split(|c: char| !c.is_alphanumeric() && c != '_') {
        if matches!(word, "T" | "U" | "V" | "W") && !tvars.iter().any(|v| v == word) {
            tvars.push(word.to_string());
        }
    }
}

/// Extract `T` from an `Option<T>` path segment.
fn option_inner(seg: &syn::PathSegment) -> Option<&Type> {
    let PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };
    args.args.iter().find_map(|a| match a {
        GenericArgument::Type(t) => Some(t),
        _ => None,
    })
}

/// Return `(value_type, is_result)`. For `-> ()` (or no return) returns
/// `(None, false)`; for `-> Result<T, E>` returns `(Some(T), true)`; otherwise
/// `(Some(T), false)`.
fn unwrap_return(output: &ReturnType) -> (Option<Type>, bool) {
    let ReturnType::Type(_, ty) = output else {
        return (None, false);
    };
    // Treat `-> ()` like no return.
    if let Type::Tuple(t) = &**ty
        && t.elems.is_empty()
    {
        return (None, false);
    }
    if let Type::Path(tp) = &**ty
        && let Some(seg) = tp.path.segments.last()
        && seg.ident == "Result"
        && let PathArguments::AngleBracketed(args) = &seg.arguments
        && let Some(GenericArgument::Type(t)) = args.args.iter().next()
    {
        return (Some(t.clone()), true);
    }
    (Some((**ty).clone()), false)
}

fn unsupported(ty: &Type) -> syn::Error {
    syn::Error::new_spanned(
        ty,
        "unsupported type for #[saule_export]; expected one of \
         i64, f64, bool, String, Option<_>, (), or Result<_, _>",
    )
}
