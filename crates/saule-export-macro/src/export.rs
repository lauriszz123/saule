//! One exported member — a free function, a static or instance method, a
//! constructor, a property accessor — turned into its `extern "C"` shim and
//! its metadata record.
//!
//! `#[saule_export]` and `#[saule_methods]` both describe what they found as
//! an [`Export`] and hand it here, so there is exactly one place that knows
//! how arguments are decoded, how a receiver is borrowed, and how a result
//! is encoded.

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::{FnArg, Pat, PatType, ReturnType, Type};

use crate::meta::Record;
use crate::types::{self, Decode, TypeCtx};

/// How a member is reached from Saule.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Receiver {
    /// `Class.member(args)` — a free function, or an associated function
    /// without `self`.
    Static,
    /// `object.member(args)` — `&self` or `&mut self`.
    Instance { mutable: bool },
    /// `Class(args)` — builds a new object.
    Constructor,
    /// `object.member` — read like a field.
    Getter,
    /// `object.member = value` — written like a field.
    Setter { mutable: bool },
}

impl Receiver {
    /// The spelling in the metadata record.
    fn as_str(self) -> &'static str {
        match self {
            Receiver::Static => "static",
            Receiver::Instance { .. } => "instance",
            Receiver::Constructor => "constructor",
            Receiver::Getter => "getter",
            Receiver::Setter { .. } => "setter",
        }
    }

    /// Whether argument 0 is the object the member is called on.
    fn has_self(self) -> bool {
        matches!(
            self,
            Receiver::Instance { .. } | Receiver::Getter | Receiver::Setter { .. }
        )
    }

    fn self_is_mut(self) -> bool {
        matches!(
            self,
            Receiver::Instance { mutable: true } | Receiver::Setter { mutable: true }
        )
    }
}

pub(crate) struct Export<'a> {
    /// The Saule class the member belongs to.
    pub(crate) class: String,
    /// The Saule-visible member name. For a constructor, `init` — the name a
    /// Saule class's constructor has, so the checker treats both alike.
    pub(crate) name: String,
    pub(crate) receiver: Receiver,
    pub(crate) doc: Option<String>,
    /// `sig(param = "…")` overrides, keyed by parameter name (and `return`).
    pub(crate) sig_overrides: &'a [(String, String)],
    /// The Rust function to call, e.g. `window_create` or `<Image>::width`.
    pub(crate) callee: TokenStream,
    /// The `impl`'s self type, for members declared in `#[saule_methods]`.
    pub(crate) self_ty: Option<&'a Type>,
    /// Every parameter *except* the receiver.
    pub(crate) inputs: Vec<&'a PatType>,
    pub(crate) output: &'a ReturnType,
    /// Where to point errors about the member as a whole.
    pub(crate) span: Span,
}

/// Parse a fn's inputs into the receiver (if any) and the other parameters.
pub(crate) fn split_inputs<'f>(
    inputs: impl IntoIterator<Item = &'f FnArg>,
) -> syn::Result<(Option<&'f syn::Receiver>, Vec<&'f PatType>)> {
    let mut recv = None;
    let mut rest = Vec::new();
    for input in inputs {
        match input {
            FnArg::Receiver(r) => recv = Some(r),
            FnArg::Typed(pt) => rest.push(pt),
        }
    }
    Ok((recv, rest))
}

