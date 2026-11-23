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
    /// Interfaces the package's classes implement (`implements(…)`) and its
    /// functions take, so one parameter can accept several classes: Shine's
    /// `Drawable` for `Graphics.draw`. They have no methods of their own.
    interfaces: Vec<(Ident, LitStr)>,
}

/// `{ Name = "doc", … }`, the body of `classes` and `interfaces`.
fn named_docs(input: ParseStream<'_>) -> syn::Result<Vec<(Ident, LitStr)>> {
    let body;
    braced!(body in input);
    let mut out = Vec::new();
    while !body.is_empty() {
        let name: Ident = body.parse()?;
        body.parse::<Token![=]>()?;
        let doc: LitStr = body.parse()?;
        out.push((name, doc));
        if body.is_empty() {
            break;
        }
        body.parse::<Token![,]>()?;
    }
    Ok(out)
}

impl Parse for PackageInput {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut name = None;
        let mut version = None;
        let mut doc = None;
        let mut classes = Vec::new();
        let mut interfaces = Vec::new();

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            match key.to_string().as_str() {
                // A braced block needs no comma after it, the way a Rust
                // item doesn't: `classes { … } interfaces { … }`.
                "classes" | "interfaces" => {
                    let entries = named_docs(input)?;
                    if key == "classes" {
                        classes.extend(entries);
                    } else {
                        interfaces.extend(entries);
                    }
                    if input.peek(Token![,]) {
                        input.parse::<Token![,]>()?;
                    }
                    continue;
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
                                     (expected `name`, `version`, `doc`, `classes` or `interfaces`)"
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
            interfaces,
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

    let interface_metas = input.interfaces.iter().map(|(iface, doc)| {
        let mut r = Record::new("interface");
        r.str("name", &iface.to_string())
            .opt_str("doc", Some(&doc.value()));
        r.emit(&format!("I_{iface}"))
    });

    quote! {
        #package_meta
        #( #class_metas )*
        #( #interface_metas )*

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
