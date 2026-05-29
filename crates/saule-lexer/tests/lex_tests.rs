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
