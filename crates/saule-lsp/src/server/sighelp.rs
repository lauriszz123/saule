//! Signature help — shows the active call's signature with the
//! parameter under the cursor highlighted. Triggered by `(`, `,`, and
//! `:` (for named-argument keys).
//!
//! Implementation strategy:
//!
//! 1. Lex / parse the document.
//! 2. Walk the AST to find the smallest `Expr::Call` /
//!    `Expr::MethodCall` whose argument span (the `(...)` parens
//!    region) contains the cursor.
//! 3. Resolve the callee to a parameter list via the class registry
//!    (constructor, method, or sibling-static dispatch).
//! 4. Compute `active_parameter` from how many `,`-separated
//!    arguments precede the cursor.
//! 5. Render `name(p1: T1, p2: T2, ...)` with the active param
//!    highlighted via `parameters[*]` ranges into the label string.

use saule_ast::{
    CallArg, ClassMember, Decl, Expr, LambdaBody, MatchBody, Method, Module, Param, Spanned,
    Stmt, TableEntry, Type,
};
use saule_semantic::{lookup_method, with_classes};
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

            let mut cx = Cx {
                offset,
                locals: Vec::new(),
                enclosing_class: None,
                user_fns: collect_user_fns(module),
                best: None,
            };
            cx.visit_module(module);
            if let Some(hit) = cx.best {
                if let Some(help) = resolve_hit(hit, offset) {
                    return Some(help);
                }
            }
        }

        // Parser failed *or* the AST walker couldn't pin down the call
        // (typical mid-keystroke: `Foo(1, ` with no trailing `)`).
        // Scan the raw source to locate the innermost unmatched `(`
        // before the cursor and resolve the call by name.
        textual_fallback(&source, offset)
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
        let start = label.len() as u32;
        let pname = names.get(i).map(|s| s.as_str()).unwrap_or("value");
        label.push_str(pname);
        label.push_str(": ");
        label.push_str(&render_type(ty));
        let end = label.len() as u32;
        param_ranges.push((start, end));
    }
    if let Some(var_ty) = &sig.variadic {
        if positional_n > 0 {
            label.push_str(", ");
        }
        let start = label.len() as u32;
        let vname = names.get(positional_n).map(|s| s.as_str()).unwrap_or("rest");
        label.push_str("...");
        label.push_str(vname);
        label.push_str(": ");
        label.push_str(&render_type(var_ty));
        let end = label.len() as u32;
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
        let start = label.len() as u32;
        label.push_str(&p.name);
        label.push_str(": ");
        label.push_str(&render_type(&p.ty));
        if p.variadic {
            label.push_str("...");
        }
        if p.default.is_some() {
            label.push_str(" = …");
        }
        let end = label.len() as u32;
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

    let active = active_parameter(args, offset, sig.params.len());

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
    /// Stdlib qualified name like `"Os.exists"` or bare native like
    /// `"println"` — resolved through `saule_typeck::sigs::lookup`.
    Native(String),
}

#[derive(Clone)]
struct CallArgInfo {
    span: std::ops::Range<usize>,
    /// `Some(i)` when this arg was written `name: value` and we could
    /// match `name` to a declared param at signature-build time. The
    /// resolution happens later in [`build_help`]; the walker just
    /// records the literal name and we look it up against the sig.
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
    best: Option<CallHit>,
}

struct Local {
    name: String,
    ty: Type,
}

impl Cx {
    fn record(&mut self, hit: CallHit) {
        if !contains(&hit.args_span, self.offset) {
            return;
        }
        let new_w = hit.args_span.end.saturating_sub(hit.args_span.start);
        if let Some(prev) = &self.best {
            let old_w = prev.args_span.end.saturating_sub(prev.args_span.start);
            if new_w >= old_w {
                return;
            }
        }
        self.best = Some(hit);
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
                self.locals.push(Local {
                    name: name.clone(),
                    ty: ty.clone().unwrap_or(Type::Named("any".into())),
                });
            }
            Stmt::LocalMulti { names, values } => {
                for v in values {
                    self.visit_expr(v);
                }
                for (n, _, t) in names {
                    self.locals.push(Local {
                        name: n.clone(),
                        ty: t.clone().unwrap_or(Type::Named("any".into())),
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
                    let user_fn = match &callee_ref {
                        CalleeRef::Free(n) => self.user_fns.get(n).cloned(),
                        _ => None,
                    };
                    self.record(CallHit {
                        callee: callee_ref,
                        args: build_arg_infos(args),
                        enclosing_class: self.enclosing_class.clone(),
                        user_fn,
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
                        args_span: args_span(&obj.span, args, e.span.end),
                    });
                }
            }
            Expr::Pipe { source, stages } => {
                self.visit_expr(source);
                for st in stages {
                    for a in &st.args {
                        visit_arg(self, a);
                    }
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
                if let Some(local) = self.locals.iter().rev().find(|l| l.name == *name) {
                    if let Type::Named(n) = &local.ty {
                        return Some(n.clone());
                    }
                }
                if with_classes(|r| r.contains_key(name)) {
                    return Some(name.clone());
                }
                None
            }
            Expr::Call { callee, .. } => {
                if let Expr::Ident(n) = &callee.value {
                    if with_classes(|r| r.contains_key(n)) {
                        return Some(n.clone());
                    }
                }
                None
            }
            Expr::MethodCall { obj, method, .. } => {
                let cls = self.receiver_class(&obj.value)?;
                let sig = lookup_method(&cls, method)?;
                if let Type::Named(n) = sig.return_ty? {
                    Some(n)
                } else {
                    None
                }
            }
            _ => None,
        }
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
                named_index: None,
            },
            CallArg::Named { value, .. } => CallArgInfo {
                span: value.span.clone(),
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

fn contains(span: &std::ops::Range<usize>, offset: usize) -> bool {
    offset >= span.start && offset <= span.end
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
        let mut cx = Cx {
            offset,
            locals: Vec::new(),
            enclosing_class: None,
            user_fns: collect_user_fns(&module),
            best: None,
        };
        cx.visit_module(&module);
        let hit = cx.best?;
        resolve_hit(hit, offset)
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
}

