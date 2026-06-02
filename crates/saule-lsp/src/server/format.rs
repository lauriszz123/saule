//! Formatter integration — wraps `saule-fmt` so the LSP can reply to
//! `textDocument/formatting` and `textDocument/rangeFormatting`.

use saule_fmt::{Comment, CommentKind};
use saule_lexer::Token;
use tower_lsp::lsp_types::{Position, TextEdit, Url};

use crate::line_index::LineIndex;

use super::Backend;

impl Backend {

    /// Format the cached source for `uri` and return a single TextEdit
    /// replacing the whole document. Returns `None` if the document is
    /// missing or fails to lex/parse — the client should leave the
    /// buffer untouched on error.
    pub(super) async fn format_document(&self, uri: &Url) -> Option<Vec<TextEdit>> {
        let entry = self.docs.get(uri.as_str())?;
        let source = entry.source.clone();
        drop(entry);

        let formatted = format_source(&source)?;
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
fn format_source(source: &str) -> Option<String> {
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
    Some(saule_fmt::format_module_with_comments(
        &module, source, &comments,
    ))
}
