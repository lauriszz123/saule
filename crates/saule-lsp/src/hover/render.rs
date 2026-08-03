//! Markdown rendering helpers — every render_* function that turns
//! AST / registry entries into the fenced-code-block blurbs editors
//! display in the hover popup.

mod decls;
mod types;

pub(crate) use decls::*;
pub(crate) use types::*;

use saule_docs::DocBlock;

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
