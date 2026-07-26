//! Signature help — shows the active call's signature with the
//! parameter under the cursor highlighted. Triggered by `(`, `,`, and
//! `:` (for named-argument keys).
//!
//! Implementation strategy:
//!
//! 1. Lex / parse the document. A buffer mid-edit usually *doesn't*
//!    parse — the call being typed has no `)` yet — so on failure
//!    [`repair_parse`] appends the missing closers and parses that.
//!    Appending never shifts the offsets before the cursor, so the
//!    same walk works on the repaired tree.
//! 2. Walk the AST to find the smallest `Expr::Call` /
//!    `Expr::MethodCall` / pipeline stage whose argument span (the
//!    `(...)` parens region) contains the cursor.
//! 3. Resolve the callee to a parameter list: free function, class
//!    constructor, method (on a local, field, parameter, or call
//!    result), sibling / static method, `self.super`, enum tuple
//!    variant, function-typed local, or a stdlib native.
//! 4. Compute `active_parameter` from how many `,`-separated
//!    arguments precede the cursor — or, for a `name: value`
//!    argument, from the slot its key names.
//! 5. Render `name(p1: T1, p2: T2, ...)` with the active param
//!    highlighted via `parameters[*]` ranges into the label string.
//!
//! [`textual_fallback`] remains as a last resort for buffers even the
//! repair can't parse; it resolves the callee by name from raw text.

use saule_ast::{
    CallArg, ClassMember, Decl, Expr, LambdaBody, MatchBody, Method, Module, Param, Spanned,
    Stmt, TableEntry, Type,
};
use saule_semantic::{lookup_field_type, lookup_method, super_init_target, with_classes};
use std::collections::HashMap;
use tower_lsp::lsp_types::{
    ParameterInformation, ParameterLabel, Position, SignatureHelp, SignatureInformation, Url,
};

use crate::line_index::LineIndex;

use super::{canonical, Backend};

impl Backend {
    /// Resolve signature help at `pos` inside `uri`. Returns `None`
    /// when the cursor isn't inside any call's arg list, the callee
    /// can't be resolved to a known signature, or the document is
    /// closed / fails to parse.
    pub(super) async fn signature_help_at(
        &self,
        uri: &Url,
        pos: Position,
    ) -> Option<SignatureHelp> {
        let entry = self.docs.get(uri.as_str())?;
        let source = entry.source.clone();
        drop(entry);

        let line_index = LineIndex::new(&source);
        let offset = line_index.offset(&source, pos);

        // Seed registries from the last successfully-parsed view of
        // this module so identifier resolution works even when the
        // current edit is mid-keystroke and the parser fails.
        let module_dir = uri
            .to_file_path()
            .ok()
            .and_then(|p| canonical(&p))
            .and_then(|p| p.parent().map(|d| d.to_path_buf()));
        let _guard = self.analysis_lock.lock().await;
        if let Some(info) = self.project_info.lock().await.clone() {
            saule_interpreter::project::set(info);
        }

        let parsed = saule_lexer::Lexer::new(&source)
            .tokenize()
            .ok()
            .and_then(|tokens| saule_parser::parse(tokens).ok());

        if let Some(module) = parsed.as_ref() {
            let seed = match &module_dir {
                Some(d) => saule_interpreter::module::collect_import_seed(module, d),
                None => saule_semantic::ModuleSeed::default(),
            };
            let _ = saule_semantic::analyze_with_seed(module, seed);

            if let Some(help) = help_from_module(module, offset) {
                return Some(help);
            }
        } else if let Some(module) = repair_parse(&source, offset) {
            // Mid-keystroke (`w.moveTo(`, `add(1, `): close the call the
            // user is typing and re-run the real walker. Everything is
            // appended at the end, so byte offsets up to the cursor —
            // and therefore the cursor itself — are unaffected.
            let seed = match &module_dir {
                Some(d) => saule_interpreter::module::collect_import_seed(&module, d),
                None => saule_semantic::ModuleSeed::default(),
            };
            let _ = saule_semantic::analyze_with_seed(&module, seed);

            if let Some(help) = help_from_module(&module, offset) {
                return Some(help);
            }
        }

        // Last resort: the repair didn't parse either (unbalanced
        // brackets elsewhere, a broken string, ...). Scan the raw source
        // for the innermost unmatched `(` and resolve the call by name.
        textual_fallback(&source, offset).and_then(drop_parameterless)
    }
}

/// Drop signatures that take no arguments, and the whole response if
/// nothing is left.
///
/// A parameterless call has nothing to tell you, and IntelliJ renders it
/// as a literal `<no parameters>` row — noise sitting next to the call
/// you're actually filling in, as with `update(dt: Timer.getDelta(`.
///
/// The AST path filters earlier, in [`help_from_module`], where spans
/// are still around to reselect the enclosing level. This is the same
/// rule for the textual fallback, which only ever has one signature.
fn drop_parameterless(help: SignatureHelp) -> Option<SignatureHelp> {
    let was_active = help
        .active_signature
        .and_then(|i| help.signatures.get(i as usize))
        .map(|s| s.label.clone());

    let signatures: Vec<SignatureInformation> = help
        .signatures
        .into_iter()
        .filter(|s| s.parameters.as_ref().is_some_and(|p| !p.is_empty()))
        .collect();
    if signatures.is_empty() {
        return None;
    }

    // If the caret was in the parameterless call we just removed, fall
    // back to the outermost surviving level rather than showing nothing.
    let active = was_active
        .and_then(|l| signatures.iter().position(|s| s.label == l))
        .unwrap_or(0);
    let active_parameter = signatures[active].active_parameter;
    Some(SignatureHelp {
        signatures,
        active_signature: Some(active as u32),
        active_parameter,
    })
}

impl Backend {
    /// Log one line per signature-help request to the client's LSP
    /// console: the caret's line as *the server* has it, with `[|]`
    /// marking where the client said the caret is, and the signature we
    /// answered with.
    ///
    /// Worth its noise because the three ways this feature goes wrong —
    /// a stale document, a caret that isn't where it looks like it is,
    /// and a genuinely wrong resolution — are indistinguishable from a
    /// screenshot of the popup. Set `SAULE_LSP_TRACE=1` to enable.
    pub(super) async fn trace_signature_help(
        &self,
        uri: &Url,
        pos: Position,
        help: &Option<SignatureHelp>,
    ) {
        if std::env::var_os("SAULE_LSP_TRACE").is_none() {
            return;
        }
        let line = self
            .docs
            .get(uri.as_str())
            .and_then(|e| e.source.lines().nth(pos.line as usize).map(str::to_string))
            .unwrap_or_else(|| "<no such line>".into());
        // Insert the marker by UTF-16 column, the unit the client counts in.
        let units: Vec<u16> = line.encode_utf16().collect();
        let col = (pos.character as usize).min(units.len());
        let marked = format!(
            "{}[|]{}",
            String::from_utf16_lossy(&units[..col]),
            String::from_utf16_lossy(&units[col..])
        );
        let got = help
            .as_ref()
            .and_then(|h| h.signatures.first())
            .map(|s| s.label.as_str())
            .unwrap_or("<none>");
        self.client
            .log_message(
                tower_lsp::lsp_types::MessageType::INFO,
                format!("sighelp {}:{} -> {got}  ::  {marked}", pos.line, pos.character),
            )
            .await;
    }
}

fn resolve_hit(hit: CallHit, offset: usize) -> Option<SignatureHelp> {
    match hit.callee {
        CalleeRef::Free(name) => {
            if with_classes(|r| r.contains_key(&name)) {
                return build_help(&name, lookup_method(&name, "init"), &hit.args, offset);
            }
            if let Some(class) = &hit.enclosing_class {
                if let Some(sig) = lookup_method(class, &name) {
                    return build_help(&name, Some(sig), &hit.args, offset);
                }
            }
            // Callback held in a local / parameter: `f(...)` where
            // `f: fn(integer) -> string`. The type carries no parameter
            // names, so they're synthesised.
            if let Some((params, ret)) = hit.local_fn {
                let params: Vec<Param> = params
                    .into_iter()
                    .enumerate()
                    .map(|(i, ty)| Param {
                        name: format!("arg{i}"),
                        ty,
                        default: None,
                        variadic: false,
                        span: 0..0,
                    })
                    .collect();
                return build_help_user_fn(&name, &params, &Some(ret), &hit.args, offset);
            }
            // User-defined top-level function (collected by the AST
            // walker into `hit.user_fn`).
            if let Some((params, ret)) = hit.user_fn {
                return build_help_user_fn(&name, &params, &ret, &hit.args, offset);
            }
            // Bare native (`println`, `assert`, ...).
            if let Some(native) = saule_typeck::sigs::lookup(&name) {
                return build_help_native(&name, &name, &native, &hit.args, offset);
            }
            None
        }
        CalleeRef::Method { class, name } => {
            if let Some(sig) = lookup_method(&class, &name) {
                return build_help(&name, Some(sig), &hit.args, offset);
            }
            // Stdlib value-type instance method (e.g. `file.write` where
            // `file: File`). Native sigs are registered as `File.write`.
            let qname = format!("{class}.{name}");
            if let Some(native) = saule_typeck::sigs::lookup(&qname) {
                return build_help_native(&name, &qname, &native, &hit.args, offset);
            }
            None
        }
        CalleeRef::SuperInit { owner } => build_help(
            &format!("{owner}.init"),
            lookup_method(&owner, "init"),
            &hit.args,
            offset,
        ),
        CalleeRef::Variant { display, fields } => {
            build_help_user_fn(&display, &fields, &None, &hit.args, offset)
        }
        CalleeRef::PipeStage(name) => {
            // Resolve the stage like a free call, then drop the first
            // parameter — the pipeline supplies it.
            let sig = hit
                .enclosing_class
                .as_ref()
                .and_then(|c| lookup_method(c, &name))
                .or_else(|| {
                    hit.user_fn.clone().map(|(params, ret)| saule_semantic::MethodSig {
                        is_static: false,
                        is_private: false,
                        type_params: Vec::new(),
                        params,
                        return_ty: ret,
                    })
                })?;
            let piped = saule_semantic::MethodSig {
                params: sig.params.iter().skip(1).cloned().collect(),
                ..sig
            };
            build_help(&name, Some(piped), &hit.args, offset)
        }
        CalleeRef::Native(qname) => {
            let native = saule_typeck::sigs::lookup(&qname)?;
            let display = qname
                .rsplit_once('.')
                .map(|(_, n)| n)
                .unwrap_or(&qname);
            build_help_native(display, &qname, &native, &hit.args, offset)
        }
    }
}

