//! Checking doc comments against the declarations they document.
//!
//! Only one rule for now, and it's the one that catches real bugs: an
//! `@param` naming a parameter that doesn't exist. That fires on typos
//! (`@param widht`) and, more usefully, on renames — you change a
//! parameter and the stale tag lights up instead of quietly describing
//! something that is no longer there.
//!
//! The reverse check — a parameter with no `@param` — is deliberately
//! *not* reported. It would fire constantly on half-written code and on
//! functions whose parameters are self-evident.

use std::ops::Range;

use crate::{extract, index::walk};
use saule_ast::Module;

/// A problem found in a doc comment. Carries a byte range so the caller
/// can convert it to whatever diagnostic type it publishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocWarning {
    pub message: String,
    /// Byte range of the offending `@param` name.
    pub span: Range<usize>,
}

/// Check every doc comment in `module`.
pub fn validate(module: &Module, source: &str) -> Vec<DocWarning> {
    let mut out = Vec::new();

    for item in walk(module) {
        let Some(block) = extract(source, item.anchor) else {
            continue;
        };
        for doc in &block.params {
            if item.params.iter().any(|p| p.name == doc.name) {
                continue;
            }
            let known: Vec<&str> = item.params.iter().map(|p| p.name.as_str()).collect();
            out.push(DocWarning {
                message: unknown_param_message(&item.qname, &doc.name, &known),
                span: doc.name_span.clone(),
            });
        }
    }

    out
}

/// Phrase the warning so it names what *is* available — the fix is
/// almost always one of the listed names.
fn unknown_param_message(qname: &str, name: &str, known: &[&str]) -> String {
    if known.is_empty() {
        format!("`@param {name}` — `{qname}` takes no parameters")
    } else {
        format!(
            "`@param {name}` — `{qname}` has no parameter `{name}` (expected one of: {})",
            known
                .iter()
                .map(|n| format!("`{n}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}
