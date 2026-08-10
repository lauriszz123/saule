//! Round-trip the workspace `tests/*.sau` corpus through the formatter
//! and verify (a) it stays parseable and (b) re-formatting is idempotent.
//!
//! This is the strongest cheap check we can run: every shape used in any
//! integration test must survive a `format → parse → format` cycle
//! unchanged.

use std::{fs, path::PathBuf};

use saule_fmt::{Comment, CommentKind};
use saule_lexer::Token;

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR for this crate is `<root>/crates/saule-fmt`.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crate is two levels below the workspace root")
        .to_path_buf()
}

fn format_str(src: &str) -> Result<String, String> {
    let tokens = saule_lexer::Lexer::new(src)
        .tokenize()
        .map_err(|e| format!("{e:?}"))?;
    let module = saule_parser::parse(tokens).map_err(|e| format!("{e:?}"))?;
    Ok(saule_fmt::format_module(&module))
}

/// Format using the lossless lexer path, threading comments through.
fn format_with_comments(src: &str) -> Result<String, String> {
    let raw = saule_lexer::Lexer::new(src)
        .tokenize_with_trivia()
        .map_err(|e| format!("{e:?}"))?;
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
    let module = saule_parser::parse(tokens).map_err(|e| format!("{e:?}"))?;
    Ok(saule_fmt::format_module_with_comments(
        &module, src, &comments,
    ))
}

#[test]
fn corpus_round_trips_and_is_idempotent() {
    let dir = workspace_root().join("tests");
    let entries = match fs::read_dir(&dir) {
        Ok(it) => it,
        Err(err) => panic!("could not read {}: {err}", dir.display()),
    };

    let mut failures: Vec<String> = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("sau") {
            continue;
        }
        // `pipe_then.sau` exercises an aspirational `then`-pipe syntax that
        // the current parser doesn't accept, so it can't round-trip.
        if path.file_name().and_then(|s| s.to_str()) == Some("pipe_then.sau") {
            continue;
        }
        let src = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(err) => {
                failures.push(format!("{}: read failed: {err}", path.display()));
                continue;
            }
        };
        let once = match format_str(&src) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!("{}: first format failed: {e}", path.display()));
                continue;
            }
        };
        let twice = match format_str(&once) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!(
                    "{}: second format failed (formatter output isn't parseable): {e}",
                    path.display()
                ));
                continue;
            }
        };
        if once != twice {
            failures.push(format!(
                "{}: formatter is not idempotent\n--- first pass ---\n{once}--- second pass ---\n{twice}",
                path.display()
            ));
            continue;
        }

        // Second axis: the comment-preserving path must also be
        // idempotent. We don't compare against the comment-stripped
        // output (it differs by design when the file has comments) — we
        // just require that two passes through the lossless formatter
        // converge.
        let lossless_once = match format_with_comments(&src) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!(
                    "{}: lossless first format failed: {e}",
                    path.display()
                ));
                continue;
            }
        };
        let lossless_twice = match format_with_comments(&lossless_once) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!(
                    "{}: lossless second format failed: {e}",
                    path.display()
                ));
                continue;
            }
        };
        if lossless_once != lossless_twice {
            failures.push(format!(
                "{}: lossless formatter is not idempotent\n--- first ---\n{lossless_once}--- second ---\n{lossless_twice}",
                path.display()
            ));
        }
    }

    if !failures.is_empty() {
        panic!(
            "{} formatter failure(s):\n\n{}",
            failures.len(),
            failures.join("\n\n")
        );
    }
}

#[test]
fn lossless_preserves_comment_text() {
    let src = "\
-- leading comment
fn main()
  -- inside
  local x = 1  -- trailing
  --[[ block
  spanning lines ]]
  return x
end
";
    let out = format_with_comments(src).expect("format");
    for needle in [
        "-- leading comment",
        "-- inside",
        "-- trailing",
        "--[[ block",
        "spanning lines ]]",
    ] {
        assert!(
            out.contains(needle),
            "expected comment fragment {needle:?} in output:\n{out}"
        );
    }
    // And idempotent.
    let twice = format_with_comments(&out).expect("re-format");
    assert_eq!(
        out, twice,
        "lossless re-format diverged:\n{out}\n---\n{twice}"
    );
}

// ── Layout regressions ──────────────────────────────────────────────────────
//
// Each of these pins a behaviour that was wrong (or absent) and is easy to
// regress, since the corpus test above only covers shapes that happen to
// appear in the integration tests.

