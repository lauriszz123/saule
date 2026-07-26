//! Indentation declared by a project's `saule.config`.
//!
//! A project that writes
//!
//! ```text
//! indent_style: "tab"
//! indent_width: 4
//! ```
//!
//! gets the same layout from `saule fmt` and from the language server, so a
//! CLI run and a Reformat-on-save can't disagree about the file.
//!
//! Both keys are optional and anything malformed is reported through
//! [`ConfigIndent::warnings`] rather than silently changing the layout — a
//! typo'd `indent_style` reverting a whole project to the default is exactly
//! the failure this file exists to prevent.
//!
//! ## Precedence
//!
//! Lowest to highest:
//!
//! 1. [`FmtOptions::default`] — canonical Saule style, 2 spaces.
//! 2. The editor's LSP `FormattingOptions` (`tabSize` / `insertSpaces`).
//!    Language server only; the CLI has no editor to ask.
//! 3. `saule.config` — the project's declared style. It wins over the
//!    editor's Code Style page on purpose: the file belongs to the project,
//!    not to whoever opened it, and this is what keeps `saule fmt -w` and
//!    format-on-save producing byte-identical output.
//! 4. `saule fmt --indent` / `--tabs` / `--spaces` — an explicit, one-off
//!    override on the command line.
//!
//! The parser mirrors the minimal `key: value` format read by
//! `saule-cli/src/project.rs` and `saule-lsp/src/workspace.rs`: one pair per
//! line, `--` line comments, optional quotes around the value.

use std::fs;
use std::path::{Path, PathBuf};

use crate::FmtOptions;

/// Columns a hard tab is assumed to occupy when a config asks for tabs
/// without naming a width. Only affects the column arithmetic behind
/// line-breaking decisions, never the bytes written for one indent level.
const DEFAULT_TAB_WIDTH: usize = 4;

/// The `indent_style` / `indent_width` pair read out of a `saule.config`.
///
/// Both fields are `None` when the config doesn't mention them, which leaves
/// the caller's own choice — the editor's options, or the built-in default —
/// untouched.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigIndent {
    /// `indent_width: <n>`, in columns.
    pub indent_width: Option<usize>,
    /// `indent_style: "tab"` → `Some(true)`, `"space"` → `Some(false)`.
    pub use_tabs: Option<bool>,
    /// Values that were present but unusable, phrased for a CLI to print.
    /// The corresponding field is left `None` so the layout falls back
    /// instead of guessing.
    pub warnings: Vec<String>,
}

impl ConfigIndent {
    /// Parse the indentation keys out of a `saule.config`'s text. Every other
    /// key is ignored — this is deliberately not a full config parser.
    pub fn parse(text: &str) -> ConfigIndent {
        let mut out = ConfigIndent::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with("--") {
                continue;
            }
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let value = unquote(value);
            match key.trim() {
                "indent_style" => match value.to_ascii_lowercase().as_str() {
                    "tab" | "tabs" => out.use_tabs = Some(true),
                    "space" | "spaces" => out.use_tabs = Some(false),
                    other => out.warnings.push(format!(
                        "ignoring `indent_style: {other}` — expected \"tab\" or \"space\""
                    )),
                },
                "indent_width" => match value.parse::<usize>() {
                    Ok(n) if (1..=16).contains(&n) => out.indent_width = Some(n),
                    _ => out.warnings.push(format!(
                        "ignoring `indent_width: {value}` — expected a number from 1 to 16"
                    )),
                },
                _ => {}
            }
        }
        out
    }

    /// True when the config declared neither key.
    pub fn is_empty(&self) -> bool {
        self.indent_width.is_none() && self.use_tabs.is_none()
    }

    /// Layer these settings over `base`, leaving fields the config didn't
    /// mention alone.
    pub fn apply_to(&self, base: FmtOptions) -> FmtOptions {
        let use_tabs = self.use_tabs.unwrap_or(base.use_tabs);
        // A config that switches to tabs without naming a width would
        // otherwise inherit a width that was measured in spaces, so line
        // breaking would assume tabs are 2 columns wide. When the base is
        // already tab-based its width came from the editor's tab size and is
        // the better answer.
        let fallback_width = if use_tabs && !base.use_tabs {
            DEFAULT_TAB_WIDTH
        } else {
            base.indent_width
        };
        FmtOptions {
            indent_width: self.indent_width.unwrap_or(fallback_width),
            use_tabs,
            ..base
        }
    }
}

