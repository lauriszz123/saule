//! Parsing a buffer mid-keystroke.
//!
//! A sentinel identifier is spliced in at the cursor so the
//! incomplete expression parses; everything downstream then works on
//! a real AST and strips the sentinel back out.

use saule_ast::{Module, Type};

/// Identifier spliced in at the caret. Deliberately unlikely to collide with
/// real user code.
pub(crate) const SENTINEL: &str = "__saule_completion__";

/// Whether a path segment can appear in an unquoted import (`import x from
/// a.b.c`), where each segment has to look like an identifier.
pub(crate) fn is_ident_segment(seg: &str) -> bool {
    !seg.is_empty() && seg.chars().all(|c| c == '_' || c.is_alphanumeric())
}

pub(crate) fn parse(src: &str) -> Option<Module> {
    saule_lexer::Lexer::new(src)
        .tokenize()
        .ok()
        .and_then(|t| saule_parser::parse(t).ok())
}

/// How many blocks we're willing to close for the author.
pub(crate) const MAX_REPAIR: usize = 8;

/// Code is written top-down, so the `end` closing the declaration the caret
/// sits in usually hasn't been typed yet — a plain parse of a class being
/// written fails, and completion would have nothing to work with. Close the
/// open blocks artificially and retry. Only reached when the source doesn't
/// parse as-is, so this can add suggestions but never change existing ones.
pub(crate) fn parse_tolerant(src: &str) -> Option<Module> {
    if let Some(m) = parse(src) {
        return Some(m);
    }
    let mut patched = src.to_string();
    for _ in 0..MAX_REPAIR {
        patched.push_str("\nend");
        if let Some(m) = parse(&patched) {
            return Some(m);
        }
    }
    None
}

/// Replace the partial identifier under the caret with [`SENTINEL`],
/// returning the patched source and the text the user had typed.
pub(crate) fn splice_sentinel(source: &str, offset: usize) -> Option<(String, String)> {
    let before = source.get(..offset)?;
    let start = before
        .char_indices()
        .rev()
        .find(|(_, c)| !(*c == '_' || c.is_alphanumeric()))
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    let prefix = before[start..].to_string();

    let mut patched = String::with_capacity(source.len() + SENTINEL.len());
    patched.push_str(&source[..start]);
    patched.push_str(SENTINEL);
    patched.push_str(&source[offset..]);
    Some((patched, prefix))
}

// ─── what the caret can see ─────────────────────────────────────────────────

/// The names in a header list the author has already committed to.
pub(crate) fn without_sentinel(names: &[String]) -> Vec<String> {
    names.iter().filter(|n| *n != SENTINEL).cloned().collect()
}

pub(crate) fn type_mentions_sentinel(ty: &Type) -> bool {
    match ty {
        Type::Named(n) => n == SENTINEL,
        Type::Nullable(inner) => type_mentions_sentinel(inner),
        Type::Table { key, value } => {
            key.as_ref().is_some_and(|k| type_mentions_sentinel(k)) || type_mentions_sentinel(value)
        }
        Type::Tuple(items) => items.iter().any(type_mentions_sentinel),
        Type::Function { params, ret } => {
            params.iter().any(type_mentions_sentinel) || type_mentions_sentinel(ret)
        }
    }
}

// ─── receiver inference ─────────────────────────────────────────────────────
