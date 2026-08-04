//! Shared utilities for the resolve and collect walkers — source
//! scanning by identifier word boundary, type-name extraction, local
//! binding record, etc.

use std::ops::Range;

use saule_ast::{Decl, Expr, Spanned, Stmt, Type};
use saule_semantic::{with_classes, with_enums, with_interfaces};

use super::Symbol;

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

/// The class that actually declares method `name`, walking up from
/// `class` through its ancestors.
///
/// The receiver's static type is only where lookup *starts*: `c.greet()`
/// on a `Child` whose `greet` comes from `Base` has to navigate to
/// `Base`, and find-references has to count it as a `Base::greet` use.
/// Keying the symbol on the receiver's class instead left inherited
/// members with no definition site at all.
pub(super) fn method_owner(class: &str, name: &str) -> Option<String> {
    with_classes(|reg| {
        let mut cur = Some(class.to_string());
        while let Some(cname) = cur {
            let info = reg.get(&cname)?;
            if info.methods.contains_key(name) {
                return Some(cname);
            }
            cur = info.parent.clone();
        }
        None
    })
}

/// Same as [`method_owner`] for a field.
pub(super) fn field_owner(class: &str, name: &str) -> Option<String> {
    with_classes(|reg| {
        let mut cur = Some(class.to_string());
        while let Some(cname) = cur {
            let info = reg.get(&cname)?;
            if info.field_types.contains_key(name) {
                return Some(cname);
            }
            cur = info.parent.clone();
        }
        None
    })
}

/// Byte range of the variable bound by a `try … catch <var>` clause.
///
/// The AST keeps no span for it, and scanning the whole statement finds
/// the wrong word whenever the `try` body declares a local of the same
/// name — so the scan starts after the last body statement, where the
/// `catch` clause always is.
pub(super) fn catch_var_span(
    source: &str,
    stmt_span: &Range<usize>,
    body: &[Spanned<Stmt>],
    catch_var: &str,
) -> Range<usize> {
    let start = body
        .last()
        .map(|s| s.span.end)
        .unwrap_or(stmt_span.start)
        .max(stmt_span.start);
    locate_word_in(source, &(start..stmt_span.end), catch_var).unwrap_or_else(|| stmt_span.clone())
}

/// Every named type head reachable from `ty` — `table<K, Player>` and
/// `fn(Point) -> Color?` all contribute their class-ish names, so a
/// cursor on any of them can navigate.
pub(super) fn named_type_heads(ty: &Type, out: &mut Vec<String>) {
    match ty {
        Type::Named(n) => out.push(n.clone()),
        Type::Nullable(inner) => named_type_heads(inner, out),
        Type::Table { key, value } => {
            if let Some(k) = key {
                named_type_heads(k, out);
            }
            named_type_heads(value, out);
        }
        Type::Tuple(items) => {
            for it in items {
                named_type_heads(it, out);
            }
        }
        Type::Function { params, ret } => {
            for p in params {
                named_type_heads(p, out);
            }
            named_type_heads(ret, out);
        }
    }
}

/// `true` when `name` matches no stdlib module, value type, or native
/// function — leaving "free function declared somewhere in this
/// workspace" as the only thing it can be.
pub(super) fn is_workspace_function_name(name: &str) -> bool {
    !saule_typeck::sigs::is_module(name)
        && !saule_typeck::sigs::is_value_type(name)
        && saule_typeck::sigs::lookup(name).is_none()
}

/// The symbol a name written in type position denotes, or `None` for
/// primitives, generic type parameters, and anything else the semantic
/// registries don't know.
pub(super) fn type_name_symbol(name: &str) -> Option<Symbol> {
    if with_classes(|r| r.contains_key(name)) {
        Some(Symbol::Class(name.to_string()))
    } else if with_interfaces(|r| r.contains_key(name)) {
        Some(Symbol::Interface(name.to_string()))
    } else if with_enums(|r| r.contains_key(name)) {
        Some(Symbol::Enum(name.to_string()))
    } else {
        None
    }
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
        Expr::Ident(n) => locals
            .iter()
            .rev()
            .find(|l| &l.name == n)
            .map(|l| l.ty.clone()),
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

/// Find the byte range of the module path inside an `import … from …`
/// declaration, for either spelling of the path.
///
/// `from "a/b"` sits between quotes; `from a.b` is bare, and the parser ends
/// the declaration's span at its last segment, so the bare path is the span's
/// suffix. Only handling the quoted form is what used to make goto-definition
/// silently do nothing on `import * from Geometry`.
pub(super) fn locate_import_path(
    source: &str,
    range: &Range<usize>,
    path: &str,
    quoted: bool,
) -> Option<Range<usize>> {
    if quoted {
        return locate_string_literal(source, range, path);
    }

    let end = range.end.min(source.len());
    if let Some(start) = end.checked_sub(path.len())
        && start >= range.start
        && source.get(start..end) == Some(path)
    {
        return Some(start..end);
    }

    // Whitespace between segments (`from a . b`) makes the joined path differ
    // from the source text; fall back to everything after the `from` keyword.
    let slice = source.get(range.start..end)?;
    let after_from = range.start + slice.rfind("from")? + "from".len();
    let tail = source.get(after_from..end)?;
    let start = after_from + (tail.len() - tail.trim_start().len());

    if start < end { Some(start..end) } else { None }
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
