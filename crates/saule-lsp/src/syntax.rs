//! Getting a tree out of a buffer that is, most of the time, halfway through
//! an edit.
//!
//! Every feature in this crate — hover, completion, signature help, inlay
//! hints, navigation, the outline — begins by turning the document's text
//! into a [`Module`]. The strict parser answers "no tree" for any file with a
//! syntax error, which describes a source file for most of the time anyone is
//! typing in it, and a language server that goes blank exactly then is a
//! language server people turn off.
//!
//! So the default here is [`tolerant`], which recovers at both stages and
//! **always** produces a tree: complete where the text was complete, holed
//! where it wasn't. [`strict`] stays available for the two jobs that must not
//! act on a guess — publishing type diagnostics, and rewriting the file.

use saule_ast::Module;
use saule_lexer::LexerError;
use saule_parser::ParseError;

/// Everything that was wrong with a buffer, in pipeline order.
///
/// Two lists rather than one interleaved by position: a lexical error changes
/// what the tokens *are*, so the parse errors below it are downstream of it,
/// and reporting them in that order is reporting them in the order they need
/// to be fixed.
pub(crate) struct SyntaxErrors {
    pub lex: Vec<LexerError>,
    pub parse: Vec<ParseError>,
}

impl SyntaxErrors {
    pub fn is_empty(&self) -> bool {
        self.lex.is_empty() && self.parse.is_empty()
    }
}

/// Lex and parse `source`, recovering at both stages — the default for
/// anything that answers a question about where the cursor is.
///
/// Total: there is no input this returns nothing for.
///
/// No prior shape, so a forgotten `end` in an *unindented* file is untangled
/// only as far as indentation allows. For an open document prefer
/// [`crate::server::Backend::syntax`], which remembers.
pub(crate) fn tolerant(source: &str) -> Module {
    analyze(source, None).0
}

/// [`tolerant`], keeping the diagnostics as well as the tree, and told where
/// this file's declarations lived at its last clean parse.
pub(crate) fn analyze(
    source: &str,
    prior: Option<&saule_parser::PriorShape>,
) -> (Module, SyntaxErrors) {
    let lexed = saule_lexer::Lexer::new(source).tokenize_recover();
    let parsed = saule_parser::parse_recover_with_prior(lexed.tokens, source, prior);
    (
        parsed.module,
        SyntaxErrors {
            lex: lexed.errors,
            parse: parsed.errors,
        },
    )
}

/// Lex and parse `source`, insisting it is valid.
///
/// For the callers that act on the tree rather than describe it — the
/// formatter, which would otherwise write a guess back to disk, and the
/// diagnostic pipeline, which reports the syntax errors themselves and has no
/// business also reporting the type errors implied by a hole.
pub(crate) fn strict(source: &str) -> Option<Module> {
    let tokens = saule_lexer::Lexer::new(source).tokenize().ok()?;
    saule_parser::parse(tokens).ok()
}
