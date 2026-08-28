//! What the parser *says* when it fails.
//!
//! The sibling `recovery_tests.rs` pins what survives in the tree; this file
//! pins the other half of the same job. A diagnostic is the only part of the
//! parser most people ever read, so the message and the help are behaviour:
//! "it reports an error" is not the assertion here, "it reports *this* error,
//! and offers *this* fix" is.
//!
//! Every case is checked twice, because the parser has two modes and they
//! must agree about what is wrong: `parse` (strict, used by the CLI and the
//! formatter) has to reject it, and `parse_recover` (used by the language
//! server on every keystroke) has to report it and keep a usable tree.

use miette::Diagnostic;
use saule_ast::{BinOp, Expr, Spanned, Stmt};
use saule_lexer::Lexer;
use saule_parser::{ParseError, Parsed, parse, parse_recover};

fn recover(src: &str) -> Parsed {
    let tokens = Lexer::new(src).tokenize().expect("lex ok");
    parse_recover(tokens, src)
}

/// The one error a strict parse stops on. Also the assertion that it *does*
/// stop: recovery must never be the only thing standing between bad input and
/// a clean parse.
fn strict_error(src: &str) -> ParseError {
    let tokens = Lexer::new(src).tokenize().expect("lex ok");
    match parse(tokens) {
        Ok(_) => panic!("strict parse should have rejected:\n{src}"),
        Err(e) => e,
    }
}

/// Message and help of the first error a recovering parse reports.
fn reported(src: &str) -> (String, Option<String>) {
    let parsed = recover(src);
    let err = parsed
        .errors
        .first()
        .unwrap_or_else(|| panic!("no diagnostic for:\n{src}"));
    (err.to_string(), err.help().map(|h| h.to_string()))
}

fn assert_contains(haystack: &str, needle: &str) {
    assert!(
        haystack.contains(needle),
        "expected {needle:?} in {haystack:?}"
    );
}

// ── found: which token, and what kind of thing it is ────────────────────────

#[test]
fn every_message_names_what_it_found() {
    let (msg, _) = reported("local x = ");
    assert_eq!(msg, "expected an expression after `=`, found end of input");
}

#[test]
fn a_reserved_word_is_named_as_one() {
    // Without "keyword", the message reads as though `end` were an ordinary
    // name the parser inexplicably disliked.
    let (msg, help) = reported("local end = 1");
    assert_eq!(
        msg,
        "expected variable name after `local`, found keyword `end`"
    );
    assert_contains(&help.expect("a name collision has a fix"), "reserved word");
}

// ── expected: which token wanted an operand ─────────────────────────────────

#[test]
fn a_missing_operand_names_the_operator_that_wanted_it() {
    let (msg, _) = reported("local x = 1 +\nlocal y = 2");
    assert_eq!(
        msg,
        "expected an expression after `+`, found keyword `local`"
    );
}

#[test]
fn a_condition_that_was_never_written_names_its_keyword() {
    let (msg, _) = reported("if then\n  println(1)\nend");
    assert_eq!(
        msg,
        "expected an expression after `if`, found keyword `then`"
    );
}

#[test]
fn a_token_that_merely_precedes_the_hole_is_not_blamed() {
    // The `)` in front of the stray one closes a call; it demands nothing
    // after it. Naming it would send the reader to a token that is not the
    // problem, so the message stays general.
    let (msg, _) = reported("println(1)\n)");
    assert_contains(&msg, "expected an expression,");
}

#[test]
fn a_trailing_comma_in_an_argument_list_says_to_remove_it() {
    let (msg, help) = reported("println(1, )");
    assert_eq!(msg, "expected an expression after `,`, found `)`");
    assert_contains(&help.expect("a trailing comma has a fix"), "remove");
}

// ── `<>`: the three ways to write nothing between angle brackets ────────────

#[test]
fn an_empty_type_argument_list_is_reported_as_one() {
    let src = "local evens = filter<>(nums, f)";
    let (msg, help) = reported(src);
    assert_eq!(msg, "empty type argument list");
    assert_contains(&help.expect("an empty list has a fix"), "drop `<>`");
    assert!(matches!(
        strict_error(src),
        ParseError::EmptyTypeArgs { .. }
    ));
}

#[test]
fn an_empty_type_argument_list_points_at_the_brackets() {
    // Not at the `>`, which is where the ordinary rules would have stopped:
    // the mistake is the pair, and the label has to cover both.
    let src = "local evens = filter<>(nums, f)";
    let span = strict_error(src).span().clone();
    assert_eq!(&src[span], "<>");
}

