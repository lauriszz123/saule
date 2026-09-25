//! The macros behind `saule-sdk`: they turn ordinary Rust into a Saule
//! native package, and compile the package's description into the library.
//!
//! ```ignore
//! use saule_sdk::prelude::*;
//!
//! saule_package! { name = "gfx", version = "0.1.0" }
//!
//! /// A decoded image.
//! #[saule_class]
//! pub struct Image { w: i64, h: i64, pixels: Vec<u32> }
//!
//! #[saule_methods]
//! impl Image {
//!     /// A blank image. Saule calls this as `Image(w, h)`.
//!     pub fn new(w: i64, h: i64) -> Self { /* … */ }
//!     /// Width in pixels. `#[saule(getter)]` makes it `img.width`.
//!     #[saule(getter)]
//!     pub fn width(&self) -> i64 { self.w }
//!     /// Paint every pixel.
//!     pub fn fill(&mut self, color: i64) { /* … */ }
//! }
//!
//! #[saule_export(class = "Gfx")]
//! fn load_image(path: &str) -> Result<Image, String> { /* … */ }
//! ```
//!
//! From that, the program gets `Image(…)`, `img.width`, `img.fill(…)` and
//! `Gfx.loadImage(…) -> Image`, fully typed: the checker rejects
//! `img.fill("red")`, and the editor completes `img.` and shows each doc
//! comment on hover. Nothing else is written by hand — no manifest, no
//! signature strings, no symbol names.
//!
//! ## Type mapping
//! `i8`…`i64`/`u8`…`u64`/`isize`/`usize` → `integer`, `f32`/`f64` → `float`,
//! `bool` → `boolean`, `String`/`&str` → `string`, `Option<T>` → `T?`,
//! `Vec<T>` → `table<T>`, `HashMap<K, V>`/`BTreeMap<K, V>` → `table<K, V>`,
//! `SValue` → `any`, `()` → `nil`, a `#[saule_class]` or `#[saule_enum]`
//! type → itself. A class is taken as `&T` / `&mut T` (borrowed for the
//! call) or `SObject<T>` (a shared handle you may keep), and returned as `T`
//! (moved into a new object) or `SObject<T>` (an existing one). A return may
//! be `Result<T, E>` (an `Err` becomes a Saule runtime error) or a tuple
//! `(A, B, …)` (a multi-value return).
//!
//! ## Callbacks
//! `SFunction` carries no signature of its own, so a callback parameter
//! declares the one Saule checks calls against: `sig(f = "fn(T) -> T")`,
//! keyed by the Rust parameter's name (`sig(return = "…")` for the return
//! slot). Leaving it off is a compile error.
//!
//! ## Generics
//! The markers `T`/`U`/`V`/`W` from `saule_sdk` are type variables:
//! `STable<T> → table<T>`, `SElem<T> → T`, and a signature that mentions
//! them gets a `fn<T, …>(…)` prefix.
//!
//! ## What gets emitted
//! Each declaration becomes an `extern "C"` shim (arity checks, argument
//! decoding, borrow checks, error and panic marshalling) plus a metadata
//! record, an exported static the interpreter reads out of the library file
//! without loading it — see `saule_native_abi`'s "Package metadata".
//! Generated code refers to `::saule_sdk`, so the crate must depend on it.

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{Expr, ExprLit, ItemEnum, ItemFn, ItemImpl, ItemStruct, Lit, Meta, MetaNameValue, Token};

mod class;
mod export;
mod meta;
mod methods;
mod package;
mod types;

use export::{Export, Receiver};
use meta::{camel_case, doc_of, is_saule_ident};

