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

/// The header keywords still available at `offset`, or empty when the caret
/// isn't in a `class` / `interface` header.
///
/// This is the one position the tree cannot answer. Until `extends` is typed
/// the parser has already entered the class body, so `class Foo ext…` leaves
/// the sentinel as a malformed member — the very same shape a field being
/// named on the *next* line produces, and `ClassMember::Field` carries no
/// span to tell the two apart. The line the caret sits on is what separates
/// them, so that is what this reads.
///
/// Only the keywords are decided here. `extends Ent…` is a type position and
/// stays with the walk, which knows which classes would close a cycle.
pub(crate) fn header_keywords(source: &str, offset: usize) -> Vec<&'static str> {
    let Some(before) = source.get(..offset) else {
        return Vec::new();
    };
    let line = &before[before.rfind('\n').map(|i| i + 1).unwrap_or(0)..];
    // A commented-out header is not a header.
    if line.contains("--") {
        return Vec::new();
    }

    // The word under the caret is still being typed; everything ahead of it
    // is what the author has committed to.
    let mut words: Vec<&str> = line.split_whitespace().collect();
    if !line.ends_with(char::is_whitespace) {
        words.pop();
    }

    let mut words = words.as_slice();
    if words.first() == Some(&"export") {
        words = &words[1..];
    }
    let is_class = match words.first() {
        Some(&"class") => true,
        Some(&"interface") => false,
        _ => return Vec::new(),
    };

    // The name has to be there already — in `class F…` the author is
    // inventing it, and no suggestion can help with that.
    let Some(name) = words.get(1) else {
        return Vec::new();
    };
    // An unclosed generic list means the caret is naming a type parameter.
    if name.matches('<').count() != name.matches('>').count() {
        return Vec::new();
    }

    let rest = &words[2..];
    let has = |kw: &str| rest.iter().any(|w| *w == kw);
    // Straight after `extends` / `implements` / a comma a *type* is wanted.
    if rest
        .last()
        .is_some_and(|w| *w == "extends" || *w == "implements" || w.ends_with(','))
    {
        return Vec::new();
    }

    let mut out = Vec::new();
    // `extends` comes first in the header, so once `implements` is written
    // there is no longer a place to put it.
    if !has("extends") && !has("implements") {
        out.push("extends");
    }
    if is_class && !has("implements") {
        out.push("implements");
    }
    out
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
