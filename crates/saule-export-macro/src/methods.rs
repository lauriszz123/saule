//! `#[saule_methods]` — export an `impl` block of a `#[saule_class]`.
//!
//! Every `pub fn` in the block is exported, and what it becomes follows
//! from its signature, so the common case needs no annotation at all:
//!
//! | Rust                                  | Saule                    |
//! |---------------------------------------|--------------------------|
//! | `pub fn new(…) -> Self`               | `Image(…)` (constructor) |
//! | `pub fn width(&self) -> i64`          | `img.width()`            |
//! | `pub fn fill(&mut self, c: i64)`      | `img.fill(c)`            |
//! | `pub fn load(path: &str) -> Result<Self, E>` | `Image.load(path)` |
//!
//! `#[saule(...)]` on a method adjusts that: `name = "…"` renames it,
//! `constructor` makes a differently-named function the constructor,
//! `getter` / `setter` turn it into a property (`img.width`,
//! `img.width = 3`), `sig(f = "…")` types a callback, and `skip` leaves a
//! `pub fn` out. Names are converted from `snake_case` to the
//! `lowerCamelCase` Saule members use.

use proc_macro2::TokenStream;
use quote::quote;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{ImplItem, ImplItemFn, ItemImpl, Meta, MetaNameValue, ReturnType, Token, Visibility};

use crate::export::{self, Export, Receiver};
use crate::meta::{camel_case, doc_of, is_saule_ident};
use crate::types::{self, TypeCtx};
use crate::{SigOverrides, str_value};

/// What `#[saule(...)]` said about one method.
#[derive(Default)]
struct MethodAttrs {
    name: Option<String>,
    constructor: bool,
    /// `Some(None)` for a bare `getter`, `Some(Some(name))` for `getter = "…"`.
    getter: Option<Option<String>>,
    setter: Option<Option<String>>,
    skip: bool,
    sigs: SigOverrides,
    /// Whether the method carried any `#[saule]` attribute at all.
    present: bool,
}

pub(crate) fn expand(mut item: ItemImpl) -> syn::Result<TokenStream> {
    if let Some((_, path, _)) = &item.trait_ {
        return Err(syn::Error::new_spanned(
            path,
            "`#[saule_methods]` goes on an inherent `impl`, not a trait impl",
        ));
    }
    if !item.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.generics,
            "`#[saule_methods]` cannot export a generic `impl`",
        ));
    }
    let self_ty = (*item.self_ty).clone();
    let class = types::named_class(&self_ty).ok_or_else(|| {
        syn::Error::new_spanned(
            &self_ty,
            "`#[saule_methods]` goes on the `impl` of a `#[saule_class]` struct",
        )
    })?;

    let mut exports = Vec::new();
    let mut constructor_seen = None;

    for impl_item in &mut item.items {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };
        let attrs = take_attrs(method)?;
        let is_pub = matches!(method.vis, Visibility::Public(_));
        if attrs.skip {
            continue;
        }
        if !is_pub {
            if attrs.present {
                return Err(syn::Error::new_spanned(
                    &method.sig.ident,
                    "only `pub` methods are exported; make this `pub`, or remove `#[saule]`",
                ));
            }
            continue;
        }

        let (recv, inputs) = export::split_inputs(&method.sig.inputs)?;
        let fn_ident = &method.sig.ident;
        let rust_name = fn_ident.to_string();

        let receiver = decide_receiver(method, recv, &attrs, &class, &self_ty)?;
        let name = match receiver {
            Receiver::Constructor => "init".to_string(),
            Receiver::Getter => attrs
                .getter
                .clone()
                .flatten()
                .or_else(|| attrs.name.clone())
                .unwrap_or_else(|| {
                    camel_case(rust_name.strip_prefix("get_").unwrap_or(&rust_name))
                }),
            Receiver::Setter { .. } => attrs
                .setter
                .clone()
                .flatten()
                .or_else(|| attrs.name.clone())
                .unwrap_or_else(|| {
                    camel_case(rust_name.strip_prefix("set_").unwrap_or(&rust_name))
                }),
            _ => attrs.name.clone().unwrap_or_else(|| camel_case(&rust_name)),
        };
        if !is_saule_ident(&name) {
            return Err(syn::Error::new_spanned(
                fn_ident,
                format!("`{name}` is not a name Saule can call"),
            ));
        }
        if receiver == Receiver::Constructor {
            if let Some(prev) = &constructor_seen {
                return Err(syn::Error::new_spanned(
                    fn_ident,
                    format!(
                        "`{class}` already has a constructor (`{prev}`); a Saule class has one"
                    ),
                ));
            }
            constructor_seen = Some(rust_name.clone());
        }

        exports.push(export::expand(Export {
            class: class.clone(),
            name,
            receiver,
            doc: doc_of(&method.attrs),
            sig_overrides: &attrs.sigs,
            callee: quote! { <#self_ty>::#fn_ident },
            self_ty: Some(&self_ty),
            inputs,
            output: &method.sig.output,
            span: fn_ident.span(),
        })?);
    }

    Ok(quote! {
        #item
        #( #exports )*
    })
}

