//! Formatter integration — wraps `saule-fmt` so the LSP can reply to
//! `textDocument/formatting` and `textDocument/rangeFormatting`.

use saule_fmt::{Comment, CommentKind, FmtOptions};
use saule_lexer::Token;
use tower_lsp::lsp_types::{FormattingOptions, Position, TextEdit, Url};

use crate::line_index::LineIndex;

use super::Backend;

/// Map the editor's LSP formatting options onto the printer's configuration.
///
/// This is what makes an IDE's Code Style page take effect: IntelliJ and
/// VS Code both send the language's indent settings as `tabSize` /
/// `insertSpaces` on every formatting request. A `tabSize` of 0 would produce
/// unindented output, so it falls back to the Saule default.
fn fmt_options(options: &FormattingOptions) -> FmtOptions {
    let defaults = FmtOptions::default();
    FmtOptions {
        indent_width: if options.tab_size == 0 {
            defaults.indent_width
        } else {
            options.tab_size as usize
        },
        use_tabs: !options.insert_spaces,
        ..defaults
    }
}

impl Backend {

    /// Format the cached source for `uri` and return a single TextEdit
    /// replacing the whole document. Returns `None` if the document is
    /// missing or fails to lex/parse — the client should leave the
    /// buffer untouched on error.
    pub(super) async fn format_document(
        &self,
        uri: &Url,
        options: &FormattingOptions,
    ) -> Option<Vec<TextEdit>> {
        let entry = self.docs.get(uri.as_str())?;
        let source = entry.source.clone();
        drop(entry);

        let formatted = format_source(&source, fmt_options(options))?;
        // Skip the edit entirely when the file is already canonical;
        // avoids spurious undo entries on no-op formats.
        if formatted == source {
            return Some(Vec::new());
        }

        let line_index = LineIndex::new(&source);
        let end = line_index.position(&source, source.len());
        Some(vec![TextEdit {
            range: tower_lsp::lsp_types::Range {
                start: Position::new(0, 0),
                end,
            },
            new_text: formatted,
        }])
    }
}

/// Lex (with trivia), parse, and pretty-print `source`. Returns `None`
/// when lex / parse fails — we never want to hand the editor a partial
/// or comment-stripped buffer if the file currently doesn't compile.
fn format_source(source: &str, options: FmtOptions) -> Option<String> {
    let raw = saule_lexer::Lexer::new(source)
        .tokenize_with_trivia()
        .ok()?;

    // Split comments off from the parser token stream — saule-fmt
    // expects the AST and a separate, ordered comment slice.
    let mut comments: Vec<Comment> = Vec::new();
    let mut tokens = Vec::with_capacity(raw.len());
    for tok in raw {
        match tok.value {
            Token::LineComment(text) => comments.push(Comment {
                span: tok.span,
                kind: CommentKind::Line,
                text,
            }),
            Token::BlockComment(text) => comments.push(Comment {
                span: tok.span,
                kind: CommentKind::Block,
                text,
            }),
            other => tokens.push(saule_ast::Spanned {
                value: other,
                span: tok.span,
            }),
        }
    }

    let module = saule_parser::parse(tokens).ok()?;
    Some(saule_fmt::format_module_with_options(
        &module, source, &comments, options,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(tab_size: u32, insert_spaces: bool) -> FormattingOptions {
        FormattingOptions {
            tab_size,
            insert_spaces,
            ..FormattingOptions::default()
        }
    }

    const SRC: &str = "class Main\n  static fn main()\n    println(\"hi\")\n  end\nend\n";

    #[test]
    fn editor_tab_size_drives_the_indent() {
        let four = format_source(SRC, fmt_options(&opts(4, true))).unwrap();
        assert!(
            four.contains("\n    static fn main()"),
            "expected 4-space indent, got:\n{four}"
        );
        let two = format_source(SRC, fmt_options(&opts(2, true))).unwrap();
        assert!(
            two.contains("\n  static fn main()"),
            "expected 2-space indent, got:\n{two}"
        );
    }

    #[test]
    fn insert_spaces_false_emits_tabs() {
        let out = format_source(SRC, fmt_options(&opts(4, false))).unwrap();
        assert!(out.contains("\n\tstatic fn main()"), "expected tabs, got:\n{out}");
    }

    #[test]
    fn zero_tab_size_falls_back_to_the_default() {
        // A client that omits tabSize must not produce unindented output.
        let out = format_source(SRC, fmt_options(&opts(0, true))).unwrap();
        assert!(out.contains("\n  static fn main()"), "got:\n{out}");
    }

    #[test]
    fn unparseable_source_is_left_alone() {
        assert!(format_source("class {{{", FmtOptions::default()).is_none());
    }
}
