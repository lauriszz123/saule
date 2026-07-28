//! Markdown rendering for hover popups and `saule doc` output.
//!
//! Kept in this crate rather than in the LSP so the CLI renders docs
//! identically — one description of what a doc comment looks like when
//! shown to a human.

use crate::DocBlock;

impl DocBlock {
    /// Render the block as Markdown: summary, then a parameter list,
    /// then the return description. Sections with nothing in them are
    /// omitted entirely, so a bare one-line summary renders as exactly
    /// that one line.
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();

        if !self.summary.trim().is_empty() {
            out.push_str(self.summary.trim_end());
        }

        if !self.params.is_empty() {
            section(&mut out);
            for p in &self.params {
                if p.desc.is_empty() {
                    out.push_str(&format!("- `{}`\n", p.name));
                } else {
                    out.push_str(&format!("- `{}` — {}\n", p.name, inline(&p.desc)));
                }
            }
            // Drop the trailing newline the loop leaves behind.
            out.pop();
        }

        if let Some(r) = &self.returns
            && !r.trim().is_empty()
        {
            section(&mut out);
            out.push_str(&format!("**Returns** — {}", inline(r)));
        }

        out
    }
}

/// Start a new block, separated from whatever came before by a blank
/// line (and by nothing at all when the output is still empty).
fn section(out: &mut String) {
    if !out.is_empty() {
        out.push_str("\n\n");
    }
}

/// Flatten a multi-line description onto one logical line so it sits
/// inside a list item without breaking the list. Paragraph breaks
/// become a space — hover popups are not the place for prose structure.
fn inline(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}