/// Work out how a method is reached from Saule.
fn decide_receiver(
    method: &ImplItemFn,
    recv: Option<&syn::Receiver>,
    attrs: &MethodAttrs,
    class: &str,
    self_ty: &syn::Type,
) -> syn::Result<Receiver> {
    let span = method.sig.ident.span();
    let recv_kind = match recv {
        None => None,
        Some(r) if r.reference.is_none() => {
            return Err(syn::Error::new(
                r.span(),
                "take `&self` or `&mut self`: a Saule object is shared, so a method \
                 cannot take it by value",
            ));
        }
        Some(r) if r.colon_token.is_some() => {
            return Err(syn::Error::new(
                r.span(),
                "an exported method takes `&self` or `&mut self`, not a typed `self`",
            ));
        }
        Some(r) => Some(r.mutability.is_some()),
    };

    if attrs.getter.is_some() {
        return match recv_kind {
            Some(_) => Ok(Receiver::Getter),
            None => Err(syn::Error::new(
                span,
                "a getter reads a property, so it takes `&self`",
            )),
        };
    }
    if attrs.setter.is_some() {
        return match recv_kind {
            Some(mutable) => Ok(Receiver::Setter { mutable }),
            None => Err(syn::Error::new(
                span,
                "a setter writes a property, so it takes `&mut self`",
            )),
        };
    }
    if attrs.constructor {
        return match recv_kind {
            None => Ok(Receiver::Constructor),
            Some(_) => Err(syn::Error::new(
                span,
                "a constructor builds a new object, so it takes no `self`",
            )),
        };
    }
    match recv_kind {
        Some(mutable) => Ok(Receiver::Instance { mutable }),
        // `new` returning the class is the constructor by convention, the
        // same convention Rust itself follows.
        None if method.sig.ident == "new" && returns_self(&method.sig.output, class, self_ty) => {
            Ok(Receiver::Constructor)
        }
        None => Ok(Receiver::Static),
    }
}

/// Whether a function returns the class: `Self`, `Image`, or a `Result` of
/// either.
fn returns_self(output: &ReturnType, class: &str, self_ty: &syn::Type) -> bool {
    let (Some(ty), _) = export::unwrap_return(output) else {
        return false;
    };
    let ctx = TypeCtx::new(Some(self_ty));
    types::resolve_self(&ty, &ctx)
        .ok()
        .and_then(|t| types::named_class(&t))
        .is_some_and(|n| n == class)
}

/// Read and remove every `#[saule(...)]` attribute on `method`. Removing is
/// required: `saule` is not an attribute the compiler knows, so leaving it
/// on the emitted `impl` would be an error.
fn take_attrs(method: &mut ImplItemFn) -> syn::Result<MethodAttrs> {
    let mut out = MethodAttrs::default();
    let mut kept = Vec::with_capacity(method.attrs.len());
    for attr in std::mem::take(&mut method.attrs) {
        if !attr.path().is_ident("saule") {
            kept.push(attr);
            continue;
        }
        out.present = true;
        let list = attr.meta.require_list()?;
        let metas = Punctuated::<Meta, Token![,]>::parse_terminated.parse2(list.tokens.clone())?;
        for meta in metas {
            let key = meta
                .path()
                .get_ident()
                .map(ToString::to_string)
                .unwrap_or_default();
            match (key.as_str(), &meta) {
                ("skip", Meta::Path(_)) => out.skip = true,
                ("constructor", Meta::Path(_)) => out.constructor = true,
                ("getter", Meta::Path(_)) => out.getter = Some(None),
                ("setter", Meta::Path(_)) => out.setter = Some(None),
                ("getter", Meta::NameValue(nv)) => out.getter = Some(Some(str_value(&nv.value)?)),
                ("setter", Meta::NameValue(nv)) => out.setter = Some(Some(str_value(&nv.value)?)),
                ("name", Meta::NameValue(nv)) => out.name = Some(str_value(&nv.value)?),
                ("sig", Meta::List(list)) => {
                    let entries = list.parse_args_with(
                        Punctuated::<MetaNameValue, Token![,]>::parse_terminated,
                    )?;
                    for nv in entries {
                        let slot = nv
                            .path
                            .get_ident()
                            .map(ToString::to_string)
                            .unwrap_or_default();
                        out.sigs.push((slot, str_value(&nv.value)?));
                    }
                }
                _ => {
                    return Err(syn::Error::new_spanned(
                        &meta,
                        "unknown `#[saule(...)]` option (expected `name = \"…\"`, \
                         `constructor`, `getter`, `setter`, `sig(…)` or `skip`)",
                    ));
                }
            }
        }
    }
    method.attrs = kept;
    let roles = [out.constructor, out.getter.is_some(), out.setter.is_some()];
    if roles.iter().filter(|r| **r).count() > 1 {
        return Err(syn::Error::new_spanned(
            &method.sig.ident,
            "a method can be only one of `constructor`, `getter` and `setter`",
        ));
    }
    Ok(out)
}
