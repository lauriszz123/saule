//! Hover cards for the standard library.
//!
//! The other renderers in this module build a signature out of a
//! [`NativeSig`], which carries types and nothing else — so a hover on
//! `String.sub` read `(string, integer, integer?) -> string`: what it takes,
//! never what it is or what the arguments mean.
//!
//! [`saule_docs::stdlib`] parses `DOCS.md`, which has the signature *with
//! parameter names* plus the prose and the example CI already checks. These
//! functions lay that out as a hover card.
//!
//! The layout follows the one the rest of the hover module uses — a fenced
//! signature, a `---` rule, then Markdown — so a stdlib hover and a hover on
//! the user's own documented class look like the same tool rendered them.
//!
//! [`NativeSig`]: saule_typeck::sigs::NativeSig

use saule_docs::{EntryKind, StdlibEntry, StdlibModule};

/// The full card for one stdlib function, method or constant.
///
/// `fallback_sig` is the signature rendered from the type registry. It is used
/// when the documentation has no entry for this name, so a newly-added native
/// still hovers with its types rather than with nothing — and the missing
/// prose is a visible prompt to write it.
pub(crate) fn render_stdlib_entry(entry: &StdlibEntry) -> String {
    let mut s = String::with_capacity(entry.body.len() + 160);

    // The signature, as documented. `fn` prefixes a call and not a constant,
    // matching how each is written in Saule.
    s.push_str("```saule\n");
    if entry.kind == EntryKind::Function {
        s.push_str("fn ");
    }
    s.push_str(&entry.signature);
    s.push_str("\n```\n\n---\n\n");

    // A context line, so the reader knows where in the library they are
    // without reading the prose: `String` · *Searching*.
    if let Some(owner) = &entry.owner {
        s.push_str("**");
        s.push_str(owner);
        s.push_str("**");
        if let Some(group) = &entry.group {
            s.push_str(" · *");
            s.push_str(group);
            s.push('*');
        }
        if entry.is_method {
            s.push_str(" · instance method");
        }
        s.push_str("\n\n");
    } else {
        s.push_str("**Prelude** · *always in scope*\n\n");
    }

    // The body's own links are written for a reader of the whole file
    // (`](#string-indexof)`), which resolves to nothing in a hover panel.
    s.push_str(
        saule_docs::stdlib()
            .absolute_links(entry.body.trim())
            .trim(),
    );

    // The reference page carries the group's shared prose — the pattern
    // syntax tables, the `bytes` vs `string` discussion — which no single
    // entry's body repeats.
    s.push_str("\n\n[Full reference →](");
    s.push_str(&entry.url);
    s.push(')');
    s
}

/// The card for a stdlib module name — hovering the `String` in
/// `String.match`.
///
/// `members` is the rendered member list the type registry produces. The
/// module's own prose goes above it, because "what is this module for" is the
/// question a hover on its name is asking; the member list answers "what is
/// in it", which is the follow-up.
pub(crate) fn render_stdlib_module_doc(module: &StdlibModule, members: String) -> String {
    let mut s = String::with_capacity(module.body.len() + members.len() + 120);
    s.push_str(&members);
    s.push_str("\n\n---\n\n");
    s.push_str(
        saule_docs::stdlib()
            .absolute_links(module.body.trim())
            .trim(),
    );
    s.push_str("\n\n[Full reference →](");
    s.push_str(&module.url);
    s.push(')');
    s
}

/// A one-line summary for a completion item's detail field.
///
/// Completion lists show one row per candidate, so this is the signature
/// alone — the prose goes in the item's documentation, which editors render
/// in the side panel.
pub(crate) fn render_stdlib_detail(entry: &StdlibEntry) -> String {
    entry.signature.clone()
}

/// The documentation body for a completion item.
///
/// Shorter than the hover card: the summary and the link, without the
/// examples. A completion panel is read while typing, and scrolling it to
/// find the next candidate is worse than following the link.
pub(crate) fn render_stdlib_completion_doc(entry: &StdlibEntry) -> String {
    let mut s = String::with_capacity(entry.summary.len() + 120);
    if let Some(group) = &entry.group {
        s.push('*');
        s.push_str(group);
        s.push_str("*\n\n");
    }
    s.push_str(
        saule_docs::stdlib()
            .absolute_links(entry.summary.trim())
            .trim(),
    );
    s.push_str("\n\n[Full reference →](");
    s.push_str(&entry.url);
    s.push(')');
    s
}
