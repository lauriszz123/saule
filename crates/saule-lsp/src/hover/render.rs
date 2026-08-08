//! Markdown rendering helpers — every render_* function that turns
//! AST / registry entries into the fenced-code-block blurbs editors
//! display in the hover popup.

mod decls;
mod types;

pub(crate) use decls::*;
pub(crate) use types::*;

use saule_docs::DocBlock;

/// Widest signature we render on one line before breaking it to one
/// parameter per line.
///
/// A hover popup soft-wraps rather than scrolling, and it wraps to
/// column 0 — so a 16-parameter `copyWith` folds into a justified
/// paragraph whose continuation lines start further left than the `fn`
/// they belong to, and nothing marks where the signature ends and the
/// next member begins. Breaking it ourselves costs vertical space and
/// buys back the one thing the popup is for.
pub(crate) const SIGNATURE_WIDTH_BUDGET: usize = 76;

/// Lay out `prefix(params)suffix`, one parameter per line if the whole
/// thing would overflow [`SIGNATURE_WIDTH_BUDGET`].
///
/// `indent` is the column the first line already starts at — 0 for a
/// standalone `fn` hover, 2 for a member inside a class blurb — so the
/// budget is measured against what the reader actually sees, and the
/// closing `)` lands under the `f` of `fn` rather than under the params.
pub(super) fn render_call_shape(
    prefix: &str,
    params: &[String],
    suffix: &str,
    indent: usize,
) -> String {
    let width = |s: &str| s.chars().count();
    // prefix + "(" + params joined by ", " + ")" + suffix
    let inline = indent
        + width(prefix)
        + 2
        + params
            .iter()
            .map(|p| width(p) + 2)
            .sum::<usize>()
            .saturating_sub(2)
        + width(suffix);

    if params.is_empty() || inline <= SIGNATURE_WIDTH_BUDGET {
        return format!("{prefix}({}){suffix}", params.join(", "));
    }

    let mut s = format!("{prefix}(\n");
    for p in params {
        s.push_str(&" ".repeat(indent + 2));
        s.push_str(p);
        s.push_str(",\n");
    }
    s.push_str(&" ".repeat(indent));
    s.push(')');
    s.push_str(suffix);
    s
}

/// Append a declaration's `---` doc comment below its rendered
/// signature, separated by a horizontal rule so the popup reads as
/// "here's the shape, here's what it means".
///
/// A missing or empty block leaves `md` untouched, which is what keeps
/// undocumented code hovering exactly as it did before.
pub(super) fn with_doc(md: String, doc: Option<&DocBlock>) -> String {
    match doc {
        Some(d) if !d.is_empty() => format!("{md}\n\n---\n\n{}", d.to_markdown()),
        _ => md,
    }
}

/// Append the `@param <name>` description to a parameter's hover.
///
/// Only the prose for this one parameter is shown — the enclosing
/// function's summary belongs on the function, not on every parameter
/// inside it.
pub(super) fn with_param_doc(md: String, doc: Option<&DocBlock>, name: &str) -> String {
    match doc.and_then(|d| d.param(name)) {
        Some(desc) if !desc.trim().is_empty() => format!("{md}\n\n---\n\n{desc}"),
        _ => md,
    }
}
