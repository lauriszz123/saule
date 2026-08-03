//! Parsing a buffer that is mid-edit.
//!
//! The call being typed usually has no `)` yet, so the document doesn't
//! parse. Appending the missing closers never shifts the offsets before
//! the cursor, so the same walk works on the repaired tree.

use saule_ast::Module;

/// Try to make a mid-keystroke buffer parse by appending a closing
/// suffix: `)` for `foo(`, `nil)` for `foo(1, ` (an empty slot after a
/// comma isn't an expression), each with enough `end`s to close the
/// blocks the call sits in. First candidate that parses wins.
///
/// Only ever appends, so every byte offset in the original source —
/// including the cursor — keeps its meaning.
pub(crate) fn repair_parse(source: &str, offset: usize) -> Option<Module> {
    let offset = offset.min(source.len());
    if !source.is_char_boundary(offset) {
        return None;
    }
    // The delimiters are inserted *at the cursor*, not at the end of the
    // buffer: the call being typed is normally in the middle of a file
    // with well-formed code after it, and a `)` appended past that code
    // closes nothing. Only the text before the cursor decides what is
    // still open, and it keeps its offsets because nothing moves ahead
    // of it.
    let (head, tail) = source.split_at(offset);
    let closers = unclosed_delimiters(head);
    for filler in ["", "nil"] {
        for ends in 0..=4 {
            if filler.is_empty() && closers.is_empty() && ends == 0 {
                continue; // that's the original source, already known to fail
            }
            let mut patched = String::with_capacity(source.len() + closers.len() + 24);
            patched.push_str(head);
            patched.push_str(filler);
            patched.push_str(&closers);
            patched.push_str(tail);
            for _ in 0..ends {
                patched.push_str("\nend");
            }
            if let Ok(tokens) = saule_lexer::Lexer::new(&patched).tokenize()
                && let Ok(module) = saule_parser::parse(tokens)
            {
                return Some(module);
            }
        }
    }
    None
}

/// The closing delimiters `source` is missing, innermost first — so
/// `foo(bar(` yields `"))"`. Skips string literals and `--` line /
/// `--[[ ]]` block comments so brackets inside them don't count.
/// Returns an empty string when everything is balanced.
pub(crate) fn unclosed_delimiters(source: &str) -> String {
    let b = source.as_bytes();
    let mut stack: Vec<u8> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'"' | b'\'' => {
                let quote = b[i];
                i += 1;
                while i < b.len() {
                    if b[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if b[i] == quote {
                        break;
                    }
                    i += 1;
                }
            }
            b'-' if b.get(i + 1) == Some(&b'-') => {
                if b.get(i + 2) == Some(&b'[') && b.get(i + 3) == Some(&b'[') {
                    i += 4;
                    while i + 1 < b.len() && !(b[i] == b']' && b[i + 1] == b']') {
                        i += 1;
                    }
                    i += 1;
                } else {
                    while i < b.len() && b[i] != b'\n' {
                        i += 1;
                    }
                }
            }
            open @ (b'(' | b'[' | b'{') => stack.push(open),
            b')' | b']' | b'}' => {
                stack.pop();
            }
            _ => {}
        }
        i += 1;
    }
    stack
        .iter()
        .rev()
        .map(|open| match open {
            b'(' => ')',
            b'[' => ']',
            _ => '}',
        })
        .collect()
}