#[test]
fn an_empty_type_argument_list_still_parses_as_a_call() {
    // The whole point of reporting it here rather than letting the `<` fall
    // through to the comparison rung: one diagnostic, and a tree the editor
    // can still answer questions about.
    let parsed = recover("local evens = filter<>(nums, f)");
    assert_eq!(parsed.errors.len(), 1, "{:?}", parsed.errors);
    let Stmt::Local {
        value: Some(Spanned {
            value: Expr::Call { args, .. },
            ..
        }),
        ..
    } = &parsed.module.stmts[0].value
    else {
        panic!("expected the call to survive: {:?}", parsed.module.stmts[0]);
    };
    assert_eq!(args.len(), 2);
}

#[test]
fn an_empty_type_parameter_list_is_reported_on_the_declaration() {
    let src = "fn map<>(x: integer) -> integer\n  return x\nend";
    let (msg, help) = reported(src);
    assert_eq!(msg, "empty type parameter list");
    assert_contains(&help.expect("an empty list has a fix"), "drop `<>`");
    assert!(matches!(
        strict_error(src),
        ParseError::EmptyTypeParams { .. }
    ));
}

#[test]
fn an_empty_type_parameter_list_keeps_the_function() {
    let parsed = recover("fn map<>(x: integer) -> integer\n  return x\nend");
    assert_eq!(parsed.errors.len(), 1, "{:?}", parsed.errors);
    assert!(
        matches!(&parsed.module.stmts[0].value, Stmt::Decl(d)
            if matches!(&d.value, saule_ast::Decl::Function { name, params, .. }
                if name == "map" && params.len() == 1)),
        "expected the signature to survive: {:?}",
        parsed.module.stmts[0]
    );
}

#[test]
fn a_declaration_reports_parameters_and_a_use_reports_arguments() {
    // `class Box<>` declares; `extends A<>` uses. Both discard the names
    // (generics on classes are still accept-and-ignore), and they are still
    // two different mistakes.
    assert_eq!(
        reported("class Box<>\n  local v: integer = 0\nend").0,
        "empty type parameter list"
    );
    assert_eq!(
        reported("class B extends A<>\n  local v: integer = 0\nend").0,
        "empty type argument list"
    );
}

#[test]
fn lt_gt_is_reported_as_the_not_equal_it_was_meant_to_be() {
    let src = "local d = a <> b";
    let (msg, help) = reported(src);
    assert_eq!(msg, "`<>` is not an operator");
    assert_contains(&help.expect("a wrong spelling has a fix"), "`!=`");
    assert!(matches!(strict_error(src), ParseError::LtGtNotEqual { .. }));
}

#[test]
fn a_spaced_lt_gt_before_a_call_is_read_as_not_equal() {
    // `f<>(x)` and `a <> (x)` are the same three tokens. The space is the
    // only evidence there is, and it is good evidence: type arguments are
    // never written away from their callee.
    assert_eq!(
        reported("local d = a <> (b + 1)").0,
        "`<>` is not an operator"
    );
    assert_eq!(reported("local d = a<>(b)").0, "empty type argument list");
}

#[test]
fn lt_gt_recovers_as_a_comparison() {
    let parsed = recover("local d = a <> b");
    assert_eq!(parsed.errors.len(), 1, "{:?}", parsed.errors);
    let Stmt::Local {
        value: Some(Spanned {
            value: Expr::Binary { op, .. },
            ..
        }),
        ..
    } = &parsed.module.stmts[0].value
    else {
        panic!("expected a binary expression: {:?}", parsed.module.stmts[0]);
    };
    assert_eq!(*op, BinOp::NotEq);
}

#[test]
fn an_empty_type_argument_list_in_a_type_is_reported_too() {
    let src = "local t: table<> = {}";
    assert_eq!(reported(src).0, "empty type argument list");
    assert!(matches!(
        strict_error(src),
        ParseError::EmptyTypeArgs { .. }
    ));
}

// ── the shapes that look like `<>` and are not ──────────────────────────────

#[test]
fn real_comparisons_and_shifts_are_untouched() {
    // The `<>` rules key on two *adjacent* tokens, and every one of these has
    // something between the brackets — or is not a bracket pair at all.
    for src in [
        "local ok = a < b",
        "local ok = a < b > c",
        "local n = (a << 2) >> 1",
        "local xs = filter<integer>(nums, f)",
        "local t: table<table<integer>> = {}",
    ] {
        let tokens = Lexer::new(src).tokenize().expect("lex ok");
        assert!(parse(tokens).is_ok(), "should still parse: {src}");
    }
}
