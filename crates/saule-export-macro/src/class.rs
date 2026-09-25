//! `#[saule_class]` and `#[saule_enum]` — Rust types that become Saule
//! types.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Fields, ItemEnum, ItemStruct};

use crate::meta::{Record, doc_of};

/// A struct whose values Saule programs hold as objects.
///
/// The struct itself is untouched. Alongside it this emits the
/// `NativeClass` impl the SDK's `SObject<T>` needs, an `IntoSaule` impl so a
/// function can simply return a `T` (it is moved into a fresh object), and
/// the class's metadata record.
pub(crate) fn expand_class(item: ItemStruct) -> syn::Result<TokenStream> {
    if !item.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.generics,
            "a `#[saule_class]` cannot be generic: Saule sees one class per Rust type, \
             so write a concrete struct for each class you want to expose",
        ));
    }
    let ident = &item.ident;
    let name = ident.to_string();

    let mut record = Record::new("class");
    record
        .str("name", &name)
        .opt_str("doc", doc_of(&item.attrs).as_deref())
        .bool("instantiable", true);
    let meta = record.emit(&format!("C_{name}"));

    Ok(quote! {
        #item

        impl ::saule_sdk::NativeClass for #ident {
            const NAME: &'static str = #name;
            fn __descriptor() -> &'static ::saule_sdk::__private::ClassDescriptor {
                static DESCRIPTOR: ::saule_sdk::__private::ClassDescriptor =
                    ::saule_sdk::__private::ClassDescriptor::of::<#ident>(#name);
                &DESCRIPTOR
            }
        }

        /// Returning the value moves it into a new Saule object.
        impl ::saule_sdk::__private::IntoSaule for #ident {
            fn into_saule(self) -> ::saule_sdk::__private::CValue {
                ::saule_sdk::__private::IntoSaule::into_saule(::saule_sdk::SObject::new(self))
            }
        }

        #meta
    })
}

/// A fieldless enum that becomes a Saule enum of the same name, with one
/// variant per Rust variant, spelled the same.
///
/// A variant crosses the boundary as its name. The interpreter turns a
/// returned name back into the enum value using this record, and hands the
/// package a variant's name when one is passed in — so the package side is
/// a plain `match` on a string, generated here.
pub(crate) fn expand_enum(item: ItemEnum) -> syn::Result<TokenStream> {
    if !item.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.generics,
            "a `#[saule_enum]` cannot be generic",
        ));
    }
    let ident = &item.ident;
    let name = ident.to_string();

    let mut variants = Vec::new();
    let mut variant_docs = Vec::new();
    for v in &item.variants {
        if !matches!(v.fields, Fields::Unit) {
            return Err(syn::Error::new_spanned(
                v,
                "a `#[saule_enum]` variant cannot carry data; model the data as a \
                 `#[saule_class]` and pass that instead",
            ));
        }
        variants.push(v.ident.clone());
        variant_docs.push(doc_of(&v.attrs).unwrap_or_default());
    }
    if variants.is_empty() {
        return Err(syn::Error::new_spanned(
            ident,
            "a `#[saule_enum]` needs at least one variant",
        ));
    }
    let names: Vec<String> = variants.iter().map(ToString::to_string).collect();
    let expected = names.join(", ");

    let mut record = Record::new("enum");
    record
        .str("name", &name)
        .opt_str("doc", doc_of(&item.attrs).as_deref())
        .str_list("variants", &names)
        .str_list("variant_docs", &variant_docs);
    let meta = record.emit(&format!("E_{name}"));

    Ok(quote! {
        #item

        impl ::saule_sdk::__private::FromSaule for #ident {
            fn from_saule(
                args: &[::saule_sdk::__private::CValue],
                idx: usize,
                func: &str,
                param: &str,
            ) -> ::core::result::Result<Self, ::std::string::String> {
                let s = <::std::string::String as ::saule_sdk::__private::FromSaule>
                    ::from_saule(args, idx, func, param)
                    .map_err(|_| ::std::format!(
                        "{func}: argument `{param}` must be a {}", #name,
                    ))?;
                match s.as_str() {
                    #( #names => ::core::result::Result::Ok(Self::#variants), )*
                    other => ::core::result::Result::Err(::std::format!(
                        "{func}: `{other}` is not a {} (expected one of {})",
                        #name, #expected,
                    )),
                }
            }
        }

        impl ::saule_sdk::__private::IntoSaule for #ident {
            fn into_saule(self) -> ::saule_sdk::__private::CValue {
                // The names are `'static`, so they are passed without the
                // return buffer — which also keeps a tuple of enums from
                // overwriting one variant's name with the next.
                ::saule_sdk::__private::CValue::string_borrowed(match self {
                    #( Self::#variants => #names.as_bytes(), )*
                })
            }
        }

        #meta
    })
}
