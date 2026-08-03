//! Last-resort resolution for buffers even [`super::repair`] can't parse.
//!
//! Resolves the callee by name from raw text — no AST — so signature help
//! still appears while the document is badly malformed.

use saule_ast::{Param, Type};
use saule_semantic::{lookup_method, super_init_target, with_classes};
use tower_lsp::lsp_types::SignatureHelp;

use super::*;

/// Scan raw source for the innermost unmatched `(` strictly before
/// `offset`, count the top-level commas that separate arguments, and
/// resolve the identifier sitting just before that `(`.
///
/// This is the lifeline that keeps signature help visible during
/// mid-keystroke edits like `add(1, ` where the parser can't yet
/// produce a usable AST.
pub(crate) fn textual_fallback(source: &str, offset: usize) -> Option<SignatureHelp> {
    let bytes = source.as_bytes();
    if offset > bytes.len() {
        return None;
    }
    // Walk left from the cursor, balancing brackets and tracking the
    // top-level comma count for the *outermost* call we land on.
    let mut paren_depth: i32 = 0;
    let mut bracket_depth: i32 = 0;
    let mut brace_depth: i32 = 0;
    let mut commas: usize = 0;
    let mut active = 0usize;
    let mut paren_pos: Option<usize> = None;
    let mut i = offset;
    let mut in_string: Option<u8> = None;
    while i > 0 {
        i -= 1;
        let c = bytes[i];
        if let Some(q) = in_string {
            if c == q && (i == 0 || bytes[i - 1] != b'\\') {
                in_string = None;
            }
            continue;
        }
        match c {
            b'"' | b'\'' => in_string = Some(c),
            b')' => paren_depth += 1,
            b']' => bracket_depth += 1,
            b'}' => brace_depth += 1,
            b'(' => {
                if paren_depth == 0 && bracket_depth == 0 && brace_depth == 0 {
                    paren_pos = Some(i);
                    active = commas;
                    break;
                }
                paren_depth -= 1;
            }
            b'[' => bracket_depth -= 1,
            b'{' => brace_depth -= 1,
            b',' if paren_depth == 0 && bracket_depth == 0 && brace_depth == 0 => {
                commas += 1;
            }
            _ => {}
        }
    }
    let lparen = paren_pos?;

    // Identifier just before `(`. Skip whitespace, then collect
    // the longest trailing run of `[A-Za-z0-9_]`.
    let mut j = lparen;
    while j > 0 && bytes[j - 1].is_ascii_whitespace() {
        j -= 1;
    }
    let id_end = j;
    while j > 0 {
        let b = bytes[j - 1];
        if b.is_ascii_alphanumeric() || b == b'_' {
            j -= 1;
        } else {
            break;
        }
    }
    if j == id_end {
        return None;
    }
    let name = &source[j..id_end];

    // Detect dotted member access (`receiver.method`) so we can route
    // method calls through the same lookup the AST path uses.
    let receiver = if j > 0 && bytes[j - 1] == b'.' {
        let mut k = j - 1;
        while k > 0 {
            let b = bytes[k - 1];
            if b.is_ascii_alphanumeric() || b == b'_' {
                k -= 1;
            } else {
                break;
            }
        }
        if k < j - 1 {
            Some(&source[k..j - 1])
        } else {
            None
        }
    } else {
        None
    };

    if let Some(recv) = receiver {
        // Mid-keystroke `self.super(` — resolve the enclosing class from
        // the raw source, then chase its parent constructor.
        if recv == "self"
            && name == "super"
            && let Some(class) = enclosing_class_textual(source, offset)
            && let Some((owner, sig)) = super_init_target(&class)
        {
            return build_help_simple(&format!("{owner}.init"), sig, active);
        }
        // Qualified the same way the AST path qualifies it, so a call
        // that falls back mid-keystroke doesn't change its heading and
        // then change back once the buffer parses again.
        let qname = format!("{recv}.{name}");
        if with_classes(|r| r.contains_key(recv))
            && let Some(sig) = lookup_method(recv, name)
        {
            return build_help_simple(&qname, sig, active);
        }
        // Stdlib module call (`Os.exists`, `String.find`, ...) or
        // value-type instance method (`File.write`).
        if let Some(native) = saule_typeck::sigs::lookup(&qname) {
            return build_help_native_simple(&qname, &qname, &native, active);
        }
        return None;
    }

    // Bare identifier: try class constructor first, then fall back to
    // the active class (if the cursor is inside one) — mirroring the
    // AST path's resolution order.
    if with_classes(|r| r.contains_key(name)) {
        let sig = lookup_method(name, "init")?;
        return build_help_simple(name, sig, active);
    }
    if let Some(class) = enclosing_class_textual(source, offset)
        && let Some(sig) = lookup_method(&class, name)
    {
        return build_help_simple(name, sig, active);
    }
    // User-defined free function. Re-parse the source defensively so
    // we can pick up `fn name(...)` even when the parse failed during
    // mid-keystroke (the failing call site may be a different stmt).
    if let Some((params, ret)) = lookup_user_fn_textual(source, name) {
        return build_help_user_fn_simple(name, &params, &ret, active);
    }
    if let Some(native) = saule_typeck::sigs::lookup(name) {
        return build_help_native_simple(name, name, &native, active);
    }
    None
}

