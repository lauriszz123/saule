//! `saule_package!` — the package's own record and the handful of symbols
//! every package exports whatever it declares.

use proc_macro2::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{Ident, LitStr, Token, braced};

use crate::meta::{Record, is_saule_ident};

pub(crate) struct PackageInput {
    name: LitStr,
    version: LitStr,
    doc: Option<LitStr>,
    /// Static-only classes (namespaces like `Graphics`) and their docs. A
    /// class that only ever appears in `#[saule_export(class = …)]` needs no
    /// entry here; listing it is how it gets a doc comment.
    classes: Vec<(Ident, LitStr)>,
}

impl Parse for PackageInput {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut name = None;
        let mut version = None;
        let mut doc = None;
        let mut classes = Vec::new();

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            match key.to_string().as_str() {
                "classes" => {
                    let body;
                    braced!(body in input);
                    while !body.is_empty() {
                        let class: Ident = body.parse()?;
                        body.parse::<Token![=]>()?;
                        let doc: LitStr = body.parse()?;
                        classes.push((class, doc));
                        if body.is_empty() {
                            break;
                        }
                        body.parse::<Token![,]>()?;
                    }
                }
                "binary" => {
                    return Err(syn::Error::new_spanned(
                        &key,
                        "`binary` is no longer needed: the package's metadata is compiled \
                         into the library, so the file you install *is* the package. \
                         Remove this line.",
                    ));
                }
                other => {
                    input.parse::<Token![=]>()?;
                    let value: LitStr = input.parse()?;
                    match other {
                        "name" => name = Some(value),
                        "version" => version = Some(value),
                        "doc" => doc = Some(value),
                        _ => {
                            return Err(syn::Error::new_spanned(
                                &key,
                                format!(
                                    "unknown `saule_package!` key `{other}` \
                                     (expected `name`, `version`, `doc` or `classes`)"
                                ),
                            ));
                        }
                    }
                }
            }
            if input.is_empty() {
                break;
            }
            input.parse::<Token![,]>()?;
        }

        let name = name.ok_or_else(|| input.error("`saule_package!` requires `name = \"…\"`"))?;
        if !is_saule_ident(&name.value()) {
            return Err(syn::Error::new_spanned(
                &name,
                "the package name is what programs import, so it must be an identifier",
            ));
        }
        let version =
            version.ok_or_else(|| input.error("`saule_package!` requires `version = \"…\"`"))?;
        Ok(PackageInput {
            name,
            version,
            doc,
            classes,
        })
    }
}

pub(crate) fn expand(input: PackageInput) -> TokenStream {
    let mut package = Record::new("package");
    package
        .str("name", &input.name.value())
        .str("version", &input.version.value())
        // The ABI this package's shims are compiled against. The authoritative
        // check is the `saule_abi_version` symbol below; this copy is what lets
        // the interpreter refuse a stale package *before* loading it.
        .int("abi_version", i64::from(saule_native_abi::ABI_VERSION))
        .opt_str("doc", input.doc.as_ref().map(LitStr::value).as_deref());
    let package_meta = package.emit("PACKAGE");

    let class_metas = input.classes.iter().map(|(class, doc)| {
        let mut r = Record::new("class");
        r.str("name", &class.to_string())
            .opt_str("doc", Some(&doc.value()))
            .bool("instantiable", false);
        r.emit(&format!("C_{class}"))
    });

    quote! {
        #package_meta
        #( #class_metas )*

        // Report the ABI this package was compiled against. The interpreter
        // calls this before it looks up any other symbol and refuses to load
        // on a mismatch, so a package built against another SDK says so
        // instead of corrupting memory at its first call.
        #[doc(hidden)]
        #[unsafe(no_mangle)]
        pub extern "C" fn saule_abi_version() -> u32 {
            ::saule_sdk::__private::ABI_VERSION
        }

        // Receive the host callback table so `STable` / `SFunction` can
        // operate on host-owned reference values.
        #[doc(hidden)]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn saule_set_host(api: *const ::saule_sdk::__private::HostApi) {
            unsafe { ::saule_sdk::__private::__set_host(api) };
        }

        // Give back the host's reference to one of this package's objects.
        #[doc(hidden)]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn saule_object_release(ptr: ::saule_sdk::__private::ObjectPtr) {
            unsafe { ::saule_sdk::__private::release_object(ptr) };
        }
    }
}
