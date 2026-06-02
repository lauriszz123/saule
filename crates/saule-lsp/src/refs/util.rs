//! Shared utilities for the resolve and collect walkers — source
//! scanning by identifier word boundary, type-name extraction, local
//! binding record, etc.

use std::ops::Range;

use saule_ast::{Decl, Expr, Type};
use saule_semantic::with_classes;

/// One local binding tracked during the walk. `def_span` is the byte
/// range of the identifier at the declaration site so each binding has
/// stable identity even when an inner scope shadows the name.
#[derive(Clone)]
pub(super) struct LocalBind {
    pub(super) name: String,
    pub(super) def_span: Range<usize>,
    pub(super) ty: Type,
}

pub(super) fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Locate the first occurrence of `name` as a word (bounded by
/// non-identifier bytes) inside `source[range]`. Returns the absolute
/// byte range of the match, or `None` if `name` doesn't appear there
/// as a standalone identifier.
pub(super) fn locate_word_in(source: &str, range: &Range<usize>, name: &str) -> Option<Range<usize>> {
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

/// Find every occurrence of `name` as a word inside `source[range]`.
/// Used for collecting references in spans that contain multiple uses
/// (e.g. an entire module body for a workspace search).
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

pub(super) fn contains(r: &Range<usize>, o: usize) -> bool {
    r.start <= o && o <= r.end
}

/// Span of a member access's `.name` part: starts after `obj.span.end`
/// (which sits on or just after the dot). Falls back to a search
/// within a wider window when the member lies on a subsequent line
/// after a multi-line `obj` expression.
pub(super) fn member_name_span(
    source: &str,
    obj_end: usize,
    parent_end: usize,
    name: &str,
) -> Option<Range<usize>> {
    let range = obj_end..parent_end.max(obj_end);
    locate_word_in(source, &range, name)
}

pub(super) fn declared_name(d: &Decl) -> &str {
    match d {
        Decl::Function { name, .. }
        | Decl::Class { name, .. }
        | Decl::Interface { name, .. }
        | Decl::Enum { name, .. } => name,
        Decl::Import { .. } => "",
    }
}

pub(super) fn named_type(ty: &Type) -> Option<String> {
    match ty {
        Type::Named(n) => Some(n.clone()),
        Type::Nullable(inner) => named_type(inner),
        _ => None,
    }
}

pub(super) fn strip_nullable(ty: Type) -> Type {
    match ty {
        Type::Nullable(inner) => *inner,
        other => other,
    }
}

/// Lightweight inference for a scrutinee expression, mirroring the
/// hover module's logic. Only the cases needed for typing match-arm
/// bindings are covered — anything else falls through to `None`.
pub(super) fn inferred_type_of(
    init: &Expr,
    locals: &[LocalBind],
    enclosing_class: &Option<String>,
) -> Option<Type> {
    match init {
        Expr::Self_ => enclosing_class.as_ref().map(|c| Type::Named(c.clone())),
        Expr::Ident(n) => locals.iter().rev().find(|l| &l.name == n).map(|l| l.ty.clone()),
        Expr::Call { callee, .. } => {
            if let Expr::Ident(n) = &callee.value
                && with_classes(|r| r.contains_key(n))
            {
                return Some(Type::Named(n.clone()));
            }
            None
        }
        _ => None,
    }
}

/// Find the byte range of `path`'s string-literal occurrence inside an
/// `import "…"` statement. Looks for the first quote after the start
/// of `range`, then matches a closing quote at `start + path.len()+1`.
pub(super) fn locate_string_literal(
    source: &str,
    range: &Range<usize>,
    path: &str,
) -> Option<Range<usize>> {
    let bytes = source.as_bytes();
    let end = range.end.min(bytes.len());
    let mut i = range.start.min(end);
    while i < end {
        if bytes[i] == b'"' {
            let start = i + 1;
            let stop = start + path.len();
            if stop < bytes.len() && bytes[stop] == b'"' {
                let candidate = source.get(start..stop)?;
                if candidate == path {
                    return Some(start..stop);
                }
            }
        }
        i += 1;
    }
    None
}
