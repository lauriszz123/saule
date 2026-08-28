//! Tiny helpers shared across the hover modules — span containment,
//! type-name extraction, member resolution against the semantic
//! registries.

use std::ops::Range;

use saule_ast::Type;
use saule_semantic::{lookup_field_type, lookup_method};

use super::render::{render_method_sig, render_native_sig_full, render_type, with_doc};

pub(super) fn contains(r: &Range<usize>, o: usize) -> bool {
    r.start <= o && o <= r.end
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Locate the first occurrence of `name` as a word (bounded by
/// non-identifier bytes) inside `source[range]`. Returns the absolute
/// byte range of the match.
pub(super) fn locate_word_in(
    source: &str,
    range: &Range<usize>,
    name: &str,
) -> Option<Range<usize>> {
    let end = range.end.min(source.len());
    let start = range.start.min(end);
    let slice = source.get(start..end)?;
    let bytes = slice.as_bytes();
    let pat = name.as_bytes();
    if pat.is_empty() || pat.len() > bytes.len() {
        return None;
    }
    let mut i = 0;
    while i + pat.len() <= bytes.len() {
        if &bytes[i..i + pat.len()] == pat {
            let before_ok = i == 0 || !is_ident_byte(bytes[i - 1]);
            let after_ok = i + pat.len() == bytes.len() || !is_ident_byte(bytes[i + pat.len()]);
            if before_ok && after_ok {
                return Some((start + i)..(start + i + pat.len()));
            }
        }
        i += 1;
    }
    None
}

/// Like [`locate_word_in`] but returns every occurrence — used when
/// the same name may appear more than once inside a span (e.g. a
/// comma-separated `implements A, B, C` list).
#[allow(dead_code)]
pub(super) fn locate_words_in(source: &str, range: &Range<usize>, name: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let end = range.end.min(source.len());
    let start = range.start.min(end);
    let Some(slice) = source.get(start..end) else {
        return out;
    };
    let bytes = slice.as_bytes();
    let pat = name.as_bytes();
    if pat.is_empty() || pat.len() > bytes.len() {
        return out;
    }
    let mut i = 0;
    while i + pat.len() <= bytes.len() {
        if &bytes[i..i + pat.len()] == pat {
            let before_ok = i == 0 || !is_ident_byte(bytes[i - 1]);
            let after_ok = i + pat.len() == bytes.len() || !is_ident_byte(bytes[i + pat.len()]);
            if before_ok && after_ok {
                out.push((start + i)..(start + i + pat.len()));
                i += pat.len();
                continue;
            }
        }
        i += 1;
    }
    out
}

// ──────────────────────────────────────────────────────────────────────────────
// Identifier / member resolution
// ──────────────────────────────────────────────────────────────────────────────

// ──────────────────────────────────────────────────────────────────────────────
// Member resolution
// ──────────────────────────────────────────────────────────────────────────────

/// Look up a member or method on `class` and render it. `is_call`
/// nudges the formatter toward the method shape when both a method and
/// a same-named field exist (rare, but `lookup_method` returns first).
pub(super) fn resolve_member(
    class: &str,
    name: &str,
    is_call: bool,
    docs: &saule_docs::DocIndex,
) -> Option<String> {
    // Docs come from the index rather than the registries: a member is
    // resolved here through `lookup_method` / `lookup_field_type`,
    // which carry types but not the source text a `---` block lives in.
    let doc = docs.get(&format!("{class}.{name}"));

    // `CrossAxisAlignment.Stretch` — an enum variant, not a class
    // member. Enums reach `receiver_class` on equal terms with classes,
    // so without this the lookups below all miss and the hover reports
    // `(unknown)` on a name the reader can see declared.
    if let Some(variant) = enum_variant(class, name) {
        return Some(with_doc(variant, doc));
    }

    // An untyped bag takes any key, so "no member" would be false —
    // `scratch.sound` on a `scratch: table` is perfectly good code.
    // Answering here also keeps a wider node from supplying a hover
    // about some unrelated symbol, which is the job `render_unknown_member`
    // does for receivers that really do have a fixed set of members.
    if matches!(class, "table" | "any") {
        return Some(format!(
            "```saule\n(key) {name}: any\n```\nRead off an untyped `{class}`, which accepts any key."
        ));
    }

    if is_call && let Some(sig) = lookup_method(class, name) {
        return Some(with_doc(render_method_sig(class, name, &sig), doc));
    }
    if let Some(ty) = lookup_field_type(class, name) {
        return Some(with_doc(
            format!(
                "```saule\n(field) {class}.{name}: {ty}\n```",
                ty = render_type(&ty)
            ),
            doc,
        ));
    }
    if let Some(sig) = lookup_method(class, name) {
        return Some(with_doc(render_method_sig(class, name, &sig), doc));
    }
    // Stdlib fallback: `Math.sqrt`, `String.byte`, etc.
    let qname = format!("{class}.{name}");
    if let Some(sigs) = saule_typeck::sigs::lookup_all(&qname) {
        // An overloaded native gets a line per form. Showing only the first
        // would under-report it: `Math.random` really does return a float
        // with no arguments and an integer with bounds, and a hover naming
        // one of those hides the call the reader is looking at.
        let body = sigs
            .iter()
            .map(|sig| format!("fn {qname}{}", render_native_sig_full(sig)))
            .collect::<Vec<_>>()
            .join("\n");
        return Some(format!("```saule\n{body}\n```"));
    }
    if saule_typeck::sigs::has_member(class, name) {
        // Member is known but its signature wasn't registered (typed
        // value field like `Math.pi`). We can't say more than "yes,
        // it exists" — better than silent hover-fail though.
        return Some(format!("```saule\n(member) {class}.{name}\n```"));
    }
    None
}

/// Render `Enum.Variant` when `owner` is an enum that declares
/// `variant`, carrying the payload arity for a tuple-style one.
///
/// `None` when `owner` isn't an enum, or doesn't have that variant —
/// the latter still being a real miss worth reporting as such.
fn enum_variant(owner: &str, variant: &str) -> Option<String> {
    let info = saule_semantic::with_enums(|r| r.get(owner).cloned())?;
    let (_, shape) = info.variants.iter().find(|(n, _)| n.as_str() == variant)?;

    let mut s = format!("```saule\n(variant) {owner}.{variant}");
    if shape.arity() > 0 {
        // The declared payload, named and typed the way the enum spells it —
        // which is what tells you the order to destructure in.
        let fields: Vec<String> = shape
            .fields
            .iter()
            .map(|f| format!("{}: {}", f.name, super::render::render_type(&f.ty)))
            .collect();
        s.push('(');
        s.push_str(&fields.join(", "));
        s.push(')');
    }
    s.push_str("\n```");
    Some(s)
}

/// The hover for a member that its receiver's class does not declare.
///
/// This is the answer typeck would give at the same position, and it
/// exists so the walker has *something* to record at the member's own
/// span: staying silent lets a wider enclosing node supply the hover
/// instead, which presents a confident description of an unrelated
/// symbol. Naming the miss is both more useful and less wrong.
pub(super) fn render_unknown_member(class: &str, name: &str) -> String {
    format!("```saule\n(unknown) {class}.{name}\n```\nNo member `{name}` on `{class}`.")
}

pub(super) fn named_type(ty: &Type) -> Option<String> {
    match ty {
        Type::Named(n) => Some(n.clone()),
        Type::Nullable(inner) => named_type(inner),
        _ => None,
    }
}

/// Walk a [`Type`] and append every named-type identifier reachable
/// from it (head names of `Named`, components of `Tuple`,
/// `Nullable`'s inner, `table<K, V>`'s key/value, function param /
/// return types) into `out`. Used by the hover walker to find every
/// type ident worth recording a hover hit for inside a written
/// ascription, without having to thread per-fragment spans through
/// the AST.
pub(super) fn collect_named_heads(ty: &Type, out: &mut Vec<String>) {
    match ty {
        Type::Named(n) => out.push(n.clone()),
        Type::Nullable(inner) => collect_named_heads(inner, out),
        Type::Table { key, value } => {
            if let Some(k) = key {
                collect_named_heads(k, out);
            }
            collect_named_heads(value, out);
        }
        Type::Tuple(items) => {
            for it in items {
                collect_named_heads(it, out);
            }
        }
        Type::Function { params, ret } => {
            for p in params {
                collect_named_heads(p, out);
            }
            collect_named_heads(ret, out);
        }
    }
}

/// Names that the language treats as primitives — hovering them
/// shouldn't fire (no useful blurb, and would shadow the surrounding
/// node). Kept centralised so the type-ascription walker, identifier
/// resolver, and renderer all agree.
pub(super) fn is_primitive(name: &str) -> bool {
    matches!(
        name,
        "integer" | "float" | "string" | "boolean" | "nil" | "any" | "table" | "nothing"
    )
}

/// Peel a single `Nullable` wrapper. Used for `match` arm bindings:
/// the bound name is only reachable when the scrutinee wasn't nil, so
/// hover should surface `T` rather than `T?`. Mirrors
/// `saule_typeck::types::strip_nullable`, kept local so this crate
/// doesn't depend on that helper just for one call.
pub(super) fn strip_nullable_type(ty: Type) -> Type {
    match ty {
        Type::Nullable(inner) => *inner,
        other => other,
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Markdown rendering
// ──────────────────────────────────────────────────────────────────────────────
