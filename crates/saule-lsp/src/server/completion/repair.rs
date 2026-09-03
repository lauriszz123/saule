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

/// The keyword the caret's line is waiting for, with the label to show it
/// under — or `None` when the line isn't waiting for one.
///
/// These are the positions the tree cannot answer, and they are all the same
/// shape: a construct whose *next* keyword hasn't been typed, so the parser
/// has already recovered past it and the sentinel has landed somewhere that
/// looks identical to the keyword having been there all along.
/// `class Foo ext…` reads as a malformed class member; `case P th…` and
/// `if c th…` read as the body having started. Nothing in the tree separates
/// those from the real thing — the line the caret sits on does.
///
/// Only the keywords are decided here. What comes *after* one of them
/// (`extends Ent…`, `case Col…`) is a real position with a real node, and
/// stays with the walk.
pub(crate) fn line_keywords(
    source: &str,
    offset: usize,
) -> Option<(Vec<&'static str>, &'static str)> {
    let line = caret_line(source, offset)?;

    // The word under the caret is still being typed; everything ahead of it
    // is what the author has committed to.
    let mut words: Vec<&str> = line.split_whitespace().collect();
    if !line.ends_with(char::is_whitespace) {
        words.pop();
    }

    if let Some(kws) = header_keywords(&words) {
        return Some((kws, "class header"));
    }
    // From the *last* opener on the line, so a one-line `match c case P …`
    // is read as the arm it ends in rather than the match it starts with.
    let tail = words
        .iter()
        .rposition(|w| matches!(*w, "case" | "if" | "elseif" | "while" | "for"))
        .map(|i| &words[i..])?;
    match tail.first() {
        Some(&"case") => arm_keywords(tail).map(|k| (k, "match arm")),
        Some(&"while") | Some(&"for") => loop_keywords(tail).map(|k| (k, "loop header")),
        _ => condition_keywords(tail).map(|k| (k, "condition")),
    }
}

/// The caret's line, with strings and a trailing comment removed, or `None`
/// when the caret is somewhere no keyword can be suggested.
///
/// Quoted text is blanked rather than dropped so a `"then"` inside a string
/// can't be mistaken for the keyword, and so brackets inside one don't
/// unbalance the count. An unclosed bracket means the caret is inside a
/// sub-expression — a payload pattern, a parenthesised condition — where the
/// construct's own next keyword is not what comes next.
fn caret_line(source: &str, offset: usize) -> Option<String> {
    let before = source.get(..offset)?;
    let raw = &before[before.rfind('\n').map(|i| i + 1).unwrap_or(0)..];

    let mut out = String::with_capacity(raw.len());
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for c in raw.chars() {
        match quote {
            Some(q) => {
                out.push(' ');
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == q {
                    quote = None;
                }
            }
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                out.push(' ');
            }
            None => out.push(c),
        }
    }
    // Inside an unterminated string there is no keyword position at all.
    if quote.is_some() {
        return None;
    }
    // A `--` before the caret puts the caret *inside* the comment, where
    // nothing is being written but prose.
    if out.contains("--") {
        return None;
    }

    let opens = out.chars().filter(|c| "([{".contains(*c)).count();
    let closes = out.chars().filter(|c| ")]}".contains(*c)).count();
    if opens != closes {
        return None;
    }
    Some(out)
}

/// `class Foo <caret>` / `interface Bar <caret>` — the header keywords not
/// yet written.
fn header_keywords(words: &[&str]) -> Option<Vec<&'static str>> {
    let mut words = words;
    if words.first() == Some(&"export") {
        words = &words[1..];
    }
    let is_class = match words.first() {
        Some(&"class") => true,
        Some(&"interface") => false,
        _ => return None,
    };

    // The name has to be there already — in `class F…` the author is
    // inventing it, and no suggestion can help with that.
    let name = words.get(1)?;
    // An unclosed generic list means the caret is naming a type parameter.
    if name.matches('<').count() != name.matches('>').count() {
        return None;
    }

    let rest = &words[2..];
    let has = |kw: &str| rest.iter().any(|w| *w == kw);
    // Straight after `extends` / `implements` / a comma a *type* is wanted.
    if rest
        .last()
        .is_some_and(|w| *w == "extends" || *w == "implements" || w.ends_with(','))
    {
        return None;
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
    (!out.is_empty()).then_some(out)
}

/// `case <pattern> <caret>` — a guard, or the body. Both are still open
/// until one of them is written.
fn arm_keywords(words: &[&str]) -> Option<Vec<&'static str>> {
    // The pattern has to be finished. `case <caret>` and `case Colour.<caret>`
    // are pattern positions, which the walk answers with real variants.
    if words.len() < 2 {
        return None;
    }
    // Past `then` the arm's body has started, and neither keyword has a
    // second place to go.
    if words.iter().any(|w| *w == "then") {
        return None;
    }
    match words.last() {
        // `when <caret>` wants the guard expression, not another keyword.
        Some(&"when") => None,
        // A guard is already written, so only the body is still to come.
        _ if words.iter().any(|w| *w == "when") => Some(vec!["then"]),
        _ => Some(vec!["when", "then"]),
    }
}

/// `while <cond> <caret>` / `for <binding> in <iter> <caret>` — the `do` that
/// opens the loop body.
///
/// A loop header only reaches its `do` once it is complete, and the two
/// `for` forms complete differently: `for v in iter` needs its `in`, and the
/// numeric `for i = from, to` needs the comma that separates the bounds.
/// Until then the author is still writing the header, and `do` is not what
/// comes next — though for a `for` that has named its variables and
/// committed to neither form, `in` is.
///
/// The *other* `do` — the one opening a trailing block, `Canvas() do … end` —
/// is not offered here. Whether a call can take one depends on the callee's
/// last parameter being a function, which is a question for the registries
/// rather than for the line.
fn loop_keywords(words: &[&str]) -> Option<Vec<&'static str>> {
    // Past `do` the body has started.
    if words.iter().any(|w| *w == "do") {
        return None;
    }
    // Something has to have been written to loop over.
    if words.len() < 2 {
        return None;
    }
    // A dangling `in` / `=` / `,` means the next thing is an expression, and
    // a dangling `:` means the next thing is the loop variable's type.
    if words.last().is_some_and(|w| {
        *w == "in" || *w == "=" || w.ends_with(',') || w.ends_with('=') || w.ends_with(':')
    }) {
        return None;
    }
    if words[0] == "for" {
        let numeric = words.iter().any(|w| w.contains('='));
        let for_in = words.iter().any(|w| *w == "in");
        // Neither form has committed yet: the variables are named, so what
        // comes next is the `in` that says what to loop over. (`= from, to`
        // is the other way through, but that is punctuation rather than a
        // word, and nothing to suggest.)
        if !numeric && !for_in {
            return Some(vec!["in"]);
        }
        // `for i = 1 <caret>` still owes its upper bound.
        if numeric && !words.iter().any(|w| w.contains(',')) {
            return None;
        }
    }
    Some(vec!["do"])
}

/// `if <cond> <caret>` / `elseif <cond> <caret>` — the `then` that opens the
/// branch. `when` has no place here: it guards a match arm, nothing else.
fn condition_keywords(words: &[&str]) -> Option<Vec<&'static str>> {
    // The condition has to be there, and `then` not already written.
    if words.len() < 2 || words.iter().any(|w| *w == "then") {
        return None;
    }
    Some(vec!["then"])
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
