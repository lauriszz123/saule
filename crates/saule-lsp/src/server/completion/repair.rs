//! Parsing a buffer mid-keystroke.
//!
//! A sentinel identifier is spliced in at the cursor so the
//! incomplete expression parses; everything downstream then works on
//! a real AST and strips the sentinel back out.

use saule_ast::{Module, Type};
use saule_parser::PriorShape;

/// Identifier spliced in at the caret. Deliberately unlikely to collide with
/// real user code.
pub(crate) const SENTINEL: &str = "__saule_completion__";

/// Whether a path segment can appear in an unquoted import (`import x from
/// a.b.c`), where each segment has to look like an identifier.
pub(crate) fn is_ident_segment(seg: &str) -> bool {
    !seg.is_empty() && seg.chars().all(|c| c == '_' || c.is_alphanumeric())
}

/// How many blocks we're willing to close for the author.
pub(crate) const MAX_REPAIR: usize = 8;

/// A tree for the buffer, however far from valid it currently is.
///
/// Three tiers, best-shaped tree first:
///
/// 1. **As written.** Nothing to repair.
/// 2. **With the missing `end`s appended.** Code is written top-down, so the
///    `end` closing the declaration the caret sits in usually hasn't been
///    typed yet. Adding them back yields a tree that is *correct*, not merely
///    recovered — worth trying before anything guesses.
/// 3. **Recovered.** `parse_recover` always produces a tree, holes and all,
///    which covers the cases appending `end`s cannot: a broken line above the
///    caret, a stray token, a half-written type.
///
/// Only tier 1 can fire on valid input, so this can add suggestions but never
/// change existing ones.
///
/// `prior` is the document's last clean shape, which tier 3 uses to untangle
/// a forgotten `end`; `None` falls back to indentation alone.
pub(crate) fn parse_tolerant(src: &str, prior: Option<&PriorShape>) -> Option<Module> {
    if let Some(m) = crate::syntax::strict(src) {
        return Some(m);
    }
    let mut patched = src.to_string();
    for _ in 0..MAX_REPAIR {
        patched.push_str("\nend");
        if let Some(m) = crate::syntax::strict(&patched) {
            return Some(m);
        }
    }
    Some(crate::syntax::tolerant_with_prior(src, prior))
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
pub(crate) fn without_sentinel(refs: &[saule_ast::TypeRef]) -> Vec<String> {
    refs.iter()
        .filter(|r| r.name != SENTINEL)
        .map(|r| r.name.clone())
        .collect()
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
        Type::Generic(g) => g.name == SENTINEL || g.args.iter().any(type_mentions_sentinel),
    }
}

// ─── receiver inference ─────────────────────────────────────────────────────
