//! Lexer error recovery: what the token stream still contains when the
//! characters are wrong.
//!
//! The question every test here asks is the same one: *the author is mid-edit,
//! does the rest of the file survive?* A lexer that gives up produces no
//! tokens at all, which means no tree, which means the editor knows nothing
//! about a file because of one unclosed quote.

use saule_lexer::{Lexer, LexerError, Token};

fn recover(src: &str) -> (Vec<Token>, Vec<LexerError>) {
    let lexed = Lexer::new(src).tokenize_recover();
    (
        lexed.tokens.into_iter().map(|t| t.value).collect(),
        lexed.errors,
    )
}

fn strict_fails(src: &str) {
    assert!(
        Lexer::new(src).tokenize().is_err(),
        "strict lex should still reject: {src:?}"
    );
}

/// The tokens after the last `Eof`-adjacent marker, as a readable string.
fn idents(tokens: &[Token]) -> Vec<&str> {
    tokens
        .iter()
        .filter_map(|t| match t {
            Token::Identifier(n) => Some(n.as_str()),
            _ => None,
        })
        .collect()
}

// ─── The invariant that makes two entry points safe ──────────────────────────

#[test]
fn strict_lex_is_unchanged_by_recovery() {
    for src in ["\"oops", "--[[ oops", "@", "0x", "\"\\q\"", "'nope"] {
        strict_fails(src);
    }
}

#[test]
fn clean_input_produces_no_errors() {
    let (tokens, errors) = recover("local x = \"hi\" -- note\nfn f() end\n");
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(idents(&tokens), ["x", "f"]);
}

// ─── Unterminated strings ────────────────────────────────────────────────────

#[test]
fn an_unterminated_string_stops_at_its_line() {
    // The case this recovery exists for. Running to EOF instead would put
    // every line below the quote inside the literal, deleting the rest of the
    // program from the token stream at the exact moment it's being typed.
    let src = "local greeting = \"hello\nfn after()\n  local b = 2\nend\n";
    let (tokens, errors) = recover(src);
    assert!(matches!(
        errors.as_slice(),
        [LexerError::UnterminatedString(_)]
    ));
    assert!(
        tokens.contains(&Token::String("hello".into())),
        "the partial literal is kept: {tokens:?}"
    );
    assert_eq!(
        idents(&tokens),
        ["greeting", "after", "b"],
        "everything below the bad line still lexes"
    );
    assert!(tokens.contains(&Token::Fn) && tokens.contains(&Token::End));
}

#[test]
fn a_valid_multi_line_string_is_untouched() {
    // A literal may legally span lines, so the line limit is applied only
    // once there is no closing quote anywhere.
    let (tokens, errors) = recover("local s = \"one\ntwo\"\nlocal t = 1\n");
    assert!(errors.is_empty(), "{errors:?}");
    assert!(
        tokens.contains(&Token::String("one\ntwo".into())),
        "{tokens:?}"
    );
    assert_eq!(idents(&tokens), ["s", "t"]);
}

#[test]
fn an_escaped_quote_does_not_close_the_literal() {
    let (tokens, errors) = recover("local s = \"a\\\"b\"\n");
    assert!(errors.is_empty(), "{errors:?}");
    assert!(tokens.contains(&Token::String("a\"b".into())), "{tokens:?}");
}

#[test]
fn an_unterminated_string_at_end_of_file_keeps_what_was_typed() {
    // What completion inside an import path sees mid-keystroke.
    let (tokens, errors) = recover("import Json from \"vend");
    assert!(matches!(
        errors.as_slice(),
        [LexerError::UnterminatedString(_)]
    ));
    assert!(tokens.contains(&Token::String("vend".into())), "{tokens:?}");
}

// ─── The other four repairs ──────────────────────────────────────────────────

#[test]
fn an_unexpected_character_is_dropped() {
    // No token stands for `@`, so leaving the tokens on either side adjacent
    // is exactly as if it had never been typed.
    let (tokens, errors) = recover("local a @ = 1\nlocal b = 2\n");
    assert!(matches!(errors.as_slice(), [LexerError::Unexpected(_)]));
    assert_eq!(idents(&tokens), ["a", "b"]);
    assert!(tokens.contains(&Token::Assign));
}

#[test]
fn a_bad_escape_keeps_its_characters() {
    let (tokens, errors) = recover("local s = \"a\\qb\"\nlocal t = 1\n");
    assert!(matches!(errors.as_slice(), [LexerError::BadEscape(_)]));
    assert!(
        tokens.contains(&Token::String("a\\qb".into())),
        "{tokens:?}"
    );
    assert_eq!(idents(&tokens), ["s", "t"]);
}

#[test]
fn a_bad_number_becomes_zero() {
    // A literal still occupies a value position; dropping the token would
    // leave a hole and turn one bad literal into a cascade of parse errors.
    let (tokens, errors) = recover("local n = 0x\nlocal m = 2\n");
    assert!(matches!(errors.as_slice(), [LexerError::BadNumber(_)]));
    assert!(tokens.contains(&Token::Int(0)), "{tokens:?}");
    assert_eq!(idents(&tokens), ["n", "m"]);
}

#[test]
fn an_unterminated_block_comment_runs_to_the_end() {
    // Nothing else is possible — a block comment is meant to span lines, so
    // there is no line to stop at.
    let (tokens, errors) = recover("local a = 1\n--[[ oops\nlocal b = 2\n");
    assert!(matches!(
        errors.as_slice(),
        [LexerError::UnterminatedBlockComment(_)]
    ));
    assert_eq!(idents(&tokens), ["a"]);
}

// ─── Several at once ─────────────────────────────────────────────────────────

#[test]
fn every_error_in_the_file_is_reported() {
    let (_, errors) = recover("local a = @\nlocal b = 0x\nlocal c = \"oops\n");
    assert_eq!(errors.len(), 3, "{errors:?}");
}

#[test]
fn errors_are_bounded() {
    let (_, errors) = recover(&"@".repeat(500));
    assert_eq!(errors.len(), saule_lexer::MAX_ERRORS);
}

#[test]
fn a_stream_of_junk_still_terminates_and_ends_in_eof() {
    for src in ["@@@@", "\"", "'", "--[[", "0x 0b \"", "\\", "€ ¬ ±"] {
        let (tokens, _) = recover(src);
        assert_eq!(
            tokens.last(),
            Some(&Token::Eof),
            "{src:?} did not end in Eof"
        );
    }
}

#[test]
fn a_trailing_backslash_does_not_escape_the_line_break() {
    // `"abc\` mid-typing: the newline is the limit, not an escapee. Consuming
    // it would step past the line the repair stops at and report a spurious
    // bad escape for the line break itself.
    let (tokens, errors) = recover("local s = \"abc\\\nlocal t = 1\n");
    assert!(
        matches!(errors.as_slice(), [LexerError::UnterminatedString(_)]),
        "{errors:?}"
    );
    assert_eq!(idents(&tokens), ["s", "t"]);
}