/// Build a `SignatureHelp` from a known sig + active-param index,
/// without args span data (we don't have it in the textual path).
pub(crate) fn build_help_simple(
    name: &str,
    sig: saule_semantic::MethodSig,
    active: usize,
) -> Option<SignatureHelp> {
    let arity = sig.params.len();
    let active = active.min(arity.saturating_sub(1));
    let dummy_args: Vec<CallArgInfo> = Vec::new();
    // Single-line: the textual fallback works from raw text with no
    // argument spans, so it can't tell how the call was laid out.
    let mut help = build_help(name, Some(sig), &dummy_args, 0, false)?;
    if let Some(s) = help.signatures.first_mut() {
        s.active_parameter = Some(active as u32);
    }
    help.active_parameter = Some(active as u32);
    Some(help)
}

/// Textual-fallback variant of [`build_help_native`] — same output
/// shape but uses a comma-derived `active` index instead of arg-span
/// containment.
pub(crate) fn build_help_native_simple(
    display: &str,
    qname: &str,
    sig: &saule_typeck::sigs::NativeSig,
    active: usize,
) -> Option<SignatureHelp> {
    let total = sig.params.len() + usize::from(sig.variadic.is_some());
    let active = if total == 0 { 0 } else { active.min(total - 1) };
    let dummy_args: Vec<CallArgInfo> = Vec::new();
    let mut help = build_help_native(display, qname, sig, &dummy_args, 0, false)?;
    if let Some(s) = help.signatures.first_mut() {
        s.active_parameter = Some(active as u32);
    }
    help.active_parameter = Some(active as u32);
    Some(help)
}

/// Best-effort scan for the enclosing `class Name` whose body brackets
/// the cursor. Used by the textual fallback so unqualified calls
/// inside a method still resolve to sibling methods.
pub(crate) fn enclosing_class_textual(source: &str, offset: usize) -> Option<String> {
    // Find the last `class <Name>` keyword strictly before the cursor;
    // bail if we then see an `end` at column 0 between the class head
    // and the cursor (rough scope check — cheap and good enough for
    // mid-keystroke recovery).
    let prefix = source.get(..offset.min(source.len()))?;
    let mut last: Option<String> = None;
    for (idx, _) in prefix.match_indices("class ") {
        // Require the keyword to be at start-of-line (ignoring leading
        // whitespace) so we don't trip on `class` inside identifiers.
        let line_start = prefix[..idx].rfind('\n').map(|n| n + 1).unwrap_or(0);
        if prefix[line_start..idx].chars().all(|c| c.is_whitespace()) {
            let after = &prefix[idx + "class ".len()..];
            let name: String = after
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                last = Some(name);
            }
        }
    }
    last
}

/// Textual-fallback variant of [`collect_user_fns`]: re-lexes and
/// re-parses the source defensively to recover top-level `fn name`
/// declarations even when the document as a whole fails to parse
/// (the failing tokens may be elsewhere in the file).
pub(crate) fn lookup_user_fn_textual(
    source: &str,
    name: &str,
) -> Option<(Vec<Param>, Option<Type>)> {
    // First try parsing the source as-is.
    if let Ok(tokens) = saule_lexer::Lexer::new(source).tokenize()
        && let Ok(module) = saule_parser::parse(tokens)
        && let Some(found) = collect_user_fns(&module).remove(name)
    {
        return Some(found);
    }
    // Mid-keystroke: the buffer has an unclosed `(` somewhere. Try
    // synthesising a closed buffer by appending `) end` enough times
    // to balance any open call/block. This is crude but cheap and
    // recovers the `fn name(...) ... end` declarations the user
    // already finished typing.
    for suffix in [") end", ") end\nend", ") end\nend\nend"] {
        let patched = format!("{source}{suffix}");
        if let Ok(tokens) = saule_lexer::Lexer::new(&patched).tokenize()
            && let Ok(module) = saule_parser::parse(tokens)
            && let Some(found) = collect_user_fns(&module).remove(name)
        {
            return Some(found);
        }
    }
    None
}

/// Textual-fallback variant of [`build_help_user_fn`]: reuses the
/// AST path's renderer but feeds it a comma-derived `active` index
/// instead of arg-span containment.
pub(crate) fn build_help_user_fn_simple(
    name: &str,
    params: &[Param],
    return_ty: &Option<Type>,
    active: usize,
) -> Option<SignatureHelp> {
    let total = params.len().max(1);
    let active = active.min(total - 1);
    let mut help = build_help_user_fn(name, params, return_ty, &[], 0, false)?;
    if let Some(s) = help.signatures.first_mut() {
        s.active_parameter = Some(active as u32);
    }
    help.active_parameter = Some(active as u32);
    Some(help)
}