pub(crate) fn expand(e: Export<'_>) -> syn::Result<TokenStream> {
    let sig_for = |slot: &str| -> Option<&str> {
        e.sig_overrides
            .iter()
            .find(|(k, _)| k == slot)
            .map(|(_, v)| v.as_str())
    };
    // What a Saule error message calls this member.
    let qualified = match e.receiver {
        Receiver::Constructor => e.class.clone(),
        _ => format!("{}.{}", e.class, e.name),
    };
    let mut ctx = TypeCtx::new(e.self_ty);

    // ── Parameters ─────────────────────────────────────────────────────────
    let offset = usize::from(e.receiver.has_self());
    let mut sig_params = Vec::new();
    let mut decodes = Vec::new();
    let mut call_args = Vec::new();
    let mut required = 0usize;
    let mut seen_optional = false;

    for (i, pt) in e.inputs.iter().enumerate() {
        let Pat::Ident(pat_ident) = &*pt.pat else {
            return Err(syn::Error::new_spanned(
                &pt.pat,
                "exported parameters must be simple identifiers",
            ));
        };
        let pname = pat_ident.ident.to_string();
        let (saule_ty, optional, decode) = types::param(&pt.ty, &mut ctx, sig_for(&pname))?;
        if optional {
            seen_optional = true;
        } else {
            if seen_optional {
                return Err(syn::Error::new_spanned(
                    &pt.ty,
                    "required parameters cannot follow an optional (`Option<_>`) parameter",
                ));
            }
            required += 1;
        }
        sig_params.push(format!("{pname}: {saule_ty}"));

        let idx = i + offset;
        let local = format_ident!("__a_{}", pat_ident.ident);
        let guard = format_ident!("__g_{}", pat_ident.ident);
        let (decode_ts, arg_ts) = decode_param(&decode, &local, &guard, idx, &qualified, &pname);
        decodes.push(decode_ts);
        call_args.push(arg_ts);
    }
    let total = e.inputs.len();

    // ── Receiver ───────────────────────────────────────────────────────────
    let receiver_decode = if e.receiver.has_self() {
        let self_ty = e.self_ty.ok_or_else(|| {
            syn::Error::new(e.span, "a method with `self` must be in `#[saule_methods]`")
        })?;
        let borrow = if e.receiver.self_is_mut() {
            quote! { let mut __self_ref = __self.borrow_mut()?; }
        } else {
            quote! { let __self_ref = __self.borrow()?; }
        };
        let pass = if e.receiver.self_is_mut() {
            quote! { &mut *__self_ref }
        } else {
            quote! { &*__self_ref }
        };
        call_args.insert(0, pass);
        quote! {
            let __self = <::saule_sdk::SObject<#self_ty> as ::saule_sdk::__private::FromSaule>
                ::from_saule(__args, 0, #qualified, "self")?;
            #borrow
        }
    } else {
        quote! {}
    };

    // ── Return type ────────────────────────────────────────────────────────
    let (ret_value_ty, is_result) = unwrap_return(e.output);
    let ret_saule = match &ret_value_ty {
        Some(ty) => types::return_type(ty, &mut ctx, sig_for("return"))?,
        None => "nil".to_string(),
    };
    check_shape(&e, &ret_saule, total)?;

    let generics = if ctx.tvars.is_empty() {
        String::new()
    } else {
        format!("<{}>", ctx.tvars.join(", "))
    };
    let sig = format!("fn{generics}({}) -> {ret_saule}", sig_params.join(", "));

    // ── Names ──────────────────────────────────────────────────────────────
    let class = &e.class;
    let name = &e.name;
    let (shim_name, meta_suffix) = match e.receiver {
        Receiver::Static | Receiver::Instance { .. } => (
            format!("saule_export_{class}_{name}"),
            format!("M_{class}_{name}"),
        ),
        Receiver::Constructor => (format!("saule_export_{class}__new"), format!("N_{class}")),
        Receiver::Getter => (
            format!("saule_export_{class}__get_{name}"),
            format!("G_{class}_{name}"),
        ),
        Receiver::Setter { .. } => (
            format!("saule_export_{class}__set_{name}"),
            format!("S_{class}_{name}"),
        ),
    };
    let shim_ident = format_ident!("{}", shim_name);

    // ── Metadata ───────────────────────────────────────────────────────────
    let mut record = Record::new("method");
    record
        .str("class", class)
        .str("name", name)
        .str("receiver", e.receiver.as_str())
        .str("sig", &sig)
        .str("symbol", &shim_name)
        .opt_str("doc", e.doc.as_deref());
    let meta = record.emit(&meta_suffix);

    // ── The shim ───────────────────────────────────────────────────────────
    let arity_desc = if required == total {
        format!("{total}")
    } else {
        format!("{required} to {total}")
    };
    let callee = &e.callee;
    let call = quote! { #callee( #( #call_args ),* ) };
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
        #[doc(hidden)]
        #[allow(non_snake_case)]
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

            ::saule_sdk::__private::run_export(__out, #qualified, || {
                let __given = __args.len().saturating_sub(#offset);
                // Spelled as a range so the generated code is lint-clean in
                // the author's crate, whatever their clippy settings.
                if __args.len() < #offset || !(#required..=#total).contains(&__given) {
                    return ::core::result::Result::Err(::std::format!(
                        "{} expects {} argument(s), got {}",
                        #qualified, #arity_desc, __given,
                    ));
                }
                #receiver_decode
                #( #decodes )*
                #encode
            })
        }

        #meta
    })
}