/// Walk up from `start` (a file or a directory) to the nearest ancestor
/// holding a `saule.config`, and read its indentation settings.
///
/// Returns the config's path alongside the settings so a caller can name the
/// file in a warning. `None` when no ancestor has one, or it can't be read —
/// formatting a loose `.sau` file outside any project is not an error.
pub fn load_project_indent(start: &Path) -> Option<(PathBuf, ConfigIndent)> {
    let path = find_project_config(start)?;
    let text = fs::read_to_string(&path).ok()?;
    Some((path, ConfigIndent::parse(&text)))
}

/// The nearest `saule.config` at or above `start`. A `start` that is itself a
/// file is searched from its containing directory.
pub fn find_project_config(start: &Path) -> Option<PathBuf> {
    let from = if start.is_dir() {
        start
    } else {
        start.parent()?
    };
    from.ancestors()
        .map(|dir| dir.join("saule.config"))
        .find(|candidate| candidate.is_file())
}

fn unquote(s: &str) -> String {
    s.trim().trim_matches('"').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_keys_leave_the_base_untouched() {
        let cfg = ConfigIndent::parse("name: \"demo\"\nsrc_dirs: [\"src\"]\n");
        assert!(cfg.is_empty());
        assert!(cfg.warnings.is_empty());
        assert_eq!(cfg.apply_to(FmtOptions::default()), FmtOptions::default());
    }

    #[test]
    fn reads_both_keys() {
        let cfg = ConfigIndent::parse("indent_style: \"tab\"\nindent_width: 8\n");
        assert_eq!(cfg.use_tabs, Some(true));
        assert_eq!(cfg.indent_width, Some(8));
        let opts = cfg.apply_to(FmtOptions::default());
        assert!(opts.use_tabs);
        assert_eq!(opts.indent_width, 8);
        // Unrelated fields survive.
        assert_eq!(opts.max_width, FmtOptions::default().max_width);
    }

    #[test]
    fn unquoted_and_uppercase_values_parse() {
        let cfg = ConfigIndent::parse("indent_style: TABS\nindent_width: 3");
        assert_eq!(cfg.use_tabs, Some(true));
        assert_eq!(cfg.indent_width, Some(3));
        assert!(cfg.warnings.is_empty());
    }

    #[test]
    fn tabs_without_a_width_assume_a_four_column_stop() {
        let cfg = ConfigIndent::parse("indent_style: \"tab\"\n");
        assert_eq!(cfg.apply_to(FmtOptions::default()).indent_width, 4);
    }

    #[test]
    fn tabs_without_a_width_keep_an_editor_supplied_tab_size() {
        let editor = FmtOptions {
            indent_width: 8,
            use_tabs: true,
            ..FmtOptions::default()
        };
        let cfg = ConfigIndent::parse("indent_style: \"tab\"\n");
        assert_eq!(cfg.apply_to(editor).indent_width, 8);
    }

    #[test]
    fn a_width_alone_keeps_spaces() {
        let cfg = ConfigIndent::parse("indent_width: 4\n");
        let opts = cfg.apply_to(FmtOptions::default());
        assert_eq!(opts.indent_width, 4);
        assert!(!opts.use_tabs);
    }

    #[test]
    fn config_overrides_the_editors_options() {
        let editor = FmtOptions {
            indent_width: 4,
            use_tabs: false,
            ..FmtOptions::default()
        };
        let cfg = ConfigIndent::parse("indent_style: \"tab\"\nindent_width: 2\n");
        let opts = cfg.apply_to(editor);
        assert!(opts.use_tabs);
        assert_eq!(opts.indent_width, 2);
    }

    #[test]
    fn bad_values_warn_and_are_ignored() {
        let cfg = ConfigIndent::parse("indent_style: \"banana\"\nindent_width: 0\n");
        assert!(cfg.is_empty());
        assert_eq!(cfg.warnings.len(), 2);
        assert!(cfg.warnings[0].contains("indent_style"));
        assert!(cfg.warnings[1].contains("indent_width"));
    }

    #[test]
    fn comments_are_skipped() {
        let cfg = ConfigIndent::parse("-- indent_style: \"tab\"\nindent_width: 4\n");
        assert_eq!(cfg.use_tabs, None);
        assert_eq!(cfg.indent_width, Some(4));
    }
}
