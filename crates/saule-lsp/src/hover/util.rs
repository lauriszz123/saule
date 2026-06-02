//! Tiny helpers shared across the hover modules — span containment,
//! type-name extraction, member resolution against the semantic
//! registries.

use std::ops::Range;

use saule_ast::Type;
use saule_semantic::{lookup_field_type, lookup_method};

use super::render::{render_method_sig, render_native_sig_full, render_type};

pub(super) fn contains(r: &Range<usize>, o: usize) -> bool {
    r.start <= o && o <= r.end
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
pub(super) fn resolve_member(class: &str, name: &str, is_call: bool) -> Option<String> {
    if is_call {
        if let Some(sig) = lookup_method(class, name) {
            return Some(render_method_sig(class, name, &sig));
        }
    }
    if let Some(ty) = lookup_field_type(class, name) {
        return Some(format!(
            "```saule\n(field) {class}.{name}: {ty}\n```",
            ty = render_type(&ty)
        ));
    }
    if let Some(sig) = lookup_method(class, name) {
        return Some(render_method_sig(class, name, &sig));
    }
    // Stdlib fallback: `Math.sqrt`, `String.byte`, etc.
    let qname = format!("{class}.{name}");
    if let Some(sig) = saule_typeck::sigs::lookup(&qname) {
        return Some(format!(
            "```saule\nfn {qname}{}\n```",
            render_native_sig_full(&sig)
        ));
    }
    if saule_typeck::sigs::has_member(class, name) {
        // Member is known but its signature wasn't registered (typed
        // value field like `Math.pi`). We can't say more than "yes,
        // it exists" — better than silent hover-fail though.
        return Some(format!("```saule\n(member) {class}.{name}\n```"));
    }
    None
}

pub(super) fn named_type(ty: &Type) -> Option<String> {
    match ty {
        Type::Named(n) => Some(n.clone()),
        Type::Nullable(inner) => named_type(inner),
        _ => None,
    }
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
