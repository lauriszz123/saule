//! The two highlighters must know every keyword the lexer knows.
//!
//! `grammar/saule.tmLanguage.json` is what the website and the VS Code
//! extension highlight with, and `www/src/lib/saule-mode.ts` is what the
//! playground's editor highlights with. Both are hand-maintained lists, and
//! a comment in each asks whoever adds a keyword to add it in all three
//! places. That discipline failed: `finally` shipped in the language and in
//! neither highlighter, so a `try`/`catch`/`finally` block rendered with the
//! `finally` as plain text in every published example.
//!
//! This is the check that comment wanted to be. It reads the keyword table
//! out of the lexer's own source rather than importing it, because the table
//! is a `match` arm rather than a value — which is also what keeps this test
//! honest: it fails if a keyword is added to the lexer and nowhere else.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    // `CARGO_MANIFEST_DIR` is `<root>/crates/saule-lexer`.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the crate lives two levels under the repository root")
        .to_path_buf()
}

/// Every `"word" => Token::…` arm in `ident_or_keyword`.
fn lexer_keywords() -> BTreeSet<String> {
    let src = include_str!("../src/lib.rs");
    src.lines()
        .filter_map(|l| {
            let l = l.trim();
            let rest = l.strip_prefix('"')?;
            let (word, tail) = rest.split_once('"')?;
            tail.trim_start().strip_prefix("=> Token::")?;
            word.chars()
                .all(|c| c.is_ascii_lowercase() || c == '_')
                .then(|| word.to_string())
        })
        .collect()
}

/// Every bare word inside a `\b(a|b|c)\b` alternation in the grammar.
fn grammar_keywords() -> BTreeSet<String> {
    let path = repo_root().join("grammar/saule.tmLanguage.json");
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()));
    let mut out = BTreeSet::new();
    // The file is JSON, so a `\b` in a pattern is written `\\b`.
    for chunk in src.split("\\\\b(").skip(1) {
        let Some((alt, _)) = chunk.split_once(")\\\\b") else {
            continue;
        };
        for word in alt.split('|') {
            if !word.is_empty() && word.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
                out.insert(word.to_string());
            }
        }
    }
    out
}

/// Every single-quoted bare word in the playground's CodeMirror mode.
fn playground_keywords() -> BTreeSet<String> {
    let path = repo_root().join("www/src/lib/saule-mode.ts");
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()));
    // Comments first: an apostrophe in prose ("the playground's editor")
    // would otherwise shift every quote pair after it by one.
    strip_comments(&src)
        .split('\'')
        .skip(1)
        .step_by(2)
        .filter(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
        .map(str::to_string)
        .collect()
}

/// Drop `//` lines and `/* … */` spans, so only code is scanned.
fn strip_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(i) = rest.find("/*") {
        out.push_str(&rest[..i]);
        rest = match rest[i + 2..].find("*/") {
            Some(j) => &rest[i + 2 + j + 2..],
            None => "",
        };
    }
    out.push_str(rest);
    out.lines()
        .map(|l| match l.find("//") {
            Some(i) => &l[..i],
            None => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_textmate_grammar_knows_every_keyword() {
    let missing: Vec<_> = lexer_keywords()
        .difference(&grammar_keywords())
        .cloned()
        .collect();
    assert!(
        missing.is_empty(),
        "grammar/saule.tmLanguage.json does not highlight: {missing:?}"
    );
}

#[test]
fn the_playground_editor_knows_every_keyword() {
    let missing: Vec<_> = lexer_keywords()
        .difference(&playground_keywords())
        .cloned()
        .collect();
    assert!(
        missing.is_empty(),
        "www/src/lib/saule-mode.ts does not highlight: {missing:?}"
    );
}

#[test]
fn the_keyword_table_was_actually_found() {
    // A parser that silently matched nothing would make both tests above
    // pass forever.
    let keywords = lexer_keywords();
    assert!(
        keywords.len() > 30,
        "only {} keywords parsed out of the lexer — the table moved",
        keywords.len()
    );
    for expected in ["class", "finally", "match", "when"] {
        assert!(keywords.contains(expected), "`{expected}` was not parsed");
    }
}