/// A comment directly above a statement captions it and stays attached; a
/// comment the author separated with a blank line is a section header and
/// keeps its gap. The formatter used to swallow the gap unconditionally.
#[test]
fn blank_line_after_a_comment_follows_the_source() {
    let src = "\
class Main
  static fn main()
    -- section header

    local a: integer = 1
    -- attached caption
    local b: integer = 2
  end
end
";
    let out = format_with_comments(src).expect("format");
    assert!(
        out.contains("-- section header\n\n"),
        "section header lost its blank line:\n{out}"
    );
    assert!(
        out.contains("-- attached caption\n    local b"),
        "attached comment should not gain a blank line:\n{out}"
    );
}

/// A file-header comment is separated from the first declaration. This is the
/// `i == 0` path, which the blank-line logic used to skip entirely.
#[test]
fn file_header_comment_keeps_its_gap() {
    let src = "-- File header.\n\nclass Main\n  static fn main()\n    println(1)\n  end\nend\n";
    let out = format_with_comments(src).expect("format");
    assert!(
        out.starts_with("-- File header.\n\nclass Main"),
        "header comment was glued to the declaration:\n{out}"
    );
}

/// The same rule inside a class body. Members are walked by their own loop,
/// separate from `module` and `block`, so this is a distinct code path.
#[test]
fn blank_line_after_a_comment_applies_to_class_members_too() {
    let src = "\
class Hud
  -- Section header.

  static fn a()
    println(1)
  end

  -- Attached caption.
  static fn b()
    println(2)
  end
end
";
    let out = format_with_comments(src).expect("format");
    assert!(
        out.contains("-- Section header.\n\n"),
        "member section header lost its blank line:\n{out}"
    );
    assert!(
        out.contains("-- Attached caption.\n  static fn b()"),
        "attached member comment should not gain a blank line:\n{out}"
    );
}

/// Over-long argument lists break one-per-line instead of running off the
/// right edge.
#[test]
fn long_call_arguments_wrap() {
    let src = "class Main\n  static fn main()\n    println(111111111, 222222222, 333333333, 444444444, 555555555, 666666666, 777777777, 888888888, 999999999)\n  end\nend\n";
    assert!(
        src.lines().any(|l| l.len() > 100),
        "test input must actually exceed the width target"
    );
    let out = format_str(src).expect("format");
    assert!(out.contains("println(\n"), "call did not wrap:\n{out}");
    for line in out.lines() {
        assert!(line.len() <= 100, "line still over width: {line:?}");
    }
}

/// Over-long signatures break the same way.
#[test]
fn long_parameter_lists_wrap() {
    let src = "class Main\n  static fn wide(alpha: integer, bravo: integer, charlie: integer, delta: integer, echo: integer, foxtrot: integer) -> integer\n    return alpha\n  end\nend\n";
    let out = format_str(src).expect("format");
    assert!(out.contains("wide(\n"), "params did not wrap:\n{out}");
    assert!(
        out.contains(") -> integer"),
        "return type misplaced:\n{out}"
    );
}

/// Wrapped lists must not emit a trailing comma: unlike table literals, the
/// parser demands an argument/parameter after every comma, so a trailing one
/// makes the formatter's own output unparseable.
#[test]
fn wrapped_lists_reparse() {
    let cases = [
        "class Main\n  static fn main()\n    println(111111111, 222222222, 333333333, 444444444, 555555555, 666666666, 777777777, 888888888, 999999999)\n  end\nend\n",
        "class Main\n  static fn wide(alpha: integer, bravo: integer, charlie: integer, delta: integer, echo: integer, foxtrot: integer) -> integer\n    return alpha\n  end\nend\n",
    ];
    for src in cases {
        let once = format_str(src).expect("format");
        let twice = format_str(&once)
            .unwrap_or_else(|e| panic!("wrapped output did not re-parse: {e}\n{once}"));
        assert_eq!(once, twice, "wrapped layout is not idempotent:\n{once}");
    }
}

/// Short calls are left alone — wrapping only kicks in past the width target.
#[test]
fn short_calls_stay_inline() {
    let src = "class Main\n  static fn main()\n    println(1, 2, 3)\n  end\nend\n";
    let out = format_str(src).expect("format");
    assert!(
        out.contains("println(1, 2, 3)"),
        "short call was wrapped:\n{out}"
    );
}

