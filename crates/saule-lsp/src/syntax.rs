//! Parsing a buffer that is not the document.
//!
//! The document's own tree comes from the database
//! ([`crate::server::Backend::parsed`]), which memoises it and remembers the
//! file's shape across edits. What is left here are the two jobs that parse
//! something *else*: text the user has not typed (completion splices a
//! sentinel identifier in at the cursor before parsing), and text that must
//! be rejected rather than repaired.

use saule_ast::Module;

/// Lex and parse `source`, recovering at both stages.
///
/// Total: there is no input this returns nothing for. The strict parser
/// answers "no tree" for any file with a syntax error, which describes a
/// source file for most of the time anyone is typing in it, and a language
/// server that goes blank exactly then is a language server people turn off.
pub(crate) fn tolerant(source: &str) -> Module {
    tolerant_with_prior(source, None)
}

/// [`tolerant`], told where the *document* this text was derived from had
/// its declarations at its last clean parse.
///
/// A file with no indentation offers no evidence of where a forgotten `end`
/// belonged, so every declaration below it is parsed one scope too deep and
/// drops out of the outline. Editing history says what whitespace cannot:
/// `after` was a top-level function a keystroke ago, so the edit that buried
/// it was a deleted `end`, not a restructuring.
pub(crate) fn tolerant_with_prior(
    source: &str,
    prior: Option<&saule_parser::PriorShape>,
) -> Module {
    let lexed = saule_lexer::Lexer::new(source).tokenize_recover();
    saule_parser::parse_recover_with_prior(lexed.tokens, source, prior).module
}

/// Lex and parse `source`, insisting it is valid.
///
/// For the callers that act on the tree rather than describe it — the
/// formatter, which would otherwise write a guess back to disk, and
/// completion's repair pass, which prefers a real tree over a recovered one
/// when a small edit can produce one.
pub(crate) fn strict(source: &str) -> Option<Module> {
    let tokens = saule_lexer::Lexer::new(source).tokenize().ok()?;
    saule_parser::parse(tokens).ok()
}