/// Export a free function as a static member of a Saule class:
/// `#[saule_export(class = "Window", name = "create")]`. `name` defaults to
/// the function's name in `lowerCamelCase`. See the crate docs.
#[proc_macro_attribute]
pub fn saule_export(attr: TokenStream, item: TokenStream) -> TokenStream {
    match expand_export(attr, item) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// Declare the package: `saule_package! { name = "engine", version = "0.1.0" }`,
/// optionally with `doc = "…"` and `classes { Graphics = "doc", … }` to
/// document static-only classes. Exactly one per package.
#[proc_macro]
pub fn saule_package(input: TokenStream) -> TokenStream {
    match syn::parse::<package::PackageInput>(input) {
        Ok(p) => package::expand(p).into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// Make a struct a Saule class whose objects the program can hold. Pair it
/// with `#[saule_methods]` on its `impl`.
#[proc_macro_attribute]
pub fn saule_class(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return syn::Error::new(
            Span::call_site(),
            "`#[saule_class]` takes no arguments: the Saule class has the struct's name",
        )
        .to_compile_error()
        .into();
    }
    match syn::parse::<ItemStruct>(item).and_then(class::expand_class) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// Export the `pub` methods of a `#[saule_class]`'s `impl`. See the
/// `methods` module docs for how each method's role is decided.
#[proc_macro_attribute]
pub fn saule_methods(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return syn::Error::new(Span::call_site(), "`#[saule_methods]` takes no arguments")
            .to_compile_error()
            .into();
    }
    match syn::parse::<ItemImpl>(item).and_then(methods::expand) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// Make a fieldless enum a Saule enum of the same name and variants.
#[proc_macro_attribute]
pub fn saule_enum(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return syn::Error::new(Span::call_site(), "`#[saule_enum]` takes no arguments")
            .to_compile_error()
            .into();
    }
    match syn::parse::<ItemEnum>(item).and_then(class::expand_enum) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn expand_export(attr: TokenStream, item: TokenStream) -> syn::Result<proc_macro2::TokenStream> {
    let func: ItemFn = syn::parse(item)?;
    let (class, name, sigs) = parse_export_attr(attr)?;
    let name = name.unwrap_or_else(|| camel_case(&func.sig.ident.to_string()));
    if !is_saule_ident(&class) {
        return Err(syn::Error::new(
            Span::call_site(),
            format!("`{class}` is not a name Saule can use for a class"),
        ));
    }
    if !is_saule_ident(&name) {
        return Err(syn::Error::new_spanned(
            &func.sig.ident,
            format!("`{name}` is not a name Saule can call"),
        ));
    }

    let (recv, inputs) = export::split_inputs(&func.sig.inputs)?;
    if let Some(r) = recv {
        return Err(syn::Error::new_spanned(
            r,
            "`#[saule_export]` is for free functions; export methods with \
             `#[saule_methods]` on the `impl`",
        ));
    }
    let fn_ident = &func.sig.ident;
    let exported = export::expand(Export {
        class,
        name,
        receiver: Receiver::Static,
        doc: doc_of(&func.attrs),
        sig_overrides: &sigs,
        callee: quote! { #fn_ident },
        self_ty: None,
        inputs,
        output: &func.sig.output,
        span: fn_ident.span(),
    })?;

    Ok(quote! {
        #func
        #exported
    })
}

/// The `sig(...)` overrides, keyed by parameter name (plus the reserved key
/// `return` for the return slot).
pub(crate) type SigOverrides = Vec<(String, String)>;

/// Parse `class = "...", name = "..."?, sig(param = "...")`.
fn parse_export_attr(attr: TokenStream) -> syn::Result<(String, Option<String>, SigOverrides)> {
    let args = Punctuated::<Meta, Token![,]>::parse_terminated.parse(attr)?;

    let mut class = None;
    let mut name = None;
    let mut sigs: SigOverrides = Vec::new();
    for meta in args {
        let key = meta
            .path()
            .get_ident()
            .map(ToString::to_string)
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
                    .map(ToString::to_string)
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

    match class {
        Some(c) => Ok((c, name, sigs)),
        None => Err(syn::Error::new(
            Span::call_site(),
            "`saule_export` requires `class = \"...\"`: the Saule class the function is a \
             static member of",
        )),
    }
}

pub(crate) fn str_value(expr: &Expr) -> syn::Result<String> {
    match expr {
        Expr::Lit(ExprLit {
            lit: Lit::Str(s), ..
        }) => Ok(s.value()),
        other => Err(syn::Error::new_spanned(other, "expected a string literal")),
    }
}