/// The decode statement for one parameter, and the expression passed to the
/// Rust function in its place.
fn decode_param(
    decode: &Decode,
    local: &syn::Ident,
    guard: &syn::Ident,
    idx: usize,
    qualified: &str,
    pname: &str,
) -> (TokenStream, TokenStream) {
    let from = |ty: TokenStream| {
        quote! {
            let #local = <#ty as ::saule_sdk::__private::FromSaule>::from_saule(
                __args, #idx, #qualified, #pname,
            )?;
        }
    };
    match decode {
        Decode::Owned(ty) => (from(quote! { #ty }), quote! { #local }),
        Decode::StrRef => (
            from(quote! { ::std::string::String }),
            quote! { &#local },
        ),
        Decode::OptStrRef => (
            from(quote! { ::core::option::Option<::std::string::String> }),
            quote! { #local.as_deref() },
        ),
        Decode::ObjRef { inner, mutable } => {
            let d = from(quote! { ::saule_sdk::SObject<#inner> });
            if *mutable {
                (
                    quote! { #d let mut #guard = #local.borrow_mut()?; },
                    quote! { &mut *#guard },
                )
            } else {
                (
                    quote! { #d let #guard = #local.borrow()?; },
                    quote! { &*#guard },
                )
            }
        }
        Decode::OptObjRef { inner, mutable } => {
            let d = from(quote! { ::core::option::Option<::saule_sdk::SObject<#inner>> });
            if *mutable {
                (
                    quote! {
                        #d
                        let mut #guard = match &#local {
                            ::core::option::Option::Some(__o) =>
                                ::core::option::Option::Some(__o.borrow_mut()?),
                            ::core::option::Option::None => ::core::option::Option::None,
                        };
                    },
                    quote! { #guard.as_deref_mut() },
                )
            } else {
                (
                    quote! {
                        #d
                        let #guard = match &#local {
                            ::core::option::Option::Some(__o) =>
                                ::core::option::Option::Some(__o.borrow()?),
                            ::core::option::Option::None => ::core::option::Option::None,
                        };
                    },
                    quote! { #guard.as_deref() },
                )
            }
        }
    }
}

/// Reject a member whose shape does not fit its role, with a message that
/// says what the role needs.
fn check_shape(e: &Export<'_>, ret_saule: &str, n_params: usize) -> syn::Result<()> {
    let err = |msg: &str| Err(syn::Error::new(e.span, msg));
    match e.receiver {
        Receiver::Getter if n_params != 0 => {
            err("a getter takes only `&self`: a property read has no arguments")
        }
        Receiver::Getter if ret_saule == "nil" => err("a getter must return the property's value"),
        Receiver::Setter { .. } if n_params != 1 => {
            err("a setter takes `&mut self` and exactly one value")
        }
        Receiver::Setter { .. } if ret_saule != "nil" => {
            err("a setter returns nothing (or `Result<(), E>`): assignment has no value")
        }
        Receiver::Constructor => {
            let class = &e.class;
            if ret_saule == *class {
                Ok(())
            } else {
                err(&format!(
                    "a constructor must return `Self` (or `Result<Self, E>`), \
                     so that `{class}(…)` is a `{class}`"
                ))
            }
        }
        _ => Ok(()),
    }
}

/// Return `(value_type, is_result)`. For `-> ()` (or no return) returns
/// `(None, false)`; for `-> Result<T, E>` returns `(Some(T), true)`; otherwise
/// `(Some(T), false)`.
pub(crate) fn unwrap_return(output: &ReturnType) -> (Option<Type>, bool) {
    let ReturnType::Type(_, ty) = output else {
        return (None, false);
    };
    if let Type::Tuple(t) = &**ty
        && t.elems.is_empty()
    {
        return (None, false);
    }
    if let Some(ok) = types::generic_arg(ty, "Result") {
        if let Type::Tuple(t) = ok
            && t.elems.is_empty()
        {
            return (None, true);
        }
        return (Some(ok.clone()), true);
    }
    (Some((**ty).clone()), false)
}