/// Float literals keep the spelling the author chose. The AST only carries the
/// parsed `f64`, so the formatter used to rewrite `0f`, `.5` and `1.50` to the
/// canonical `0.0` / `0.5` / `1.5`.
#[test]
fn float_literals_keep_their_source_spelling() {
    let src = "\
class Main
  static fn main()
    local a = 0f
    local b = .5
    local c = 1.50
    local d = -.25
    local e = 2F
    println(match a case 0f then 1 case -1.0 then 2 case _ then 3 end)
  end
end
";
    let out = format_with_comments(src).expect("format");
    for needle in [
        "= 0f",
        "= .5",
        "= 1.50",
        "= -.25",
        "= 2F",
        "case 0f",
        "case -1.0",
    ] {
        assert!(
            out.contains(needle),
            "literal {needle:?} was rewritten:\n{out}"
        );
    }
    let twice = format_with_comments(&out).expect("re-format");
    assert_eq!(out, twice, "verbatim literals are not idempotent:\n{out}");
}

/// Without the original source there are no spans to quote, so literals fall
/// back to the canonical rendering — and that rendering must still be a float.
#[test]
fn float_literals_without_source_stay_floats() {
    let src = "class Main\n  static fn main()\n    local a = 0f\n    local b = .5\n  end\nend\n";
    let out = format_str(src).expect("format");
    assert!(out.contains("= 0.0"), "want canonical float:\n{out}");
    assert!(out.contains("= 0.5"), "want canonical float:\n{out}");
}

/// The indent width is configurable, and the width-based layout decisions
/// account for it rather than assuming two spaces.
#[test]
fn indent_width_is_configurable() {
    use saule_fmt::FmtOptions;

    let src = "class Main\n  static fn main()\n    println(1)\n  end\nend\n";
    let tokens = saule_lexer::Lexer::new(src).tokenize().expect("lex");
    let module = saule_parser::parse(tokens).expect("parse");

    let four = saule_fmt::format_module_with_options(
        &module,
        src,
        &[],
        FmtOptions {
            indent_width: 4,
            ..FmtOptions::default()
        },
    );
    assert!(
        four.contains("\n    static fn main()"),
        "want 4 spaces:\n{four}"
    );

    let tabs = saule_fmt::format_module_with_options(
        &module,
        src,
        &[],
        FmtOptions {
            use_tabs: true,
            ..FmtOptions::default()
        },
    );
    assert!(tabs.contains("\n\tstatic fn main()"), "want tabs:\n{tabs}");
}