/// Try to make a mid-keystroke buffer parse by appending a closing
/// suffix: `)` for `foo(`, `nil)` for `foo(1, ` (an empty slot after a
/// comma isn't an expression), each with enough `end`s to close the
/// blocks the call sits in. First candidate that parses wins.
///
/// Only ever appends, so every byte offset in the original source —
/// including the cursor — keeps its meaning.
fn repair_parse(source: &str, offset: usize) -> Option<Module> {
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
fn unclosed_delimiters(source: &str) -> String {
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

/// Scan raw source for the innermost unmatched `(` strictly before
/// `offset`, count the top-level commas that separate arguments, and
/// resolve the identifier sitting just before that `(`.
///
/// This is the lifeline that keeps signature help visible during
/// mid-keystroke edits like `add(1, ` where the parser can't yet
/// produce a usable AST.
fn textual_fallback(source: &str, offset: usize) -> Option<SignatureHelp> {
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
            b',' => {
                if paren_depth == 0 && bracket_depth == 0 && brace_depth == 0 {
                    commas += 1;
                }
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
        if with_classes(|r| r.contains_key(recv)) {
            if let Some(sig) = lookup_method(recv, name) {
                return build_help_simple(name, sig, active);
            }
        }
        // Stdlib module call (`Os.exists`, `String.find`, ...) or
        // value-type instance method (`File.write`).
        let qname = format!("{recv}.{name}");
        if let Some(native) = saule_typeck::sigs::lookup(&qname) {
            return build_help_native_simple(name, &qname, &native, active);
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
    if let Some(class) = enclosing_class_textual(source, offset) {
        if let Some(sig) = lookup_method(&class, name) {
            return build_help_simple(name, sig, active);
        }
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
fn build_help_simple(
    name: &str,
    sig: saule_semantic::MethodSig,
    active: usize,
) -> Option<SignatureHelp> {
    let arity = sig.params.len();
    let active = active.min(arity.saturating_sub(1));
    let dummy_args: Vec<CallArgInfo> = Vec::new();
    let mut help = build_help(name, Some(sig), &dummy_args, 0)?;
    if let Some(s) = help.signatures.first_mut() {
        s.active_parameter = Some(active as u32);
    }
    help.active_parameter = Some(active as u32);
    Some(help)
}

/// Render a `NativeSig` as a `SignatureHelp`. Native sigs have no
/// param names, so we synthesize `arg0`, `arg1`, ... — cheap, but
/// good enough for stdlib calls where the type is the load-bearing
/// piece of information anyway.
fn build_help_native(
    display: &str,
    qname: &str,
    sig: &saule_typeck::sigs::NativeSig,
    args: &[CallArgInfo],
    offset: usize,
) -> Option<SignatureHelp> {
    let names = super::native_names::param_names(qname, sig);
    let mut label = String::new();
    label.push_str(display);
    label.push('(');
    let mut param_ranges: Vec<(u32, u32)> = Vec::new();
    let positional_n = sig.params.len();
    for (i, ty) in sig.params.iter().enumerate() {
        if i > 0 {
            label.push_str(", ");
        }
        let start = utf16_len(&label);
        let pname = names.get(i).map(|s| s.as_str()).unwrap_or("value");
        label.push_str(pname);
        label.push_str(": ");
        label.push_str(&render_type(ty));
        let end = utf16_len(&label);
        param_ranges.push((start, end));
    }
    if let Some(var_ty) = &sig.variadic {
        if positional_n > 0 {
            label.push_str(", ");
        }
        let start = utf16_len(&label);
        let vname = names.get(positional_n).map(|s| s.as_str()).unwrap_or("rest");
        label.push_str("...");
        label.push_str(vname);
        label.push_str(": ");
        label.push_str(&render_type(var_ty));
        let end = utf16_len(&label);
        param_ranges.push((start, end));
    }
    label.push(')');
    if !sig.returns.is_empty() {
        label.push_str(" -> ");
        let parts: Vec<String> = sig.returns.iter().map(render_type).collect();
        label.push_str(&parts.join(", "));
    }

    let arity_with_var = param_ranges.len();
    let active = active_parameter(args, offset, arity_with_var);

    let parameters = param_ranges
        .into_iter()
        .map(|(s, e)| ParameterInformation {
            label: ParameterLabel::LabelOffsets([s, e]),
            documentation: None,
        })
        .collect::<Vec<_>>();

    Some(SignatureHelp {
        signatures: vec![SignatureInformation {
            label,
            documentation: None,
            parameters: Some(parameters),
            active_parameter: Some(active as u32),
        }],
        active_signature: Some(0),
        active_parameter: Some(active as u32),
    })
}

/// Textual-fallback variant of [`build_help_native`] — same output
/// shape but uses a comma-derived `active` index instead of arg-span
/// containment.
fn build_help_native_simple(
    display: &str,
    qname: &str,
    sig: &saule_typeck::sigs::NativeSig,
    active: usize,
) -> Option<SignatureHelp> {
    let total = sig.params.len() + usize::from(sig.variadic.is_some());
    let active = if total == 0 { 0 } else { active.min(total - 1) };
    let dummy_args: Vec<CallArgInfo> = Vec::new();
    let mut help = build_help_native(display, qname, sig, &dummy_args, 0)?;
    if let Some(s) = help.signatures.first_mut() {
        s.active_parameter = Some(active as u32);
    }
    help.active_parameter = Some(active as u32);
    Some(help)
}

/// Build help for a user-defined free top-level function. We pull
/// `Param` records (with names!) directly from the AST, so the
/// rendering matches what `build_help` produces for class methods.
fn build_help_user_fn(
    name: &str,
    params: &[Param],
    return_ty: &Option<Type>,
    args: &[CallArgInfo],
    offset: usize,
) -> Option<SignatureHelp> {
    use saule_semantic::MethodSig;
    let sig = MethodSig {
        is_static: false,
        is_private: false,
        type_params: Vec::new(),
        params: params.to_vec(),
        return_ty: return_ty.clone(),
    };
    build_help(name, Some(sig), args, offset)
}

/// Best-effort scan for the enclosing `class Name` whose body brackets
/// the cursor. Used by the textual fallback so unqualified calls
/// inside a method still resolve to sibling methods.
fn enclosing_class_textual(source: &str, offset: usize) -> Option<String> {
    // Find the last `class <Name>` keyword strictly before the cursor;
    // bail if we then see an `end` at column 0 between the class head
    // and the cursor (rough scope check — cheap and good enough for
    // mid-keystroke recovery).
    let prefix = source.get(..offset.min(source.len()))?;
    let mut last: Option<String> = None;
    for (idx, _) in prefix.match_indices("class ") {
        // Require the keyword to be at start-of-line (ignoring leading
        // whitespace) so we don't trip on `class` inside identifiers.
        let line_start = prefix[..idx]
            .rfind('\n')
            .map(|n| n + 1)
            .unwrap_or(0);
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

fn build_help(
    name: &str,
    sig: Option<saule_semantic::MethodSig>,
    args: &[CallArgInfo],
    offset: usize,
) -> Option<SignatureHelp> {
    let sig = sig?;
    // Render `name(p1: T1, p2: T2, ...)` and remember each param's
    // [start, end) byte offsets within the rendered label so the
    // editor can highlight the active one.
    let mut label = String::new();
    label.push_str(name);
    label.push('(');
    let mut param_ranges: Vec<(u32, u32)> = Vec::new();
    for (i, p) in sig.params.iter().enumerate() {
        if i > 0 {
            label.push_str(", ");
        }
        let start = utf16_len(&label);
        label.push_str(&p.name);
        label.push_str(": ");
        label.push_str(&render_type(&p.ty));
        if p.variadic {
            label.push_str("...");
        }
        if p.default.is_some() {
            label.push_str(" = …");
        }
        let end = utf16_len(&label);
        param_ranges.push((start, end));
    }
    label.push(')');
    if let Some(rt) = &sig.return_ty {
        label.push_str(" -> ");
        label.push_str(&render_type(rt));
    }

    let parameters = param_ranges
        .into_iter()
        .map(|(s, e)| ParameterInformation {
            label: ParameterLabel::LabelOffsets([s, e]),
            documentation: None,
        })
        .collect::<Vec<_>>();

    // Now that the signature is known, point each `name: value`
    // argument at the slot its key names.
    let resolved: Vec<CallArgInfo> = args
        .iter()
        .map(|a| CallArgInfo {
            named_index: a
                .name
                .as_ref()
                .and_then(|n| sig.params.iter().position(|p| &p.name == n)),
            ..a.clone()
        })
        .collect();
    let active = active_parameter(&resolved, offset, sig.params.len());

    Some(SignatureHelp {
        signatures: vec![SignatureInformation {
            label,
            documentation: None,
            parameters: Some(parameters),
            active_parameter: Some(active as u32),
        }],
        active_signature: Some(0),
        active_parameter: Some(active as u32),
    })
}

/// Index of the currently-edited parameter slot. Counts how many
/// completed positional/named arguments precede the cursor — a
/// trailing `,` means the *next* param is active. Clamped to the
/// signature's arity so we never report an out-of-range slot.
fn active_parameter(args: &[CallArgInfo], offset: usize, arity: usize) -> usize {
    let mut idx = 0;
    for arg in args {
        // The cursor is inside this arg's span -> this slot is active.
        if offset >= arg.span.start && offset <= arg.span.end {
            return arg.named_index.unwrap_or(idx).min(arity.saturating_sub(1));
        }
        // Cursor lies past the arg -> we've completed it; advance.
        if offset > arg.span.end {
            idx += 1;
        }
    }
    idx.min(arity.saturating_sub(1).max(0))
}

/// Length of `s` in UTF-16 code units.
///
/// `ParameterInformation.label` offsets are indices into the signature
/// label *as the client sees it*, and LSP defines string positions in
/// UTF-16 code units — not Rust byte offsets. The label can contain
/// non-ASCII (`" = …"` for a defaulted parameter), so handing out
/// `label.len()` makes every offset past that point too large and the
/// client slices out of bounds.
fn utf16_len(s: &str) -> u32 {
    s.encode_utf16().count() as u32
}

pub(super) fn render_type(ty: &Type) -> String {
    match ty {
        Type::Named(n) => n.clone(),
        Type::Nullable(inner) => format!("{}?", render_type(inner)),
        Type::Function { params, ret } => {
            let ps = params
                .iter()
                .map(render_type)
                .collect::<Vec<_>>()
                .join(", ");
            format!("fn({ps}) -> {}", render_type(ret))
        }
        Type::Table { key, value } => match key {
            Some(k) => format!("table<{}, {}>", render_type(k), render_type(value)),
            None => format!("table<{}>", render_type(value)),
        },
        Type::Tuple(parts) => format!(
            "({})",
            parts
                .iter()
                .map(render_type)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

// ──────────────────────────────────────────────────────────────────────
// AST walk: locate the smallest enclosing call
// ──────────────────────────────────────────────────────────────────────

#[derive(Clone)]
enum CalleeRef {
    Free(String),
    Method { class: String, name: String },
    /// `self.super(...)` — the `init` of `owner`, the nearest ancestor
    /// that declares one. Kept separate from `Method` so the rendered
    /// label can say `View.init` instead of a bare `init`.
    SuperInit { owner: String },
    /// `Shape.Circle(...)` — a tuple-style enum variant used as a
    /// constructor. Its fields are `Param`s on the AST, so they're
    /// carried here directly rather than looked up in a registry.
    Variant { display: String, fields: Vec<Param> },
    /// `when(x):stage(...)` — the piped value fills the first parameter,
    /// so the rendered signature drops it.
    PipeStage(String),
    /// Stdlib qualified name like `"Os.exists"` or bare native like
    /// `"println"` — resolved through `saule_typeck::sigs::lookup`.
    Native(String),
}

#[derive(Clone)]
struct CallArgInfo {
    span: std::ops::Range<usize>,
    /// Key of a `name: value` argument. Resolved to a parameter slot in
    /// [`build_help`], where the signature is known, so that typing
    /// inside `Widget(y: …)` highlights `y` rather than slot 0.
    name: Option<String>,
    /// Slot this argument fills, once resolved against the signature.
    named_index: Option<usize>,
}

struct CallHit {
    callee: CalleeRef,
    args: Vec<CallArgInfo>,
    enclosing_class: Option<String>,
    /// Resolved params for a user-defined free function — populated
    /// by the walker when it sees `Expr::Call` whose callee identifier
    /// matches a top-level `Decl::Function`.
    user_fn: Option<(Vec<Param>, Option<Type>)>,
    /// Param / return types when the callee identifier is a local or
    /// parameter of function type. Takes precedence over `user_fn`,
    /// matching the resolver's locals-shadow-globals rule.
    local_fn: Option<(Vec<Type>, Type)>,
    /// Source span of the entire arg list region — used to disambiguate
    /// nested calls (the smallest enclosing call wins).
    args_span: std::ops::Range<usize>,
}

struct Cx {
    offset: usize,
    locals: Vec<Local>,
    enclosing_class: Option<String>,
    /// Top-level user functions discovered in the module. Populated
    /// during a single pre-pass before `visit_module` so call sites
    /// resolve regardless of declaration order.
    user_fns: HashMap<String, (Vec<Param>, Option<Type>)>,
    /// `(enum, variant) -> fields` for tuple-style variants, which are
    /// called like constructors. Same pre-pass rationale as `user_fns`.
    enum_variants: HashMap<(String, String), Vec<Param>>,
    /// Collection filter for [`Cx::record`]. `None` collects the calls
    /// containing the cursor; `Some(r)` collects everything nested
    /// inside `r`.
    region: Option<std::ops::Range<usize>>,
    hits: Vec<CallHit>,
}

struct Local {
    name: String,
    ty: Type,
}

impl Cx {
    /// Two collection modes, one per pass in [`help_from_module`].
    ///
    /// `region: None` — only calls whose argument list contains the
    /// cursor, i.e. the enclosing chain.
    ///
    /// `region: Some(r)` — every call nested anywhere inside `r`,
    /// whether or not it contains the cursor. This is what makes the
    /// signature list identical for every caret position inside one
    /// call expression.
    fn record(&mut self, hit: CallHit) {
        let keep = match &self.region {
            None => contains(&hit.args_span, self.offset),
            Some(r) => hit.args_span.start >= r.start && hit.args_span.end <= r.end,
        };
        if keep {
            self.hits.push(hit);
        }
    }

    fn visit_module(&mut self, module: &Module) {
        for s in &module.stmts {
            self.visit_stmt(s);
        }
    }

    fn visit_stmt(&mut self, s: &Spanned<Stmt>) {
        match &s.value {
            Stmt::Local { name, ty, value, .. } => {
                if let Some(v) = value {
                    self.visit_expr(v);
                }
                let ty = ty.clone().unwrap_or_else(|| match value {
                    Some(v) => self.infer_local_ty(&v.value),
                    None => Type::Named("any".into()),
                });
                self.locals.push(Local {
                    name: name.clone(),
                    ty,
                });
            }
            Stmt::LocalMulti { names, values } => {
                for v in values {
                    self.visit_expr(v);
                }
                for (i, (n, _, t)) in names.iter().enumerate() {
                    let ty = t.clone().unwrap_or_else(|| match values.get(i) {
                        Some(v) => self.infer_local_ty(&v.value),
                        None => Type::Named("any".into()),
                    });
                    self.locals.push(Local {
                        name: n.clone(),
                        ty,
                    });
                }
            }
            Stmt::Decl(d) => self.visit_decl(d),
            Stmt::Assign { target, value } => {
                self.visit_expr(target);
                self.visit_expr(value);
            }
            Stmt::AssignMulti { targets, values } => {
                for t in targets {
                    self.visit_expr(t);
                }
                for v in values {
                    self.visit_expr(v);
                }
            }
            Stmt::Expr(e) => self.visit_expr(e),
            Stmt::If {
                cond,
                then_block,
                elseifs,
                else_block,
            } => {
                self.visit_expr(cond);
                self.visit_block(then_block);
                for (c, b) in elseifs {
                    self.visit_expr(c);
                    self.visit_block(b);
                }
                if let Some(b) = else_block {
                    self.visit_block(b);
                }
            }
            Stmt::While { cond, body } => {
                self.visit_expr(cond);
                self.visit_block(body);
            }
            Stmt::Repeat { body, cond } => {
                self.visit_block(body);
                self.visit_expr(cond);
            }
            Stmt::ForNumeric {
                var,
                from,
                to,
                step,
                body,
                ..
            } => {
                self.visit_expr(from);
                self.visit_expr(to);
                if let Some(s) = step {
                    self.visit_expr(s);
                }
                let mark = self.locals.len();
                self.locals.push(Local {
                    name: var.clone(),
                    ty: Type::Named("integer".into()),
                });
                self.visit_block(body);
                self.locals.truncate(mark);
            }
            Stmt::ForIn { vars, iter, body } => {
                self.visit_expr(iter);
                let mark = self.locals.len();
                for (n, _) in vars {
                    self.locals.push(Local {
                        name: n.clone(),
                        ty: Type::Named("any".into()),
                    });
                }
                self.visit_block(body);
                self.locals.truncate(mark);
            }
            Stmt::Return(es) => {
                for e in es {
                    self.visit_expr(e);
                }
            }
            Stmt::Throw(e) => self.visit_expr(e),
            Stmt::Try { body, catch_body, .. } => {
                self.visit_block(body);
                self.visit_block(catch_body);
            }
            Stmt::Break | Stmt::Continue => {}
        }
    }

    fn visit_decl(&mut self, d: &Spanned<Decl>) {
        match &d.value {
            Decl::Function { params, body, .. } => {
                self.with_function(params, |this| this.visit_block(body));
            }
            Decl::Class { name, members, .. } => {
                let prev = self.enclosing_class.replace(name.clone());
                for m in members {
                    if let ClassMember::Method(meth) = &m.value {
                        self.visit_method(meth);
                    }
                    if let ClassMember::Field { default: Some(d), .. } = &m.value {
                        self.visit_expr(d);
                    }
                }
                self.enclosing_class = prev;
            }
            Decl::Enum { methods, .. } => {
                for m in methods {
                    self.visit_method(m);
                }
            }
            Decl::Interface { .. } | Decl::Import { .. } => {}
        }
    }

    fn visit_method(&mut self, m: &Method) {
        self.with_function(&m.params, |this| this.visit_block(&m.body));
    }

    fn with_function(&mut self, params: &[Param], body: impl FnOnce(&mut Self)) {
        let saved = std::mem::take(&mut self.locals);
        for p in params {
            self.locals.push(Local {
                name: p.name.clone(),
                ty: p.ty.clone(),
            });
        }
        body(self);
        self.locals = saved;
    }

    fn visit_block(&mut self, body: &[Spanned<Stmt>]) {
        let mark = self.locals.len();
        for s in body {
            self.visit_stmt(s);
        }
        self.locals.truncate(mark);
    }

    fn visit_expr(&mut self, e: &Spanned<Expr>) {
        match &e.value {
            Expr::Call { callee, args } => {
                self.visit_expr(callee);
                for a in args {
                    visit_arg(self, a);
                }
                if let Some(callee_ref) = self.callee_ref(&callee.value) {
                    let (user_fn, local_fn) = match &callee_ref {
                        CalleeRef::Free(n) => (self.user_fns.get(n).cloned(), self.local_fn(n)),
                        _ => (None, None),
                    };
                    self.record(CallHit {
                        callee: callee_ref,
                        args: build_arg_infos(args),
                        enclosing_class: self.enclosing_class.clone(),
                        user_fn,
                        local_fn,
                        args_span: args_span(&callee.span, args, e.span.end),
                    });
                }
            }
            Expr::MethodCall { obj, method, args } => {
                self.visit_expr(obj);
                for a in args {
                    visit_arg(self, a);
                }
                if let Some(class) = self.receiver_class(&obj.value) {
                    self.record(CallHit {
                        callee: CalleeRef::Method {
                            class,
                            name: method.clone(),
                        },
                        args: build_arg_infos(args),
                        enclosing_class: self.enclosing_class.clone(),
                        user_fn: None,
                        local_fn: None,
                        args_span: method_args_span(&obj.span, method, e.span.end),
                    });
                }
            }
            Expr::Pipe { source, stages } => {
                self.visit_expr(source);
                for st in stages {
                    for a in &st.args {
                        visit_arg(self, a);
                    }
                    // `:name(` — the arg region starts after the stage
                    // name, which sits one `:` past the stage's start.
                    let args_start = st.span.start + 1 + st.name.len();
                    self.record(CallHit {
                        callee: CalleeRef::PipeStage(st.name.clone()),
                        args: build_arg_infos(&st.args),
                        enclosing_class: self.enclosing_class.clone(),
                        user_fn: self.user_fns.get(&st.name).cloned(),
                        local_fn: None,
                        args_span: args_start.min(st.span.end)..st.span.end,
                    });
                }
            }
            Expr::Unary { rhs, .. } => self.visit_expr(rhs),
            Expr::Binary { lhs, rhs, .. } => {
                self.visit_expr(lhs);
                self.visit_expr(rhs);
            }
            Expr::Member { obj, .. } | Expr::SafeMember { obj, .. } => self.visit_expr(obj),
            Expr::Index { obj, index } => {
                self.visit_expr(obj);
                self.visit_expr(index);
            }
            Expr::ForceUnwrap(inner) => self.visit_expr(inner),
            Expr::Table(entries) => {
                for entry in entries {
                    match entry {
                        TableEntry::Positional(v) => self.visit_expr(v),
                        TableEntry::Field { key, value } => {
                            self.visit_expr(key);
                            self.visit_expr(value);
                        }
                    }
                }
            }
            Expr::Lambda { params, body, .. } => {
                self.with_function(params, |this| match body {
                    LambdaBody::Expr(b) => this.visit_expr(b),
                    LambdaBody::Block(b) => this.visit_block(b),
                });
            }
            Expr::Match { scrutinee, arms } => {
                self.visit_expr(scrutinee);
                for arm in arms {
                    if let Some(g) = &arm.guard {
                        self.visit_expr(g);
                    }
                    match &arm.body {
                        MatchBody::Expr(e) => self.visit_expr(e),
                        MatchBody::Block(b) => self.visit_block(b),
                    }
                }
            }
            Expr::Int(_)
            | Expr::Float(_)
            | Expr::Bool(_)
            | Expr::Str(_)
            | Expr::Nil
            | Expr::Ident(_)
            | Expr::Self_ => {}
        }
    }

    fn callee_ref(&self, callee: &Expr) -> Option<CalleeRef> {
        match callee {
            Expr::Ident(name) => Some(CalleeRef::Free(name.clone())),
            Expr::Member { obj, name } => {
                // Tuple-style enum variant used as a constructor:
                // `Shape.Circle(1.0)`.
                if let Expr::Ident(enum_name) = &obj.value
                    && let Some(fields) =
                        self.enum_variants.get(&(enum_name.clone(), name.clone()))
                {
                    return Some(CalleeRef::Variant {
                        display: format!("{enum_name}.{name}"),
                        fields: fields.clone(),
                    });
                }
                // `self.super(...)` delegates to the parent constructor;
                // there is no member called `super` to look up.
                if name == "super"
                    && matches!(obj.value, Expr::Self_)
                    && let Some(class) = &self.enclosing_class
                    && let Some((owner, _)) = super_init_target(class)
                {
                    return Some(CalleeRef::SuperInit { owner });
                }
                // Stdlib static call (`Os.exists`, `String.find`, ...) —
                // these aren't user classes so receiver_class can't see
                // them. Probe the typeck sig registry directly.
                if let Expr::Ident(mod_name) = &obj.value {
                    let qname = format!("{mod_name}.{name}");
                    if saule_typeck::sigs::lookup(&qname).is_some() {
                        return Some(CalleeRef::Native(qname));
                    }
                }
                let class = self.receiver_class(&obj.value)?;
                Some(CalleeRef::Method {
                    class,
                    name: name.clone(),
                })
            }
            _ => None,
        }
    }

    fn receiver_class(&self, obj: &Expr) -> Option<String> {
        match obj {
            Expr::Self_ => self.enclosing_class.clone(),
            Expr::Ident(name) => {
                if let Some(local) = self.locals.iter().rev().find(|l| l.name == *name)
                    && let Some(n) = class_of(&local.ty)
                {
                    return Some(n);
                }
                if with_classes(|r| r.contains_key(name)) {
                    return Some(name.clone());
                }
                None
            }
            // `obj.field.method(` — the field's declared type carries
            // the class the method is looked up on.
            Expr::Member { obj: inner, name } => {
                let inner_class = self.receiver_class(&inner.value)?;
                class_of(&lookup_field_type(&inner_class, name)?)
            }
            Expr::Call { callee, .. } => match &callee.value {
                // Constructor: `Widget(...).method(`.
                Expr::Ident(n) if with_classes(|r| r.contains_key(n)) => Some(n.clone()),
                // Method or static call returning a class:
                // `Widget.make(1).moveTo(`, `self.child().moveTo(`.
                Expr::Member { obj: inner, name } => {
                    let inner_class = self.receiver_class(&inner.value)?;
                    class_of(&lookup_method(&inner_class, name)?.return_ty?)
                }
                _ => None,
            },
            Expr::MethodCall { obj, method, .. } => {
                let cls = self.receiver_class(&obj.value)?;
                class_of(&lookup_method(&cls, method)?.return_ty?)
            }
            // `maybeWidget!.moveTo(` — force-unwrap is transparent here.
            Expr::ForceUnwrap(inner) => self.receiver_class(&inner.value),
            _ => None,
        }
    }

    /// Param / return types of a function-typed local or parameter
    /// named `name` — the callback case, `f(...)`.
    fn local_fn(&self, name: &str) -> Option<(Vec<Type>, Type)> {
        let local = self.locals.iter().rev().find(|l| l.name == *name)?;
        match &local.ty {
            Type::Function { params, ret } => Some((params.clone(), (**ret).clone())),
            _ => None,
        }
    }

    /// Static type of a `local` with no annotation, from its
    /// initialiser. Only the shapes that actually matter for resolving
    /// a later `recv.method(` — constructor calls, calls returning a
    /// class, field reads, and aliases of another local.
    fn infer_local_ty(&self, init: &Expr) -> Type {
        let any = || Type::Named("any".into());
        match init {
            Expr::Self_ => self
                .enclosing_class
                .as_ref()
                .map(|c| Type::Named(c.clone()))
                .unwrap_or_else(any),
            Expr::Ident(n) => self
                .locals
                .iter()
                .rev()
                .find(|l| l.name == *n)
                .map(|l| l.ty.clone())
                .unwrap_or_else(any),
            Expr::Call { callee, .. } => match &callee.value {
                Expr::Ident(n) if with_classes(|r| r.contains_key(n)) => Type::Named(n.clone()),
                Expr::Member { obj, name } => self
                    .receiver_class(&obj.value)
                    .and_then(|c| lookup_method(&c, name))
                    .and_then(|sig| sig.return_ty)
                    .unwrap_or_else(any),
                _ => any(),
            },
            Expr::MethodCall { obj, method, .. } => self
                .receiver_class(&obj.value)
                .and_then(|c| lookup_method(&c, method))
                .and_then(|sig| sig.return_ty)
                .unwrap_or_else(any),
            Expr::Member { obj, name } => self
                .receiver_class(&obj.value)
                .and_then(|c| lookup_field_type(&c, name))
                .unwrap_or_else(any),
            Expr::ForceUnwrap(inner) => match self.infer_local_ty(&inner.value) {
                Type::Nullable(t) => *t,
                other => other,
            },
            _ => any(),
        }
    }
}

/// Head class name of a type, peeling `T?`. `None` for primitives and
/// structural types, which have no methods to look up.
fn class_of(ty: &Type) -> Option<String> {
    match ty {
        Type::Named(n) => Some(n.clone()),
        Type::Nullable(inner) => class_of(inner),
        _ => None,
    }
}

fn visit_arg(cx: &mut Cx, a: &CallArg) {
    match a {
        CallArg::Positional(e) | CallArg::Named { value: e, .. } => cx.visit_expr(e),
    }
}

fn build_arg_infos(args: &[CallArg]) -> Vec<CallArgInfo> {
    args.iter()
        .map(|a| match a {
            CallArg::Positional(e) => CallArgInfo {
                span: e.span.clone(),
                name: None,
                named_index: None,
            },
            CallArg::Named { name, value } => CallArgInfo {
                span: value.span.clone(),
                name: Some(name.clone()),
                named_index: None,
            },
        })
        .collect()
}

/// Best-effort reconstruction of the `(...)` arg-list region. Without
/// a dedicated paren-span on the AST we approximate it as `from the
/// end of the callee to the end of the call expression`.
fn args_span(
    callee_or_obj: &std::ops::Range<usize>,
    _args: &[CallArg],
    call_end: usize,
) -> std::ops::Range<usize> {
    callee_or_obj.end..call_end
}

/// `(...)` region of `obj.method(...)`. The callee here is the
/// *receiver*, so its span stops before `.method` — stepping over the
/// dot and the name lands on the `(` and gives the same boundaries a
/// free call gets. Falls back to the receiver's end if the arithmetic
/// overshoots (whitespace around the `.`, and so on).
fn method_args_span(
    obj: &std::ops::Range<usize>,
    method: &str,
    call_end: usize,
) -> std::ops::Range<usize> {
    let lparen = obj.end + 1 + method.len();
    if lparen < call_end {
        lparen..call_end
    } else {
        obj.end..call_end
    }
}

/// Is the cursor inside this call's argument list?
///
/// `span` runs from the `(` to one past the `)`, so both ends are
/// *outside* the arg list and the test is strict on both sides:
/// `f(|x)` and `f(x|)` are in, `f|(x)` and `f(x)|` are out.
///
/// The strictness is what makes nesting work. With `f(g())`, the two
/// boundary positions `f(g|())` and `f(g()|)` sit exactly on the inner
/// call's span ends; accepting them let the narrower inner call win
/// there and the popup showed `g`'s parameters while the caret was
/// plainly in `f`'s argument list.
fn contains(span: &std::ops::Range<usize>, offset: usize) -> bool {
    offset > span.start && offset < span.end
}

/// Pre-pass: collect every top-level `fn name(...)` declaration so the
/// signature-help walker can resolve free-call expressions whose target
/// is a user-defined function (not a class init, not a stdlib native).
fn collect_user_fns(module: &Module) -> HashMap<String, (Vec<Param>, Option<Type>)> {
    let mut out = HashMap::new();
    for s in &module.stmts {
        if let Stmt::Decl(d) = &s.value {
            if let Decl::Function {
                name,
                params,
                return_ty,
                ..
            } = &d.value
            {
                out.insert(name.clone(), (params.clone(), return_ty.clone()));
            }
        }
    }
    out
}

/// Pre-pass: collect every tuple-style enum variant's fields so
/// `Enum.Variant(...)` calls resolve like constructors.
fn collect_enum_variants(module: &Module) -> HashMap<(String, String), Vec<Param>> {
    let mut out = HashMap::new();
    for s in &module.stmts {
        if let Stmt::Decl(d) = &s.value
            && let Decl::Enum { name, variants, .. } = &d.value
        {
            for v in variants {
                if let saule_ast::EnumVariant::Tuple {
                    name: vname,
                    fields,
                } = &v.value
                {
                    out.insert((name.clone(), vname.clone()), fields.clone());
                }
            }
        }
    }
    out
}

/// Render one signature per call in the expression the cursor is in,
/// ordered by source position, with `active_signature` on the level the
/// cursor actually sits in.
///
/// Returning the whole expression rather than just the innermost call is
/// what makes the popup follow the caret in IntelliJ. LSP4IJ sets the
/// list of signatures *once*, when the popup opens
/// (`showParameterInfo` -> `setItemsToShow`); on a later cursor move
/// `updateParameterInfo` re-requests but only feeds the response into
/// `setUIComponentEnabled` / `setCurrentParameter`. So a response that
/// swaps in a different single signature is silently ignored, while one
/// that keeps the same list and moves `activeSignature` does update the
/// display.
///
/// Hence two rules the caret can't break, however it moves — typing
/// forward, arrowing back, or clicking straight onto some parameter:
///
/// 1. The list depends only on the *expression*, never on where in it
///    the cursor is, so every position yields the same rows in the same
///    order and only the selection differs.
/// 2. The list never grows between responses (see
///    [`reconcile_with_client`]), because the popup's rows are built
///    once and indexed by position thereafter.
fn help_from_module(module: &Module, offset: usize) -> Option<SignatureHelp> {
    let walk = |region: Option<std::ops::Range<usize>>| {
        let mut cx = Cx {
            offset,
            locals: Vec::new(),
            enclosing_class: None,
            user_fns: collect_user_fns(module),
            enum_variants: collect_enum_variants(module),
            region,
            hits: Vec::new(),
        };
        cx.visit_module(module);
        cx.hits
    };

    // Pass 1: the calls the cursor is actually inside. The widest of
    // them is the whole call expression the caret is somewhere within.
    let enclosing = walk(None);
    let outermost = enclosing
        .iter()
        .max_by_key(|h| h.args_span.end.saturating_sub(h.args_span.start))?
        .args_span
        .clone();

    // Pass 2: every call in that expression, cursor-containing or not.
    // Ordered by source position so an entry keeps its index as the
    // caret moves — the index is what the client selects by.
    let mut hits = walk(Some(outermost));
    hits.sort_by(|a, b| {
        a.args_span
            .start
            .cmp(&b.args_span.start)
            .then(a.args_span.end.cmp(&b.args_span.end))
    });
    // Runaway expression: fall back to just the enclosing chain rather
    // than filling the popup with every call on the line.
    if hits.len() > MAX_SIGNATURES {
        hits = enclosing;
        hits.sort_by(|a, b| {
            a.args_span
                .start
                .cmp(&b.args_span.start)
                .then(a.args_span.end.cmp(&b.args_span.end))
        });
    }

    // Resolve each level, remembering its span so the innermost one
    // containing the cursor can be selected afterwards. A level we
    // can't resolve is skipped, not fatal: `unknownFn(Color(` still
    // shows `Color`.
    let mut entries: Vec<(std::ops::Range<usize>, SignatureInformation)> = Vec::new();
    for hit in hits {
        let span = hit.args_span.clone();
        if let Some(help) = resolve_hit(hit, offset) {
            for sig in help.signatures {
                entries.push((span.clone(), sig));
            }
        }
    }
    // Drop levels that take no arguments *before* choosing the active
    // one. IntelliJ renders those as a bare `<no parameters>` row, and
    // dropping them first means a cursor inside `update(t.getDelta(|))`
    // selects the enclosing `update` — which still contains the cursor —
    // instead of falling back to an arbitrary row.
    entries.retain(|(_, sig)| sig.parameters.as_ref().is_some_and(|p| !p.is_empty()));
    if entries.is_empty() {
        return None;
    }

    // Active = narrowest surviving entry whose arg list holds the
    // cursor. Purely a function of the cursor against fixed spans, so
    // it lands correctly whether the caret got there by typing, by
    // arrowing back, or by a click straight onto a parameter.
    let active = entries
        .iter()
        .enumerate()
        .filter(|(_, (span, _))| contains(span, offset))
        .min_by_key(|(_, (span, _))| span.end.saturating_sub(span.start))
        .map(|(i, _)| i)
        .unwrap_or(0);

    let signatures: Vec<SignatureInformation> =
        entries.into_iter().map(|(_, sig)| sig).collect();
    let active_parameter = signatures[active].active_parameter;
    Some(SignatureHelp {
        signatures,
        active_signature: Some(active as u32),
        active_parameter,
    })
}

/// Upper bound on signatures in one response. Keeps a dense expression
/// from producing an unreadable popup.
const MAX_SIGNATURES: usize = 8;

/// Reconcile a fresh response against what the client already shows.
///
/// IntelliJ creates one UI row per signature when the popup opens and
/// never rebuilds them, but LSP4IJ's `updateParameterInfo` indexes those
/// rows by the position of every *later* response
/// (`setUIComponentEnabled(i, …)`). A response with more signatures than
/// the popup was opened with therefore throws ArrayIndexOutOfBounds
/// inside the IDE.
///
/// So the list must never grow on a retrigger. When the fresh chain is
/// longer, keep the client's own list and just move the selection to the
/// matching entry.
pub(super) fn reconcile_with_client(
    fresh: SignatureHelp,
    prev: &SignatureHelp,
) -> SignatureHelp {
    if fresh.signatures.len() <= prev.signatures.len() {
        return fresh;
    }
    let active_label = fresh
        .active_signature
        .and_then(|i| fresh.signatures.get(i as usize))
        .map(|s| s.label.clone());
    let idx = active_label
        .and_then(|l| prev.signatures.iter().position(|s| s.label == l))
        .unwrap_or(0);
    let mut out = prev.clone();
    out.active_signature = Some(idx as u32);
    out.active_parameter = fresh.active_parameter;
    if let Some(s) = out.signatures.get_mut(idx) {
        s.active_parameter = fresh.active_parameter;
    }
    out
}

/// Textual-fallback variant of [`collect_user_fns`]: re-lexes and
/// re-parses the source defensively to recover top-level `fn name`
/// declarations even when the document as a whole fails to parse
/// (the failing tokens may be elsewhere in the file).
fn lookup_user_fn_textual(source: &str, name: &str) -> Option<(Vec<Param>, Option<Type>)> {
    // First try parsing the source as-is.
    if let Ok(tokens) = saule_lexer::Lexer::new(source).tokenize() {
        if let Ok(module) = saule_parser::parse(tokens) {
            if let Some(found) = collect_user_fns(&module).remove(name) {
                return Some(found);
            }
        }
    }
    // Mid-keystroke: the buffer has an unclosed `(` somewhere. Try
    // synthesising a closed buffer by appending `) end` enough times
    // to balance any open call/block. This is crude but cheap and
    // recovers the `fn name(...) ... end` declarations the user
    // already finished typing.
    for suffix in [") end", ") end\nend", ") end\nend\nend"] {
        let patched = format!("{source}{suffix}");
        if let Ok(tokens) = saule_lexer::Lexer::new(&patched).tokenize() {
            if let Ok(module) = saule_parser::parse(tokens) {
                if let Some(found) = collect_user_fns(&module).remove(name) {
                    return Some(found);
                }
            }
        }
    }
    None
}

/// Textual-fallback variant of [`build_help_user_fn`]: reuses the
/// AST path's renderer but feeds it a comma-derived `active` index
/// instead of arg-span containment.
fn build_help_user_fn_simple(
    name: &str,
    params: &[Param],
    return_ty: &Option<Type>,
    active: usize,
) -> Option<SignatureHelp> {
    let total = params.len().max(1);
    let active = active.min(total - 1);
    let mut help = build_help_user_fn(name, params, return_ty, &[], 0)?;
    if let Some(s) = help.signatures.first_mut() {
        s.active_parameter = Some(active as u32);
    }
    help.active_parameter = Some(active as u32);
    Some(help)
}

#[cfg(test)]
mod tests {
    //! Signature help tests. Bypasses `Backend` by replicating the
    //! handler's pure inner logic (parse + analyse + walk + dispatch
    //! to `build_help`) against an in-memory source string.

    use super::*;
    use std::sync::Once;

    fn init_stdlib() {
        static ONCE: Once = Once::new();
        ONCE.call_once(saule_interpreter::init);
    }

    fn help(src: &str, cursor_at: &str, offset_into: usize) -> Option<SignatureHelp> {
        init_stdlib();
        let offset = src.find(cursor_at).expect("needle") + offset_into;
        let tokens = saule_lexer::Lexer::new(src).tokenize().expect("lex");
        let module = saule_parser::parse(tokens).expect("parse");
        let _ = saule_semantic::analyze(&module);
        help_from_module(&module, offset)
    }

    #[test]
    fn signature_for_class_constructor_first_arg() {
        let src = "class Point\n  x: integer = 0\n  y: integer = 0\n  fn init(x: integer, y: integer)\n    self.x = x\n    self.y = y\n  end\nend\n\nfn main()\n  local p = Point(1, 2)\nend\n";
        // Cursor right after the `(`
        let h = help(src, "Point(1", 6).expect("help");
        let sig = &h.signatures[0];
        assert!(sig.label.starts_with("Point("), "label={}", sig.label);
        assert!(sig.label.contains("x: integer"), "label={}", sig.label);
        assert!(sig.label.contains("y: integer"), "label={}", sig.label);
        assert_eq!(h.active_parameter, Some(0));
    }

    #[test]
    fn signature_active_param_advances_after_comma() {
        let src = "class Point\n  fn init(x: integer, y: integer)\n  end\nend\n\nfn main()\n  local p = Point(1, 2)\nend\n";
        // Cursor on the `2` (second arg)
        let h = help(src, "1, 2", 3).expect("help");
        assert_eq!(h.active_parameter, Some(1));
    }

    #[test]
    fn signature_for_method_call() {
        let src = "class Foo\n  fn bar(n: integer) -> integer\n    return n\n  end\nend\n\nfn main()\n  local f: Foo = Foo()\n  local r = f.bar(7)\nend\n";
        let h = help(src, "bar(7", 4).expect("help");
        let sig = &h.signatures[0];
        assert!(sig.label.starts_with("bar("), "label={}", sig.label);
        assert!(sig.label.contains("n: integer"), "label={}", sig.label);
    }

    #[test]
    fn no_signature_outside_call() {
        let src = "fn main()\n  local x = 1\nend\n";
        assert!(help(src, "x = 1", 0).is_none());
    }

    // ── textual fallback (mid-keystroke recovery) ─────────────────

    /// Drive the textual fallback directly, without parsing. We still
    /// need the registries seeded so `with_classes` / `lookup_method`
    /// work — analyse a *prelude* containing the class def so the
    /// in-progress snippet doesn't have to be syntactically valid.
    fn fallback_help(prelude: &str, snippet: &str, cursor_at_end: usize) -> Option<SignatureHelp> {
        init_stdlib();
        let tokens = saule_lexer::Lexer::new(prelude).tokenize().expect("lex");
        let module = saule_parser::parse(tokens).expect("parse");
        let _ = saule_semantic::analyze(&module);
        // Pretend the snippet sits right after the prelude in the
        // same buffer; the fallback only reads `source[..offset]`.
        let combined = format!("{prelude}{snippet}");
        let offset = combined.len() - cursor_at_end;
        textual_fallback(&combined, offset)
    }

    #[test]
    fn fallback_keeps_help_after_first_comma() {
        // User has typed `Point(1, ` with no closing paren — parser
        // would fail, but textual fallback should still surface the
        // sig with `active_parameter = 1`.
        let prelude = "class Point\n  fn init(x: integer, y: integer)\n  end\nend\n";
        let snippet = "Point(1, ";
        let h = fallback_help(prelude, snippet, 0).expect("fallback help");
        let sig = &h.signatures[0];
        assert!(sig.label.starts_with("Point("), "label={}", sig.label);
        assert_eq!(h.active_parameter, Some(1));
    }

    #[test]
    fn fallback_active_param_zero_right_after_open_paren() {
        let prelude = "class Point\n  fn init(x: integer, y: integer)\n  end\nend\n";
        let snippet = "Point(";
        let h = fallback_help(prelude, snippet, 0).expect("fallback help");
        assert_eq!(h.active_parameter, Some(0));
    }

    #[test]
    fn fallback_resolves_method_call_via_dot() {
        let prelude = "class Foo\n  fn bar(n: integer, m: integer) -> integer\n    return n\n  end\nend\n";
        let snippet = "Foo().bar(1, ";
        let h = fallback_help(prelude, snippet, 0).expect("fallback help");
        assert!(h.signatures[0].label.starts_with("bar("));
        assert_eq!(h.active_parameter, Some(1));
    }

    #[test]
    fn fallback_clamps_active_param_to_arity() {
        let prelude = "class P\n  fn init(x: integer)\n  end\nend\n";
        let snippet = "P(1, 2, 3, ";
        let h = fallback_help(prelude, snippet, 0).expect("fallback help");
        // Only one param exists — clamp instead of going out of range.
        assert_eq!(h.active_parameter, Some(0));
    }

    // ── stdlib (native) signature help ────────────────────────────

    #[test]
    fn signature_for_stdlib_module_member() {
        let src = "fn main()\n  local n = Math.floor(3.14)\nend\n";
        let h = help(src, "floor(3", 6).expect("help");
        let sig = &h.signatures[0];
        assert!(sig.label.starts_with("floor("), "label={}", sig.label);
        assert_eq!(h.active_parameter, Some(0));
    }

    #[test]
    fn fallback_signature_for_stdlib_member_mid_typing() {
        // No closing paren → parser would fail; textual fallback
        // should still resolve `Math.floor(`.
        let prelude = "";
        let snippet = "  local n = Math.floor(";
        let h = fallback_help(prelude, snippet, 0).expect("fallback help");
        let sig = &h.signatures[0];
        assert!(sig.label.starts_with("floor("), "label={}", sig.label);
        assert_eq!(h.active_parameter, Some(0));
    }

    #[test]
    fn fallback_signature_for_stdlib_two_arg_after_comma() {
        // `Math.atan` is registered with 2 params (`(n, n?)`).
        let prelude = "";
        let snippet = "  local r = Math.atan(1, ";
        let h = fallback_help(prelude, snippet, 0).expect("fallback help");
        assert!(h.signatures[0].label.starts_with("atan("));
        assert_eq!(h.active_parameter, Some(1));
    }

    // ── user-defined free top-level functions ─────────────────────

    #[test]
    fn signature_for_free_top_level_user_fn() {
        let src = "fn add(x: integer, y: integer) -> integer\n  return x + y\nend\n\nfn main()\n  local r = add(1, 2)\nend\n";
        let h = help(src, "add(1", 4).expect("help");
        let label = &h.signatures[0].label;
        assert!(label.starts_with("add("), "label={label}");
        assert!(label.contains("x: integer"), "label={label}");
        assert!(label.contains("y: integer"), "label={label}");
        assert!(label.contains("-> integer"), "label={label}");
        assert_eq!(h.active_parameter, Some(0));
    }

    #[test]
    fn signature_for_free_user_fn_advances_active_param() {
        let src = "fn add(x: integer, y: integer) -> integer\n  return x + y\nend\n\nfn main()\n  local r = add(1, 2)\nend\n";
        // Position cursor between `1, ` and `2)` — second arg.
        let h = help(src, "1, 2", 3).expect("help");
        assert_eq!(h.active_parameter, Some(1));
    }

    #[test]
    fn fallback_signature_for_free_user_fn() {
        // Mid-keystroke: closing paren missing on the call site, but
        // the `fn add` declaration parses fine on its own.
        let prelude = "fn add(x: integer, y: integer) -> integer\n  return x + y\nend";
        let snippet = "\nfn main()\n  local r = add(";
        let h = fallback_help(prelude, snippet, 0).expect("fallback help");
        let label = &h.signatures[0].label;
        assert!(label.starts_with("add("), "label={label}");
        assert!(label.contains("x: integer"), "label={label}");
        assert_eq!(h.active_parameter, Some(0));
    }

    // ── better native param names ─────────────────────────────────

    #[test]
    fn signature_for_stdlib_uses_real_param_names() {
        // `Math.floor(n: number) -> integer` — names should come from
        // the static stdlib table, not synthesised `arg0`.
        let src = "fn main()\n  local n = Math.floor(3.14)\nend\n";
        let h = help(src, "floor(3", 6).expect("help");
        let label = &h.signatures[0].label;
        assert!(label.contains("n: "), "expected `n:` in {label}");
        assert!(!label.contains("arg0"), "should not contain arg0: {label}");
    }

    #[test]
    fn signature_for_stdlib_string_find_uses_real_param_names() {
        let src = "fn main()\n  local i, j = String.find(\"hello\", \"l\")\nend\n";
        let h = help(src, "find(\"", 5).expect("help");
        let label = &h.signatures[0].label;
        assert!(label.contains("s: "), "expected `s:` in {label}");
        assert!(label.contains("pattern: "), "expected `pattern:` in {label}");
        assert!(label.contains("init"), "expected `init` in {label}");
    }

    // ── `self.super(...)` → parent constructor ────────────────────

    #[test]
    fn signature_for_self_super_shows_parent_init() {
        let src = "class Base
  fn init(x: integer, y: integer)
  end
end

class Child extends Base
  fn init()
    self.super(1, 2)
  end
end
";
        let h = help(src, "self.super(1", "self.super(".len()).expect("help");
        let label = &h.signatures[0].label;
        assert!(label.starts_with("Base.init("), "label={label}");
        assert!(label.contains("x: integer"), "label={label}");
        assert_eq!(h.active_parameter, Some(0));
    }

    /// Mid-keystroke `self.super(1, ` with no closing paren. The
    /// registries still hold the last good parse (both classes), which
    /// is what the enclosing-class text scan is resolved against.
    #[test]
    fn fallback_signature_for_self_super_mid_typing() {
        let prelude = "class Base
  fn init(x: integer, y: integer)
  end
end

class Child extends Base
  fn init()
  end
end
";
        let snippet = "class Child extends Base
  fn init()
    self.super(1, ";
        let h = fallback_help(prelude, snippet, 0).expect("fallback help");
        assert!(h.signatures[0].label.starts_with("Base.init("));
        assert_eq!(h.active_parameter, Some(1));
    }


    // ── coverage matrix: every call form the language has ─────────
    //
    // One shared fixture, two passes: the finished-code path (parens
    // closed) and the mid-keystroke path (the user has typed `(` and
    // nothing after it yet). A form missing from here is a form where
    // the parameter popup silently does nothing.

    const FIXTURE: &str = "\
class Color
  fn apply(alpha: float)
  end
end

class Widget
  fn init(x: float, y: float)
  end
  fn moveTo(x: float, y: float)
  end
  static fn make(n: integer) -> Widget
    return Widget(0.0, 0.0)
  end
end

enum Shape
  Circle(r: float)
  Square
end

fn add(x: integer, y: integer) -> integer
  return x + y
end
";

    /// Byte offset just past `needle`, searched after the fixture so
    /// call sites are found rather than the declarations above.
    fn call_offset(src: &str, needle: &str) -> usize {
        let start = FIXTURE.len();
        src[start..]
            .find(needle)
            .map(|i| start + i + needle.len())
            .unwrap_or_else(|| panic!("needle {needle:?} not found"))
    }

    /// The signature the user actually sees highlighted. The list is
    /// ordered by source position and can carry several nesting levels,
    /// so `active_signature` — not index 0 — is the current one.
    fn label(h: Option<SignatureHelp>, case: &str) -> String {
        let h = h.unwrap_or_else(|| panic!("no signature help for {case}"));
        let i = h.active_signature.unwrap_or(0) as usize;
        h.signatures[i].label.clone()
    }

    fn active_label(h: &SignatureHelp) -> &str {
        &h.signatures[h.active_signature.unwrap_or(0) as usize].label
    }

    /// Finished code: the call's parens are closed, so the document
    /// parses and the AST walker resolves the callee.
    #[test]
    fn signature_help_covers_every_call_form() {
        init_stdlib();
        let cases: Vec<(&str, &str, &str, &str)> = vec![
            ("free fn", "  local r = add(1, 2)\n", "add(1", "add("),
            ("constructor", "  local w = Widget(1.0, 2.0)\n", "Widget(1.0", "Widget("),
            (
                "method on annotated local",
                "  local w: Widget = Widget(1.0, 2.0)\n  w.moveTo(3.0, 4.0)\n",
                "moveTo(3.0",
                "moveTo(",
            ),
            (
                "method on inferred local",
                "  local w = Widget(1.0, 2.0)\n  w.moveTo(3.0, 4.0)\n",
                "moveTo(3.0",
                "moveTo(",
            ),
            ("static method", "  Widget.make(1)\n", "make(1", "make("),
            (
                "method on constructor result",
                "  Widget(1.0, 2.0).moveTo(3.0, 4.0)\n",
                "moveTo(3.0",
                "moveTo(",
            ),
            (
                "method on call result",
                "  Widget.make(1).moveTo(3.0, 4.0)\n",
                "moveTo(3.0",
                "moveTo(",
            ),
            ("stdlib module fn", "  local n = Math.floor(3.14)\n", "floor(3.14", "floor("),
            ("bare native", "  println(1)\n", "println(1", "println("),
            (
                "enum tuple variant",
                "  local s = Shape.Circle(1.0)\n",
                "Circle(1.0",
                "Shape.Circle(",
            ),
            (
                "function-typed local",
                "  local f: fn(integer) -> integer = fn(n: integer) -> integer return n end\n  local z = f(1)\n",
                // Inside the args — a needle ending in `)` would put the
                // caret past the close paren, which is outside the call.
                "f(1",
                "f(",
            ),
            ("nested inner call", "  local r = add(add(1, 2), 3)\n", "add(1", "add("),
            ("named argument", "  local w = Widget(x: 1.0, y: 2.0)\n", "Widget(x: 1.0", "Widget("),
            ("table-literal argument", "  local t = add({1, 2}, 3)\n", "add({1", "add("),
            // The piped value fills slot 0, so only `y` is left.
            ("pipeline stage", "  local n = when(4):add(3)\n", "add(3", "add(y: integer)"),
        ];
        for (case, body, needle, expected) in cases {
            let src = format!("{FIXTURE}\nfn probe()\n{body}end\n");
            let got = label(help_at(&src, call_offset(&src, needle)), case);
            assert!(got.starts_with(expected), "{case}: got {got:?}");
        }
    }

    /// Call forms that only exist inside a class body.
    #[test]
    fn signature_help_covers_in_class_call_forms() {
        init_stdlib();
        let src = format!(
            "{FIXTURE}
class Probe extends Widget
  tint: Color
  fn init()
    self.super(1.0, 2.0)
  end
  fn go(w: Widget)
    self.go2(1)
    w.moveTo(3.0, 4.0)
    self.tint.apply(1.0)
    go2(1)
    Probe.stat(1)
  end
  fn go2(n: integer)
  end
  static fn stat(n: integer)
  end
end
"
        );
        for (case, needle, expected) in [
            ("self.super", "self.super(1.0", "Widget.init("),
            ("self.method", "self.go2(1", "go2("),
            ("method on parameter", "w.moveTo(3.0", "moveTo("),
            ("method on field", "self.tint.apply(1.0", "apply("),
            ("bare sibling method", "\n    go2(1", "go2("),
            ("own static via class name", "Probe.stat(1", "stat("),
        ] {
            let got = label(help_at(&src, call_offset(&src, needle)), case);
            assert!(got.starts_with(expected), "{case}: got {got:?}");
        }
    }

    /// Mid-keystroke: the user has typed `(` (or `(arg, `) and the
    /// buffer doesn't parse yet. This is the path that has to keep the
    /// popup alive while the arguments are being typed.
    #[test]
    fn signature_help_survives_unclosed_call() {
        init_stdlib();
        for (case, snippet, expected, active) in [
            ("free fn", "fn probe()\n  local r = add(", "add(", 0),
            ("free fn second arg", "fn probe()\n  local r = add(1, ", "add(", 1),
            ("constructor", "fn probe()\n  local w = Widget(", "Widget(", 0),
            (
                "method on annotated local",
                "fn probe()\n  local w: Widget = Widget(1.0, 2.0)\n  w.moveTo(",
                "moveTo(",
                0,
            ),
            (
                "method on inferred local",
                "fn probe()\n  local w = Widget(1.0, 2.0)\n  w.moveTo(",
                "moveTo(",
                0,
            ),
            ("static method", "fn probe()\n  Widget.make(", "make(", 0),
            (
                "method on constructor result",
                "fn probe()\n  Widget(1.0, 2.0).moveTo(",
                "moveTo(",
                0,
            ),
            ("stdlib module fn", "fn probe()\n  local n = Math.floor(", "floor(", 0),
            ("bare native", "fn probe()\n  println(", "println(", 0),
            ("enum tuple variant", "fn probe()\n  local s = Shape.Circle(", "Shape.Circle(", 0),
            (
                "self.method",
                "class Probe extends Widget\n  fn go()\n    self.moveTo(",
                "moveTo(",
                0,
            ),
            (
                "self.super",
                "class Probe extends Widget\n  fn init()\n    self.super(",
                "Widget.init(",
                0,
            ),
            (
                "method on field",
                "class Probe extends Widget\n  tint: Color\n  fn go()\n    self.tint.apply(",
                "apply(",
                0,
            ),
            ("nested inner call", "fn probe()\n  local r = add(add(", "add(", 0),
        ] {
            let src = format!("{FIXTURE}\n{snippet}");
            let offset = src.len();
            let h = help_mid_keystroke(&src, offset)
                .unwrap_or_else(|| panic!("no signature help for {case}"));
            assert_eq!(h.active_parameter, Some(active), "{case}");
            let got = active_label(&h);
            assert!(got.starts_with(expected), "{case}: got {got:?}");
        }
    }

    /// A `name: value` argument highlights the slot its key names,
    /// not the positional slot it happens to sit in.
    #[test]
    fn named_argument_highlights_its_own_slot() {
        init_stdlib();
        let src = format!("{FIXTURE}
fn probe()
  local w = Widget(y: 2.0)
end
");
        let h = help_at(&src, call_offset(&src, "Widget(y: 2")).expect("help");
        assert_eq!(h.active_parameter, Some(1));
    }

    /// The realistic shape of the same thing: the half-typed call sits
    /// in the *middle* of a file, with well-formed code after it. The
    /// repair has to close the call at the cursor — appending past the
    /// trailing `end`s closes nothing and the popup stays empty.
    #[test]
    fn signature_help_survives_unclosed_call_mid_file() {
        init_stdlib();
        for (case, head, tail, expected, active) in [
            (
                "method on local",
                "fn probe()\n  local w: Widget = Widget(1.0, 2.0)\n  w.moveTo(",
                "\nend\n",
                "moveTo(",
                0,
            ),
            (
                "constructor second arg",
                "fn probe()\n  local w = Widget(1.0, ",
                "\nend\n\nfn after()\n  println(1)\nend\n",
                "Widget(",
                1,
            ),
            (
                "self.super",
                "class Probe extends Widget\n  fn init()\n    self.super(",
                "\n  end\nend\n",
                "Widget.init(",
                0,
            ),
            (
                "nested call",
                "fn probe()\n  local r = add(add(",
                "\nend\n",
                "add(",
                0,
            ),
            (
                "method on field",
                "class Probe extends Widget\n  tint: Color\n  fn go()\n    self.tint.apply(",
                "\n  end\nend\n",
                "apply(",
                0,
            ),
        ] {
            let src = format!("{FIXTURE}\n{head}{tail}");
            let offset = FIXTURE.len() + 1 + head.len();
            let h = help_mid_keystroke(&src, offset)
                .unwrap_or_else(|| panic!("no signature help for {case}"));
            assert_eq!(h.active_parameter, Some(active), "{case}");
            let got = active_label(&h);
            assert!(got.starts_with(expected), "{case}: got {got:?}");
        }
    }

    /// Every parameter range must be a valid slice of the label
    /// measured in UTF-16 code units — that's what the client indexes
    /// with. A defaulted parameter renders `" = …"`, whose `…` is three
    /// bytes but one code unit, so byte offsets would run past the end
    /// of the label and the client would slice out of bounds.
    #[test]
    fn parameter_offsets_are_utf16_code_units() {
        init_stdlib();
        let src = "class Color
  fn init(r: float = 1.0, g: float = 1.0, b: float = 1.0)
  end
end

fn probe()
  local c = Color(1.0)
end
";
        let h = help_at(&src, src.rfind("Color(1.0").unwrap() + "Color(".len()).expect("help");
        let sig = &h.signatures[0];
        let units: Vec<u16> = sig.label.encode_utf16().collect();
        assert!(sig.label.contains(" = …"), "label={}", sig.label);
        let params = sig.parameters.as_ref().expect("parameters");
        assert_eq!(params.len(), 3);
        for (i, p) in params.iter().enumerate() {
            let ParameterLabel::LabelOffsets([s, e]) = p.label else {
                panic!("expected offset labels");
            };
            assert!(
                s <= e && (e as usize) <= units.len(),
                "param {i} range [{s}, {e}) out of bounds for label of {} units: {}",
                units.len(),
                sig.label
            );
            let slice = String::from_utf16(&units[s as usize..e as usize]).expect("utf16");
            assert!(slice.starts_with(["r", "g", "b"][i]), "param {i} sliced to {slice:?}");
        }
    }

    /// Nested calls: which signature is showing depends on the caret
    /// position, and the boundaries have to be exact. `f(g())` has
    /// four interesting spots — the caret is in `f`'s arg list
    /// everywhere except strictly inside `g`'s parens.
    #[test]
    fn nested_call_switches_signature_at_the_paren_boundaries() {
        init_stdlib();
        let src = "class Color
  fn init(r: float = 1.0, g: float = 1.0)
  end
end

class View
  fn setBackground(color: Color)
  end
end

class PanelView extends View
  fn init()
    setBackground(Color())
  end
end
";
        let call = src.find("setBackground(Color())").expect("call site");
        // setBackground(Color())
        // ^0           ^13    ^20
        for (case, at, expected) in [
            ("before the argument", call + "setBackground(".len(), "setBackground("),
            ("on the callee name", call + "setBackground(Color".len(), "setBackground("),
            ("inside the inner parens", call + "setBackground(Color(".len(), "Color("),
            ("after the inner call", call + "setBackground(Color()".len(), "setBackground("),
        ] {
            let got = label(help_at(src, at), case);
            assert!(got.starts_with(expected), "{case}: expected {expected:?}, got {got:?}");
        }
        // Past the outer `)` the popup belongs to nobody.
        assert!(
            help_at(src, call + "setBackground(Color())".len()).is_none(),
            "caret past the outer close paren should not report a signature"
        );
    }

    /// A nested call reports one signature per enclosing level,
    /// innermost first, with `active_signature` on the innermost. The
    /// whole chain has to be in the *first* response: LSP4IJ fixes the
    /// signature list when the popup opens and afterwards only moves
    /// `activeSignature`, so a level missing here can never be shown.
    #[test]
    fn nested_call_reports_the_whole_enclosing_chain() {
        init_stdlib();
        let src = "class Color
  fn init(r: float = 1.0, g: float = 1.0)
  end
end

class View
  fn setBackground(color: Color?)
  end
end

class PanelView extends View
  fn init()
    self.setBackground(Color(10f, 10f))
  end
end
";
        let call = src.find("self.setBackground(Color").expect("call site");
        let outer = help_at(src, call + "self.setBackground(".len()).expect("help");
        let inner = help_at(src, call + "self.setBackground(Color(".len()).expect("help");

        // The list is identical from both positions — same entries, same
        // order. Only `active_signature` moves. This is the property the
        // IDE depends on: it builds its rows once, from whichever
        // response opened the popup, and thereafter only re-selects.
        let labels = |h: &SignatureHelp| -> Vec<String> {
            h.signatures.iter().map(|s| s.label.clone()).collect()
        };
        assert_eq!(labels(&outer), labels(&inner), "signature list must be stable");
        assert_eq!(outer.signatures.len(), 2, "got {:?}", labels(&outer));
        assert!(labels(&outer)[0].starts_with("setBackground("), "source order");
        assert!(labels(&outer)[1].starts_with("Color("), "source order");

        assert_eq!(outer.active_signature, Some(0));
        assert!(active_label(&outer).starts_with("setBackground("));
        assert_eq!(inner.active_signature, Some(1));
        assert!(active_label(&inner).starts_with("Color("));
    }

    /// The response must never carry more signatures than the popup was
    /// opened with: IntelliJ creates its rows once and LSP4IJ indexes
    /// them by the position of every later response, so a longer list
    /// throws ArrayIndexOutOfBounds inside the IDE.
    #[test]
    fn retrigger_never_grows_the_signature_list() {
        let sig = |label: &str| SignatureInformation {
            label: label.to_string(),
            documentation: None,
            parameters: Some(Vec::new()),
            active_parameter: Some(0),
        };
        let prev = SignatureHelp {
            signatures: vec![sig("setBackground(color: Color?)")],
            active_signature: Some(0),
            active_parameter: Some(0),
        };
        let fresh = SignatureHelp {
            signatures: vec![sig("setBackground(color: Color?)"), sig("Color(r: float)")],
            active_signature: Some(1),
            active_parameter: Some(0),
        };
        let out = reconcile_with_client(fresh, &prev);
        assert_eq!(out.signatures.len(), 1, "must not exceed the client's row count");

        // Shrinking or staying level is safe, so the fresh answer wins.
        let fresh = SignatureHelp {
            signatures: vec![sig("Color(r: float)")],
            active_signature: Some(0),
            active_parameter: Some(1),
        };
        let out = reconcile_with_client(fresh, &prev);
        assert_eq!(out.signatures.len(), 1);
        assert!(out.signatures[0].label.starts_with("Color("));
    }

    /// A call that takes no arguments is never worth a popup row —
    /// IntelliJ spells it `<no parameters>`. It's dropped from the
    /// chain, and if that empties the chain there's no popup at all.
    #[test]
    fn parameterless_calls_are_not_offered() {
        init_stdlib();
        let src = "class Timer
  fn getDelta() -> float
    return 0.0
  end
end

class Root
  fn update(dt: float)
  end
end

fn probe()
  local t = Timer()
  local root = Root()
  root.update(t.getDelta())
end
";
        let call = src.find("root.update(t.getDelta())").expect("call site");

        // Caret inside the parameterless `getDelta()`: that level is
        // dropped, leaving the enclosing `update` as the only row.
        let h = help_at(src, call + "root.update(t.getDelta(".len()).expect("help");
        let labels: Vec<&str> = h.signatures.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels.len(), 1, "getDelta should be filtered out: {labels:?}");
        assert!(labels[0].starts_with("update("), "{labels:?}");
        assert!(active_label(&h).starts_with("update("));

        // Caret inside the parameterless call with nothing enclosing it:
        // no popup at all.
        let src = "class Timer
  fn getDelta() -> float
    return 0.0
  end
end

fn probe()
  local t = Timer()
  local d = t.getDelta()
end
";
        let at = src.find("t.getDelta()").expect("call") + "t.getDelta(".len();
        assert!(help_at(src, at).is_none(), "parameterless call should not open a popup");
    }

    /// The caret is not required to arrive by typing. It can be arrowed
    /// backwards, arrowed forwards again, or clicked straight onto any
    /// argument — every position has to resolve on its own terms.
    ///
    /// Two invariants, checked at every offset in the expression: the
    /// signature *list* never changes (the IDE builds its rows once), and
    /// the *selection* always names the call the caret is really in.
    #[test]
    fn caret_can_land_anywhere_in_a_nested_call() {
        init_stdlib();
        let src = "class Color
  static fn rgb(r: integer, g: integer, b: integer) -> Color?
    return nil
  end
end

class Root
  fn setBackground(color: Color?)
  end
end

fn probe()
  local root = Root()
  root.setBackground(Color.rgb(38, 38, 38))
end
";
        let text = "root.setBackground(Color.rgb(38, 38, 38))";
        let call = src.find(text).expect("call site");
        let outer_open = "root.setBackground(".len();
        let inner_open = "root.setBackground(Color.rgb(".len();
        let inner_close = text.rfind("))").expect("closers");

        // 1. The list is the same at every live position.
        let mut seen: Option<Vec<String>> = None;
        for i in outer_open..=inner_close + 1 {
            let h = help_at(src, call + i).unwrap_or_else(|| panic!("no help at +{i}"));
            let labels: Vec<String> = h.signatures.iter().map(|s| s.label.clone()).collect();
            match &seen {
                None => seen = Some(labels),
                Some(prev) => assert_eq!(prev, &labels, "list changed at +{i}"),
            }
        }
        let labels = seen.expect("swept at least one offset");
        assert_eq!(labels.len(), 2, "{labels:?}");
        assert!(labels[0].starts_with("setBackground("), "source order: {labels:?}");
        assert!(labels[1].starts_with("rgb("), "source order: {labels:?}");

        // 2. The selection tracks the caret, in both directions.
        for i in outer_open..inner_open {
            let h = help_at(src, call + i).expect("help");
            assert!(active_label(&h).starts_with("setBackground("), "at +{i}");
        }
        for i in inner_open..=inner_close {
            let h = help_at(src, call + i).expect("help");
            assert!(active_label(&h).starts_with("rgb("), "at +{i}");
        }
        // Between the two closing parens we're back in the outer call.
        let h = help_at(src, call + inner_close + 1).expect("help");
        assert!(active_label(&h).starts_with("setBackground("));
        // Past the whole expression there's nothing to show.
        assert!(help_at(src, call + text.len()).is_none());

        // 3. Clicking directly onto an argument selects that slot —
        //    including jumping backwards from the third to the first.
        for (slot, delta) in [(0u32, 0usize), (1, 4), (2, 8), (0, 0)] {
            let h = help_at(src, call + inner_open + delta).expect("help");
            assert!(active_label(&h).starts_with("rgb("), "slot {slot}");
            assert_eq!(h.active_parameter, Some(slot), "clicked slot {slot}");
        }
    }

    #[test]
    fn unclosed_delimiters_ignores_strings_and_comments() {
        assert_eq!(unclosed_delimiters("add(1, 2)"), "");
        assert_eq!(unclosed_delimiters("add(f("), "))");
        assert_eq!(unclosed_delimiters("f(\"a )( b\""), ")");
        assert_eq!(unclosed_delimiters("f( -- a ) comment\n"), ")");
        assert_eq!(unclosed_delimiters("f( --[[ ) ]] "), ")");
        assert_eq!(unclosed_delimiters("t = {1, [2] = f("), ")}");
    }

    /// Like `help` but takes an absolute byte offset.
    fn help_at(src: &str, offset: usize) -> Option<SignatureHelp> {
        init_stdlib();
        let tokens = saule_lexer::Lexer::new(src).tokenize().expect("lex");
        let module = saule_parser::parse(tokens).expect("parse");
        let _ = saule_semantic::analyze(&module);
        help_from_module(&module, offset)
    }

    /// Mirrors `signature_help_at`'s dispatch for a buffer that may not
    /// parse: real AST first, repaired AST second, text scan last.
    fn help_mid_keystroke(src: &str, offset: usize) -> Option<SignatureHelp> {
        init_stdlib();
        if let Ok(tokens) = saule_lexer::Lexer::new(src).tokenize()
            && let Ok(module) = saule_parser::parse(tokens)
        {
            let _ = saule_semantic::analyze(&module);
            return help_from_module(&module, offset);
        }
        if let Some(module) = repair_parse(src, offset) {
            let _ = saule_semantic::analyze(&module);
            if let Some(h) = help_from_module(&module, offset) {
                return Some(h);
            }
        }
        textual_fallback(src, offset)
    }
}
