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
//! This is the check that comment wanted to be.
//!
//! It used to read the keyword table out of the lexer's own source, because
//! the table was a `match` arm rather than a value. It is now
//! [`saule_lexer::KEYWORDS`], expanded from the same macro as the lookup, so
//! this imports it instead — a scrape that silently matched nothing would let
//! every check below pass forever, and that is exactly what happened when the
//! table moved.
//!
//! What still cannot be checked from here: each of the three editor plugins
//! keeps its own copy of this list in its own repository. The failure message
//! names them.

use saule_lexer::{KEYWORDS, Lexer, Token};
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

/// The reserved words, straight from the lexer.
fn lexer_keywords() -> BTreeSet<String> {
    KEYWORDS.iter().map(|k| (*k).to_string()).collect()
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
        "grammar/saule.tmLanguage.json does not highlight: {missing:?}\n\
         Each editor plugin keeps its own copy, in its own repository:\n\
         \x20 saule-vscode   syntaxes/saule.tmLanguage.json\n\
         \x20 saule-intellij src/main/kotlin/com/saule/lang/lexer/SauleTokenTypes.kt\n\
         \x20 saule-nvim     syntax/saule.vim"
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
fn the_keyword_list_is_not_empty_or_malformed() {
    // An empty list would make both tests above pass forever.
    assert!(
        KEYWORDS.len() > 30,
        "only {} keywords in KEYWORDS — did the macro lose its body?",
        KEYWORDS.len()
    );
    for expected in ["class", "finally", "match", "when"] {
        assert!(KEYWORDS.contains(&expected), "`{expected}` is missing");
    }
    let unique: BTreeSet<&&str> = KEYWORDS.iter().collect();
    assert_eq!(unique.len(), KEYWORDS.len(), "KEYWORDS has a duplicate");
    for word in KEYWORDS {
        assert!(
            !word.is_empty() && word.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
            "`{word}` is not a plausible keyword"
        );
    }
}

/// Each reserved word lexes as one, and both token helpers agree it is one.
///
/// The highlighters are not the only consumers of "is this a keyword".
/// [`Token::is_keyword`] decides whether a name collision is reported as
/// "`x` is a reserved word — choose another name" rather than a bare parse
/// error, and whether the word is accepted as a member name after a dot. A
/// keyword added to the table but not to that list is reserved by the parser
/// and invisible to everything that asks a token what it is — which is the
/// state `finally` was in, and what this test exists to catch.
#[test]
fn the_token_helpers_know_every_keyword() {
    for word in KEYWORDS {
        let lexed = Lexer::new(word).tokenize().expect("a keyword lexes");
        // `tokenize` appends `Eof`.
        assert_eq!(lexed.len(), 2, "`{word}` did not lex to a single token");
        let tok = &lexed[0].value;
        assert!(
            !matches!(tok, Token::Identifier(_)),
            "`{word}` lexed as an identifier"
        );
        assert!(
            tok.is_keyword(),
            "`{word}` is reserved but `Token::is_keyword` says no — add it there"
        );
        assert_eq!(
            tok.lexeme(),
            Some(*word),
            "`{word}` is reserved but `Token::lexeme` spells it differently"
        );
    }
}