/// A non-default indent must survive the same round trip as the canonical
/// one: a tab-indented project reformatted by `saule fmt --tabs` (or by a
/// `saule.config` that declares tabs) has to stay parseable and converge.
#[test]
fn corpus_round_trips_under_a_configured_indent() {
    use saule_fmt::{ConfigIndent, FmtOptions};

    let styles = [
        ("indent_style: \"tab\"\n", "\t"),
        ("indent_width: 4\n", "    "),
    ];
    let mut failures: Vec<String> = Vec::new();

    for (config, unit) in styles {
        let options = ConfigIndent::parse(config).apply_to(FmtOptions::default());
        for path in corpus_files() {
            let Ok(src) = fs::read_to_string(&path) else {
                continue;
            };
            let once = match format_with_options(&src, options) {
                Ok(s) => s,
                Err(e) => {
                    failures.push(format!(
                        "{} [{config:?}]: format failed: {e}",
                        path.display()
                    ));
                    continue;
                }
            };
            match format_with_options(&once, options) {
                Ok(twice) if twice == once => {}
                Ok(twice) => failures.push(format!(
                    "{} [{config:?}]: not idempotent\n--- first ---\n{once}--- second ---\n{twice}",
                    path.display()
                )),
                Err(e) => failures.push(format!(
                    "{} [{config:?}]: re-format failed (output isn't parseable): {e}",
                    path.display()
                )),
            }
            // Nothing may leak the canonical two spaces into a file that
            // asked for something else.
            if let Some(bad) = once
                .lines()
                .find(|l| l.starts_with(' ') || l.starts_with('\t'))
                .filter(|l| !l.starts_with(unit))
            {
                failures.push(format!(
                    "{} [{config:?}]: indented line does not use the configured unit: {bad:?}",
                    path.display()
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{} failure(s):\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// Every `.sau` file the corpus test above formats.
fn corpus_files() -> Vec<PathBuf> {
    let dir = workspace_root().join("tests");
    let Ok(entries) = fs::read_dir(&dir) else {
        panic!("could not read {}", dir.display())
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("sau"))
        // Same exclusion as `corpus_round_trips_and_is_idempotent`.
        .filter(|p| p.file_name().and_then(|s| s.to_str()) != Some("pipe_then.sau"))
        .collect()
}

/// Lossless format with an explicit [`saule_fmt::FmtOptions`].
fn format_with_options(src: &str, options: saule_fmt::FmtOptions) -> Result<String, String> {
    let raw = saule_lexer::Lexer::new(src)
        .tokenize_with_trivia()
        .map_err(|e| format!("{e:?}"))?;
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
    let module = saule_parser::parse(tokens).map_err(|e| format!("{e:?}"))?;
    Ok(saule_fmt::format_module_with_options(
        &module, src, &comments, options,
    ))
}

// ── String quoting ──────────────────────────────────────────────────────────
//
// `Token::String` carries only the decoded value, so the delimiter is gone by
// the time the AST exists. The printer reads it back out of the source at the
// literal's span — the same way it preserves float spelling — so a format is
// not allowed to change which quote the author wrote.

/// Format with the source available, which is what `saule fmt` and the LSP do.
fn format_with_source(src: &str) -> String {
    let tokens = saule_lexer::Lexer::new(src).tokenize().expect("lex ok");
    let module = saule_parser::parse(tokens).expect("parse ok");
    saule_fmt::format_module_with_comments(&module, src, &[])
}

#[test]
fn quote_style_is_preserved() {
    for src in [
        "local s = 'hello'",
        r#"local s = "hello""#,
        r#"local s = 'he said "hi"'"#,
        r#"local s = "it's fine""#,
        // Escaping the delimiter is preserved too, rather than being
        // "optimised" into the other quote style.
        r#"local s = "say \"hi\"""#,
        r#"local s = 'it\'s'"#,
    ] {
        assert_eq!(
            format_with_source(src).trim(),
            src,
            "formatting changed the quote style"
        );
    }
}

#[test]
fn quote_style_is_preserved_in_every_position() {
    for src in [
        // Pattern literal in a match arm.
        "local r = match v\n  case 'a' then 1\n  case _ then 2\nend",
        // Non-identifier table key.
        "local t = { 'a b': 1 }",
        // Import path.
        "import x from 'a/b'",
    ] {
        let out = format_with_source(src);
        assert!(
            out.contains('\''),
            "single quotes lost in: {src}\n got: {out}"
        );
    }
}

#[test]
fn quote_choice_is_idempotent() {
    for src in [
        "local s = 'hello'",
        r#"local s = "hello""#,
        r#"local s = 'he said "hi"'"#,
        "import x from 'a/b'",
    ] {
        let once = format_with_source(src);
        let twice = format_with_source(&once);
        assert_eq!(once, twice, "not idempotent: {src}");
    }
}

#[test]
fn without_source_a_delimiter_is_chosen_by_escape_count() {
    // `format_module` has no source to consult, so it picks whichever quote
    // needs fewer backslashes.
    assert_eq!(
        format_str(r#"local s = 'he said "hi"'"#).unwrap().trim(),
        r#"local s = 'he said "hi"'"#
    );
    assert_eq!(
        format_str("local s = 'hello'").unwrap().trim(),
        r#"local s = "hello""#
    );
}

#[test]
fn control_characters_survive_a_round_trip() {
    // This used to emit `\x00`, which the lexer rejects — formatting produced
    // a file that no longer lexed.
    let once = format_with_source(r#"local s = "a\0b""#);
    let twice = format_with_source(&once);
    assert_eq!(once, twice, "formatted output must still lex");
}

// ── Compound assignment ──────────────────────────────────────────────────

#[test]
fn compound_assignment_is_normalised_to_single_spaces() {
    for (src, expected) in [
        ("n+=2", "n += 2"),
        ("n   -=   3", "n -= 3"),
        ("n*=4", "n *= 4"),
        ("n/=5", "n /= 5"),
        ("n%=6", "n %= 6"),
        ("n^=7", "n ^= 7"),
        ("s..=\"b\"", "s ..= \"b\""),
        ("obj.count+=1", "obj.count += 1"),
        ("t[i]+=1", "t[i] += 1"),
    ] {
        assert_eq!(format_str(src).unwrap().trim(), expected, "source: {src}");
    }
}

#[test]
fn compound_assignment_rhs_is_not_parenthesised() {
    // The RHS runs to the end of the statement, so no operator inside it
    // can bind loosely enough to need brackets.
    assert_eq!(format_str("n *= 3 + 4").unwrap().trim(), "n *= 3 + 4");
    assert_eq!(format_str("s ..= a .. b").unwrap().trim(), "s ..= a .. b");
}

#[test]
fn compound_assignment_formatting_is_idempotent() {
    for src in ["n+=2", "s..=\"b\"", "t[i] %= 3", "obj.f ^= 2"] {
        let once = format_str(src).unwrap();
        let twice = format_str(&once).unwrap();
        assert_eq!(once, twice, "source: {src}");
    }
}

// ── Trailing blocks ─────────────────────────────────────────────────────────
//
// `f(a, fn() … end)` and `f(a) do … end` parse to the same tree, so the
// printer cannot tell them apart from the AST alone — it reads the lambda's
// span back out of the source, the same trick it uses for quotes and floats.
// Moving a lambda out of the parentheses is a rewrite, not a reformat: the
// trailing form binds to the callee's last free function-typed parameter,
// which the formatter has no signatures to reason about.

#[test]
fn a_lambda_written_inside_the_parentheses_stays_there() {
    let src = "MenuItem(\"Open\", fn()\n  showToast(\"Open\")\nend)";
    assert_eq!(format_with_source(src).trim(), src);
}

#[test]
fn a_lambda_written_as_a_trailing_block_stays_one() {
    let src = "MenuItem(\"Open\") do\n  showToast(\"Open\")\nend";
    assert_eq!(format_with_source(src).trim(), src);
}

#[test]
fn a_trailing_block_keeps_its_parameters_and_return_type() {
    let src = "apply() do (n: integer) -> integer\n  return n * 2\nend";
    assert_eq!(format_with_source(src).trim(), src);
}

#[test]
fn both_trailing_block_spellings_are_idempotent() {
    for src in [
        "MenuItem(\"Open\", fn()\n  showToast(\"Open\")\nend)",
        "MenuItem(\"Open\") do\n  showToast(\"Open\")\nend",
        "View(spacing: 10) do\n  print(1)\nend",
    ] {
        let once = format_with_source(src);
        let twice = format_with_source(&once);
        assert_eq!(once, twice, "not idempotent: {src}");
    }
}

// ── Nullable function types ─────────────────────────────────────────────────
//
// `?` binds to the return type inside a function type, so a nullable callback
// has to keep its parentheses. Printing the inner type bare rewrote
// `(fn() -> nil)?` — an optional callback — into `fn() -> nil?`, a callback
// that is always present and returns a nullable nothing. Both spellings parse
// and both are idempotent, so nothing here caught it until the type itself was
// compared.

#[test]
fn a_nullable_function_type_keeps_its_parentheses() {
    for src in [
        "local f: (fn() -> nil)? = nil",
        "local g: (fn(string) -> boolean)? = nil",
        "class C\n  onTap: (fn(float, float) -> nil)?\nend",
        "fn take(cb: (fn() -> nil)? = nil) -> nil\nend",
        "local h: table<(fn() -> nil)?> = {}",
    ] {
        assert_eq!(format_str(src).unwrap().trim(), src);
    }
}

/// The parentheses are not cosmetic: without them the formatted source
/// parses to a *different* type. Compare the trees, not the text.
#[test]
fn formatting_a_nullable_function_type_preserves_the_type() {
    let src = "local f: (fn(string) -> nil)? = nil";
    let before = saule_parser::parse(saule_lexer::Lexer::new(src).tokenize().unwrap()).unwrap();
    let formatted = format_str(src).unwrap();
    let after =
        saule_parser::parse(saule_lexer::Lexer::new(&formatted).tokenize().unwrap()).unwrap();
    assert_eq!(before, after, "formatted to a different type: {formatted}");
}

/// A `?` that really does belong to the return type must not gain parentheses
/// it never had — the fix is a parenthesise-when-nullable rule, not a
/// parenthesise-every-function one.
#[test]
fn a_function_returning_a_nullable_is_left_alone() {
    for src in [
        "local f: fn() -> integer? = nil",
        "local g: fn(string) -> table<integer>? = nil",
    ] {
        assert_eq!(format_str(src).unwrap().trim(), src);
    }
}
