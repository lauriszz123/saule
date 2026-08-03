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
//! ## Generics
//! The marker types `T`/`U`/`V`/`W` from `saule_sdk` are type variables:
//! `STable<T> → table<T>`, `SElem<T> → T`. A signature that mentions them
//! emits a `fn<T, …>(…)` prefix, so e.g.
//! `fn(t: STable<T>, f: SFunction) -> Option<SElem<T>>` checks as
//! `fn<T>(t: table<T>, f: function) -> T?`.
//!
//! Generated code references `::saule_sdk::__private::*`, so the annotated
//! crate must depend on `saule-sdk`.

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::{format_ident, quote};
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{
    Expr, ExprLit, FnArg, GenericArgument, ItemFn, Lit, MetaNameValue, Pat, PathArguments,
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
    let (class, name) = parse_attr(attr)?;

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
        let (saule_ty, is_optional) = saule_type(&ty, &mut tvars)?;

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
        Some(ty) => saule_return_type(ty, &mut tvars)?,
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

/// Parse the `class = "..."`, `name = "..."` attribute arguments.
fn parse_attr(attr: TokenStream) -> Result<(String, String), syn::Error> {
    let parser = Punctuated::<MetaNameValue, Token![,]>::parse_terminated;
    let args = parser.parse(attr)?;

    let mut class = None;
    let mut name = None;
    for nv in args {
        let key = nv
            .path
            .get_ident()
            .map(|i| i.to_string())
            .unwrap_or_default();
        let value = match &nv.value {
            Expr::Lit(ExprLit {
                lit: Lit::Str(s), ..
            }) => s.value(),
            other => {
                return Err(syn::Error::new_spanned(other, "expected a string literal"));
            }
        };
        match key.as_str() {
            "class" => class = Some(value),
            "name" => name = Some(value),
            other => {
                return Err(syn::Error::new_spanned(
                    &nv.path,
                    format!("unknown `saule_export` key `{other}` (expected `class` or `name`)"),
                ));
            }
        }
    }

    match (class, name) {
        (Some(c), Some(n)) => Ok((c, n)),
        _ => Err(syn::Error::new(
            Span::call_site(),
            "`saule_export` requires `class = \"...\"` and `name = \"...\"`",
        )),
    }
}

/// Map a return type to its Saule signature string. Unlike [`saule_type`],
/// this accepts a non-unit tuple `(A, B, …)` and renders it as a Saule
/// multi-return type `(a, b, …)` — the manifest parser turns that into a
/// `Type::Tuple` and the interpreter spreads the value across several
/// bindings. Element types still flow through [`saule_type`], so generic
/// markers and `Option<_>` work inside a tuple return.
fn saule_return_type(ty: &Type, tvars: &mut Vec<String>) -> Result<String, syn::Error> {
    if let Type::Tuple(t) = ty {
        if t.elems.is_empty() {
            return Ok("nil".to_string());
        }
        let mut parts = Vec::with_capacity(t.elems.len());
        for elem in &t.elems {
            parts.push(saule_type(elem, tvars)?.0);
        }
        return Ok(format!("({})", parts.join(", ")));
    }
    Ok(saule_type(ty, tvars)?.0)
}

/// Map a Rust type to its Saule type string. Returns `(saule_type, is_optional)`
/// where `is_optional` is true for `Option<T>`.
fn saule_type(ty: &Type, tvars: &mut Vec<String>) -> Result<(String, bool), syn::Error> {
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
        return saule_type(&r.elem, tvars);
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
        "STable" => match option_inner(seg) {
            Some(inner) => {
                let (it, _) = saule_type(inner, tvars)?;
                Ok((format!("table<{it}>"), false))
            }
            None => Ok(("table".to_string(), false)),
        },
        "SFunction" => Ok(("function".to_string(), false)),
        // A value typed as a generic type parameter: `SElem<T>` → `T`.
        "SElem" => {
            let inner = option_inner(seg).ok_or_else(|| unsupported(ty))?;
            saule_type(inner, tvars)
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
            let (inner_ty, _) = saule_type(inner, tvars)?;
            Ok((format!("{inner_ty}?"), true))
        }
        _ => Err(unsupported(ty)),
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
