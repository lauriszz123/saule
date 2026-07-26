//! Tests moved out of src/lib.rs.
use saule_lexer::{Lexer, LexerError, Token};

fn lex(src: &str) -> Vec<Token> {
    Lexer::new(src)
        .tokenize()
        .unwrap()
        .into_iter()
        .map(|t| t.value)
        .collect()
}

#[test]
fn keywords_and_identifier() {
    assert_eq!(
        lex("local foo"),
        vec![Token::Local, Token::Identifier("foo".into()), Token::Eof]
    );
}

#[test]
fn word_operators() {
    assert_eq!(
        lex("a and b or not c"),
        vec![
            Token::Identifier("a".into()),
            Token::And,
            Token::Identifier("b".into()),
            Token::Or,
            Token::Not,
            Token::Identifier("c".into()),
            Token::Eof,
        ]
    );
}

#[test]
fn integers_and_floats() {
    assert_eq!(
        lex("1 2.5 100"),
        vec![
            Token::Int(1),
            Token::Float(2.5),
            Token::Int(100),
            Token::Eof,
        ]
    );
}

#[test]
fn dot_after_int_is_member_access_when_not_digit() {
    assert_eq!(
        lex("1.foo"),
        vec![
            Token::Int(1),
            Token::Dot,
            Token::Identifier("foo".into()),
            Token::Eof,
        ]
    );
}

#[test]
fn double_dot_is_concat_not_float() {
    assert_eq!(
        lex("1..2"),
        vec![Token::Int(1), Token::DotDot, Token::Int(2), Token::Eof]
    );
}

#[test]
fn leading_dot_is_a_float() {
    assert_eq!(
        lex(".5 .0 .25"),
        vec![
            Token::Float(0.5),
            Token::Float(0.0),
            Token::Float(0.25),
            Token::Eof,
        ]
    );
}

#[test]
fn leading_dot_float_after_an_operator() {
    // The spot a leading-dot literal actually shows up in real code.
    assert_eq!(
        lex("x = .5"),
        vec![
            Token::Identifier("x".into()),
            Token::Assign,
            Token::Float(0.5),
            Token::Eof,
        ]
    );
}

#[test]
fn concat_with_a_leading_dot_float_still_splits() {
    // `..` must win over `.5`: the char after the first dot is another dot.
    assert_eq!(
        lex("a...5"),
        vec![
            Token::Identifier("a".into()),
            Token::Ellipsis,
            Token::Int(5),
            Token::Eof,
        ]
    );
    assert_eq!(
        lex("a...5e"),
        vec![
            Token::Identifier("a".into()),
            Token::Ellipsis,
            Token::Int(5),
            Token::Identifier("e".into()),
            Token::Eof,
        ]
    );
}

#[test]
fn bare_dot_is_still_member_access() {
    assert_eq!(
        lex("a.b"),
        vec![
            Token::Identifier("a".into()),
            Token::Dot,
            Token::Identifier("b".into()),
            Token::Eof,
        ]
    );
}

#[test]
fn f_suffix_makes_an_integer_literal_a_float() {
    assert_eq!(
        lex("1f 0f 42F"),
        vec![
            Token::Float(1.0),
            Token::Float(0.0),
            Token::Float(42.0),
            Token::Eof,
        ]
    );
}

#[test]
fn f_suffix_on_an_already_fractional_literal() {
    assert_eq!(
        lex("2.5f .5f"),
        vec![Token::Float(2.5), Token::Float(0.5), Token::Eof]
    );
}

#[test]
fn f_is_only_a_suffix_when_no_identifier_follows() {
    // `1foo` must stay `1` then `foo`, not `1f` then `oo`.
    assert_eq!(
        lex("1foo"),
        vec![Token::Int(1), Token::Identifier("foo".into()), Token::Eof]
    );
    assert_eq!(
        lex("1fs"),
        vec![Token::Int(1), Token::Identifier("fs".into()), Token::Eof]
    );
    assert_eq!(
        lex("1f_"),
        vec![Token::Int(1), Token::Identifier("f_".into()), Token::Eof]
    );
}

#[test]
fn f_suffix_terminated_by_punctuation_or_eof() {
    assert_eq!(
        lex("f(1f, 2f)"),
        vec![
            Token::Identifier("f".into()),
            Token::LParen,
            Token::Float(1.0),
            Token::Comma,
            Token::Float(2.0),
            Token::RParen,
            Token::Eof,
        ]
    );
}

#[test]
fn suffixed_literal_span_covers_the_suffix() {
    // The span must include `f`, or diagnostics and the formatter would
    // underline the wrong slice of source.
    let toks = Lexer::new("1f").tokenize().unwrap();
    assert_eq!(toks[0].value, Token::Float(1.0));
    assert_eq!(toks[0].span, 0..2);
}

#[test]
fn leading_dot_float_span_starts_at_the_dot() {
    let toks = Lexer::new(".5").tokenize().unwrap();
    assert_eq!(toks[0].value, Token::Float(0.5));
    assert_eq!(toks[0].span, 0..2);
}

#[test]
fn ellipsis() {
    assert_eq!(
        lex("fn f(...x: int)"),
        vec![
            Token::Fn,
            Token::Identifier("f".into()),
            Token::LParen,
            Token::Ellipsis,
            Token::Identifier("x".into()),
            Token::Colon,
            Token::Identifier("int".into()),
            Token::RParen,
            Token::Eof,
        ]
    );
}

#[test]
fn line_comment_is_skipped() {
    assert_eq!(
        lex("fn -- comment here\nfoo"),
        vec![Token::Fn, Token::Identifier("foo".into()), Token::Eof]
    );
}

#[test]
fn block_comment_is_skipped() {
    assert_eq!(
        lex("fn --[[ multi\nline ]] foo"),
        vec![Token::Fn, Token::Identifier("foo".into()), Token::Eof]
    );
}

#[test]
fn minus_still_works_after_comment_check() {
    assert_eq!(
        lex("1 - 2"),
        vec![Token::Int(1), Token::Minus, Token::Int(2), Token::Eof]
    );
}

#[test]
fn arrow_and_fat_arrow() {
    assert_eq!(
        lex("-> =>"),
        vec![Token::Arrow, Token::FatArrow, Token::Eof]
    );
}

#[test]
fn null_safety_operators() {
    assert_eq!(
        lex("a?.b ?? c"),
        vec![
            Token::Identifier("a".into()),
            Token::QuestionDot,
            Token::Identifier("b".into()),
            Token::QuestionQuestion,
            Token::Identifier("c".into()),
            Token::Eof,
        ]
    );
}

#[test]
fn string_with_escapes() {
    assert_eq!(
        lex(r#""hi\n\t\"there\"""#),
        vec![Token::String("hi\n\t\"there\"".into()), Token::Eof]
    );
}

#[test]
fn unterminated_string_errors() {
    assert!(matches!(
        Lexer::new("\"oops").tokenize(),
        Err(LexerError::UnterminatedString(_))
    ));
}

#[test]
fn unterminated_block_comment_errors() {
    assert!(matches!(
        Lexer::new("--[[ never closed").tokenize(),
        Err(LexerError::UnterminatedBlockComment(_))
    ));
}
