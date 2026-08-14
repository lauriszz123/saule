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
fn hex_and_binary_literals() {
    assert_eq!(
        lex("0xFF 0xff 0X10 0b1010 0B11"),
        vec![
            Token::Int(255),
            Token::Int(255),
            Token::Int(16),
            Token::Int(10),
            Token::Int(3),
            Token::Eof,
        ]
    );
}

#[test]
fn underscores_group_digits_in_a_base_literal() {
    assert_eq!(
        lex("0xFF_FF 0b1010_1010"),
        vec![Token::Int(65535), Token::Int(170), Token::Eof]
    );
}

/// `0` on its own, and a decimal that merely starts with one, must not be
/// mistaken for a base prefix.
#[test]
fn a_bare_zero_is_still_a_decimal() {
    assert_eq!(
        lex("0 0.5 07"),
        vec![Token::Int(0), Token::Float(0.5), Token::Int(7), Token::Eof]
    );
}

#[test]
fn a_base_literal_needs_digits_and_valid_ones() {
    assert!(saule_lexer::Lexer::new("0x").tokenize().is_err());
    assert!(saule_lexer::Lexer::new("0xGG").tokenize().is_err());
    assert!(saule_lexer::Lexer::new("0b102").tokenize().is_err());
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

// ── Single-quoted strings ───────────────────────────────────────────────────
//
// `'…'` and `"…"` are interchangeable, as in Lua. Only the delimiter that
// opened a literal closes it, so each style carries the other unescaped.

#[test]
fn single_quoted_string() {
    assert_eq!(
        lex("'hello'"),
        vec![Token::String("hello".into()), Token::Eof]
    );
}

#[test]
fn quote_styles_produce_identical_tokens() {
    assert_eq!(lex("'hello'"), lex("\"hello\""));
}

#[test]
fn opposite_quote_needs_no_escape() {
    assert_eq!(
        lex(r#"'he said "hi"'"#),
        vec![Token::String("he said \"hi\"".into()), Token::Eof]
    );
    assert_eq!(
        lex(r#""it's fine""#),
        vec![Token::String("it's fine".into()), Token::Eof]
    );
}

#[test]
fn both_quote_escapes_work_in_either_style() {
    assert_eq!(
        lex(r#"'\'\"'"#),
        vec![Token::String("'\"".into()), Token::Eof]
    );
    assert_eq!(
        lex(r#""\'\"""#),
        vec![Token::String("'\"".into()), Token::Eof]
    );
}

#[test]
fn unterminated_single_quoted_string_errors() {
    assert!(matches!(
        Lexer::new("'oops").tokenize(),
        Err(LexerError::UnterminatedString(_))
    ));
}

#[test]
fn a_quote_does_not_close_the_other_style() {
    // The `"` here is ordinary text, so the literal runs to the second `'`.
    assert_eq!(
        lex("'a\"b'"),
        vec![Token::String("a\"b".into()), Token::Eof]
    );
}

// ── Compound assignment ──────────────────────────────────────────────────

#[test]
fn compound_assignment_operators() {
    assert_eq!(
        lex("a += b -= c *= d /= e %= f ^= g ..= h"),
        vec![
            Token::Identifier("a".into()),
            Token::PlusEq,
            Token::Identifier("b".into()),
            Token::MinusEq,
            Token::Identifier("c".into()),
            Token::StarEq,
            Token::Identifier("d".into()),
            Token::SlashEq,
            Token::Identifier("e".into()),
            Token::PercentEq,
            Token::Identifier("f".into()),
            Token::CaretEq,
            Token::Identifier("g".into()),
            Token::DotDotEq,
            Token::Identifier("h".into()),
            Token::Eof,
        ]
    );
}

#[test]
fn compound_assignment_does_not_shadow_existing_operators() {
    // `-=` must not eat the `->` return arrow, `..=` must not eat `...`,
    // and `==` / `<=` / `>=` / `!=` are untouched.
    assert_eq!(
        lex("-> ... == <= >= != .. ="),
        vec![
            Token::Arrow,
            Token::Ellipsis,
            Token::EqEq,
            Token::LtEq,
            Token::GtEq,
            Token::NotEq,
            Token::DotDot,
            Token::Assign,
            Token::Eof,
        ]
    );
}

#[test]
fn minus_eq_is_not_confused_with_a_comment() {
    // `--` opens a comment and is claimed before `symbol` runs, so `-=`
    // has to survive sitting right next to one.
    assert_eq!(
        lex("a -= 1 -- note\nb += 2"),
        vec![
            Token::Identifier("a".into()),
            Token::MinusEq,
            Token::Int(1),
            Token::Identifier("b".into()),
            Token::PlusEq,
            Token::Int(2),
            Token::Eof,
        ]
    );
}

#[test]
fn compound_assignment_spans_cover_the_whole_operator() {
    let toks = Lexer::new("a ..= b").tokenize().unwrap();
    assert_eq!(toks[1].value, Token::DotDotEq);
    assert_eq!(toks[1].span, 2..5);
}

// ── Bitwise operators ────────────────────────────────────────────────────

#[test]
fn bitwise_operators() {
    assert_eq!(
        lex("a & b | c ~ d << e >> f"),
        vec![
            Token::Identifier("a".into()),
            Token::Amp,
            Token::Identifier("b".into()),
            Token::Pipe,
            Token::Identifier("c".into()),
            Token::Tilde,
            Token::Identifier("d".into()),
            Token::Shl,
            Token::Identifier("e".into()),
            Token::Shr,
            Token::Identifier("f".into()),
            Token::Eof,
        ]
    );
}

#[test]
fn bitwise_compound_assignment() {
    assert_eq!(
        lex("a &= b |= c <<= d >>= e"),
        vec![
            Token::Identifier("a".into()),
            Token::AmpEq,
            Token::Identifier("b".into()),
            Token::PipeEq,
            Token::Identifier("c".into()),
            Token::ShlEq,
            Token::Identifier("d".into()),
            Token::ShrEq,
            Token::Identifier("e".into()),
            Token::Eof,
        ]
    );
}

#[test]
fn shifts_do_not_swallow_comparison_operators() {
    // `<=` and `>=` are checked before the doubled forms, so a comparison
    // never turns into a shift.
    assert_eq!(
        lex("a <= b >= c < d > e"),
        vec![
            Token::Identifier("a".into()),
            Token::LtEq,
            Token::Identifier("b".into()),
            Token::GtEq,
            Token::Identifier("c".into()),
            Token::Lt,
            Token::Identifier("d".into()),
            Token::Gt,
            Token::Identifier("e".into()),
            Token::Eof,
        ]
    );
}

#[test]
fn tilde_eq_is_not_one_token() {
    // Lua's inequality spelling gets no meaning here: `~` then `=`, which
    // fails in the parser rather than becoming xor-assignment. See
    // `Token::AmpEq`'s note.
    assert_eq!(
        lex("a ~= b"),
        vec![
            Token::Identifier("a".into()),
            Token::Tilde,
            Token::Assign,
            Token::Identifier("b".into()),
            Token::Eof,
        ]
    );
}

#[test]
fn nested_generics_lex_as_one_shift() {
    // The lexer cannot tell the two closers of `table<table<integer>>` from
    // a right shift, so it always produces `Shr`; splitting it back is the
    // parser's job, and only where a type argument list is open.
    assert_eq!(
        lex("table<table<integer>>"),
        vec![
            Token::Identifier("table".into()),
            Token::Lt,
            Token::Identifier("table".into()),
            Token::Lt,
            Token::Identifier("integer".into()),
            Token::Shr,
            Token::Eof,
        ]
    );
}

#[test]
fn bitwise_operator_spans_cover_the_whole_operator() {
    let toks = Lexer::new("a >>= b").tokenize().unwrap();
    assert_eq!(toks[1].value, Token::ShrEq);
    assert_eq!(toks[1].span, 2..5);

    let toks = Lexer::new("a << b").tokenize().unwrap();
    assert_eq!(toks[1].value, Token::Shl);
    assert_eq!(toks[1].span, 2..4);
}
