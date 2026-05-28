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
    assert_eq!(out, twice, "lossless re-format diverged:\n{out}\n---\n{twice}");
}
