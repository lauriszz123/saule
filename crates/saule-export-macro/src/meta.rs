//! Metadata records: the TOML payloads the macros compile into a package,
//! and the exported statics that carry them.
//!
//! The format is defined in `saule_native_abi` ("Package metadata"). In short:
//! one exported static per declaration, named `SAULE_META_<suffix>`, holding
//! `META_MAGIC | u32 length | TOML`. The interpreter reads these out of the
//! library file without loading it.

use proc_macro2::{Literal, TokenStream};
use quote::{format_ident, quote};
use saule_native_abi::{META_MAGIC, META_SYMBOL_PREFIX};

/// A record under construction: `key = value` lines in the order written.
pub(crate) struct Record {
    body: String,
}

impl Record {
    /// Start a record declaring a `kind` (`package`, `class`, `enum`,
    /// `method`).
    pub(crate) fn new(kind: &str) -> Self {
        let mut r = Record {
            body: String::new(),
        };
        r.str("kind", kind);
        r
    }

    pub(crate) fn str(&mut self, key: &str, value: &str) -> &mut Self {
        self.body
            .push_str(&format!("{key} = {}\n", toml_string(value)));
        self
    }

    /// A string key written only when there is something to say — a doc
    /// comment, chiefly. Absent and empty read the same to the interpreter.
    pub(crate) fn opt_str(&mut self, key: &str, value: Option<&str>) -> &mut Self {
        if let Some(v) = value.filter(|v| !v.is_empty()) {
            self.str(key, v);
        }
        self
    }

    pub(crate) fn int(&mut self, key: &str, value: i64) -> &mut Self {
        self.body.push_str(&format!("{key} = {value}\n"));
        self
    }

    pub(crate) fn bool(&mut self, key: &str, value: bool) -> &mut Self {
        self.body.push_str(&format!("{key} = {value}\n"));
        self
    }

    pub(crate) fn str_list(&mut self, key: &str, values: &[String]) -> &mut Self {
        let items: Vec<String> = values.iter().map(|v| toml_string(v)).collect();
        self.body
            .push_str(&format!("{key} = [{}]\n", items.join(", ")));
        self
    }

    /// The exported static carrying this record, named
    /// `SAULE_META_<suffix>`. `suffix` must be unique within the package;
    /// two declarations of the same thing collide at link time, which is the
    /// right outcome for a package that declares a class twice.
    pub(crate) fn emit(&self, suffix: &str) -> TokenStream {
        let mut bytes = META_MAGIC.to_vec();
        let len = u32::try_from(self.body.len()).expect("metadata record over 4 GiB");
        bytes.extend_from_slice(&len.to_le_bytes());
        bytes.extend_from_slice(self.body.as_bytes());
        let total = bytes.len();
        let lit = Literal::byte_string(&bytes);
        let ident = format_ident!("{}{}", META_SYMBOL_PREFIX, suffix);
        quote! {
            // Exported, not merely `#[used]`: the linker never discards an
            // exported symbol, and every object format lists exports, so the
            // interpreter can find this without platform-specific sections.
            #[doc(hidden)]
            #[allow(non_upper_case_globals)]
            #[unsafe(no_mangle)]
            pub static #ident: [u8; #total] = *#lit;
        }
    }
}

/// Encode `s` as a TOML basic string. Control characters are escaped, and
/// everything else — including any Unicode a doc comment carries — is
/// written as-is, which TOML permits.
pub(crate) fn toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\u{:04X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The doc comment on an item, as the author wrote it: `///` lines joined
/// with newlines, each stripped of the single space rustdoc convention puts
/// after the slashes. `None` when there is none.
///
/// This is what the editor shows on hover, so it is carried verbatim —
/// Markdown and all — rather than summarised.
pub(crate) fn doc_of(attrs: &[syn::Attribute]) -> Option<String> {
    let mut lines = Vec::new();
    for attr in attrs {
        if !attr.path().is_ident("doc") {
            continue;
        }
        let syn::Meta::NameValue(nv) = &attr.meta else {
            continue;
        };
        if let syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(s),
            ..
        }) = &nv.value
        {
            let v = s.value();
            lines.push(v.strip_prefix(' ').unwrap_or(&v).to_string());
        }
    }
    let joined = lines.join("\n");
    let trimmed = joined.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// `snake_case` → `lowerCamelCase`, the spelling Saule members use.
/// `set_pixel` → `setPixel`, `new_image_from_base64` → `newImageFromBase64`.
/// A leading underscore is dropped; an all-lowercase name is unchanged.
pub(crate) fn camel_case(snake: &str) -> String {
    let mut out = String::with_capacity(snake.len());
    let mut upper_next = false;
    for ch in snake.trim_start_matches('_').chars() {
        if ch == '_' {
            upper_next = !out.is_empty();
        } else if upper_next {
            out.extend(ch.to_uppercase());
            upper_next = false;
        } else {
            out.push(ch);
        }
    }
    out
}

/// Whether `s` is a name Saule can spell: an ASCII identifier. Every member
/// name ends up in a Saule program, and in a C symbol, so both have to
/// accept it.
pub(crate) fn is_saule_ident(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}
