//! Inlay hint provider — emits two kinds of hints:
//!
//! * **Type hints on locals** — `local x = Foo()` → `x : Foo` ghost
//!   text after the local's name. Only fires when the right-hand side
//!   has a confidently-inferrable named type (constructor call,
//!   string/integer/float/bool literal, identifier referencing a
//!   typed local). This deliberately stays narrow: false-positive
//!   inlay hints are visually loud and we'd rather skip than mislead.
//!
//! * **Parameter-name hints on positional call args** — `add(2, 3)` →
//!   `add(a: 2, b: 3)` ghost labels in front of each argument so the
//!   reader sees what slot they're filling. Suppressed when the arg
//!   expression already mentions the param name (`add(a, b)` shouldn't
//!   show `a: a`) and for trivially-named single-arg calls
//!   (`println("..."`)).
//!
//! Hints are produced from the same thread-local class / interface /
//! enum registries the hover walker uses, so they reflect imports
//! seeded by `analyze_with_seed`.

use saule_ast::{
    CallArg, ClassMember, Decl, Expr, LambdaBody, MatchBody, Method, Module, Param, Spanned,
    Stmt, TableEntry, Type,
};
use saule_semantic::{lookup_method, with_classes};
use tower_lsp::lsp_types::{InlayHint, InlayHintKind, InlayHintLabel, Url};

use crate::line_index::LineIndex;

use super::{canonical, Backend};

impl Backend {
    /// Compute inlay hints for `uri`. Re-runs analysis under the shared
    /// lock so the registries reflect the current document. Returns an
    /// empty vec on lex / parse failure or for closed documents.
    pub(super) async fn inlay_hints(&self, uri: &Url) -> Vec<InlayHint> {
        let entry = match self.docs.get(uri.as_str()) {
            Some(e) => e,
            None => return Vec::new(),
        };
        let source = entry.source.clone();
        drop(entry);

        let Ok(tokens) = saule_lexer::Lexer::new(&source).tokenize() else {
            return Vec::new();
        };
        let Ok(module) = saule_parser::parse(tokens) else {
            return Vec::new();
        };
        let line_index = LineIndex::new(&source);

        let module_dir = uri
            .to_file_path()
            .ok()
            .and_then(|p| canonical(&p))
            .and_then(|p| p.parent().map(|d| d.to_path_buf()));
        let _guard = self.analysis_lock.lock().await;
        if let Some(info) = self.project_info.lock().await.clone() {
            saule_interpreter::project::set(info);
        }
        let seed = match &module_dir {
            Some(d) => saule_interpreter::module::collect_import_seed(&module, d),
            None => saule_semantic::ModuleSeed::default(),
        };
        let _ = saule_semantic::analyze_with_seed(&module, seed);

        let mut cx = Cx {
            source: &source,
            out: Vec::new(),
            locals: Vec::new(),
            enclosing_class: None,
        };
        cx.visit_module(&module);
        cx.out
            .into_iter()
            .map(|raw| InlayHint {
                position: line_index.position(&source, raw.byte),
                label: InlayHintLabel::String(raw.label),
                kind: Some(raw.kind),
                text_edits: None,
                tooltip: None,
                padding_left: raw.padding_left,
                padding_right: raw.padding_right,
                data: None,
            })
            .collect()
    }
}

struct RawHint {
    byte: usize,
    label: String,
    kind: InlayHintKind,
    padding_left: Option<bool>,
    padding_right: Option<bool>,
}

struct Local {
    name: String,
    ty: Type,
}

struct Cx<'a> {
    #[allow(dead_code)]
    source: &'a str,
    out: Vec<RawHint>,
    locals: Vec<Local>,
    enclosing_class: Option<String>,
}

impl<'a> Cx<'a> {
    // ── traversal ───────────────────────────────────────────────────

    fn visit_module(&mut self, module: &Module) {
        for s in &module.stmts {
            self.visit_stmt(s);
        }
    }

    fn visit_stmt(&mut self, s: &Spanned<Stmt>) {
        match &s.value {
            Stmt::Local { name, ty, value, name_span, .. } => {
                let resolved_ty = ty.clone().or_else(|| {
                    value.as_ref().and_then(|v| self.infer_type(&v.value))
                });
                if ty.is_none() {
                    if let Some(ref t) = resolved_ty {
                        if let Some(label) = render_type(t) {
                            self.out.push(RawHint {
                                byte: name_span.end,
                                label: format!(": {label}"),
                                kind: InlayHintKind::TYPE,
                                padding_left: None,
                                padding_right: None,
                            });
                        }
                    }
                }
                if let Some(v) = value {
                    self.visit_expr(v);
                }
                self.locals.push(Local {
                    name: name.clone(),
                    ty: resolved_ty.unwrap_or(Type::Named("any".into())),
                });
            }
            Stmt::LocalMulti { names, values } => {
                for v in values {
                    self.visit_expr(v);
                }
                for (n, t) in names {
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
                var_ty,
                from,
                to,
                step,
                body,
            } => {
                self.visit_expr(from);
                self.visit_expr(to);
                if let Some(s) = step {
                    self.visit_expr(s);
                }
                let mark = self.locals.len();
                self.locals.push(Local {
                    name: var.clone(),
                    ty: var_ty.clone().unwrap_or(Type::Named("integer".into())),
                });
                self.visit_block(body);
                self.locals.truncate(mark);
            }
            Stmt::ForIn { vars, iter, body } => {
                self.visit_expr(iter);
                let mark = self.locals.len();
                for (n, t) in vars {
                    self.locals.push(Local {
                        name: n.clone(),
                        ty: t.clone().unwrap_or(Type::Named("any".into())),
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
            Stmt::Try {
                body, catch_body, ..
            } => {
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
                let params = self.callee_params(&callee.value);
                self.emit_param_hints(args, params.as_deref());
                for a in args {
                    self.visit_call_arg(a);
                }
            }
            Expr::MethodCall { obj, method, args } => {
                self.visit_expr(obj);
                let params = self
                    .receiver_class(&obj.value)
                    .and_then(|c| lookup_method(&c, method))
                    .map(|sig| sig.params);
                self.emit_param_hints(args, params.as_deref());
                for a in args {
                    self.visit_call_arg(a);
                }
            }
            Expr::Pipe { source, stages } => {
                self.visit_expr(source);
                for st in stages {
                    for a in &st.args {
                        self.visit_call_arg(a);
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
                let saved = std::mem::take(&mut self.locals);
                for p in params {
                    self.locals.push(Local {
                        name: p.name.clone(),
                        ty: p.ty.clone(),
                    });
                }
                match body {
                    LambdaBody::Expr(b) => self.visit_expr(b),
                    LambdaBody::Block(b) => self.visit_block(b),
                }
                self.locals = saved;
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

    fn visit_call_arg(&mut self, a: &CallArg) {
        match a {
            CallArg::Positional(e) | CallArg::Named { value: e, .. } => self.visit_expr(e),
        }
    }

    // ── inlay emission ──────────────────────────────────────────────

    /// Emit `name:` hints in front of every positional argument whose
    /// matching parameter we could resolve. Suppressed when the arg
    /// is itself the bare parameter name (`add(a, b)` would be noisy)
    /// or already named at the source level.
    fn emit_param_hints(&mut self, args: &[CallArg], params: Option<&[Param]>) {
        let Some(params) = params else { return };
        // Strip a leading `self` / receiver slot from the signature
        // when the call is a method call — saule's class registry
        // keeps `self` implicit, but a few entry points include it.
        // We index by position into `args`, so just walk in lockstep.
        let mut pi = 0;
        for arg in args {
            match arg {
                CallArg::Named { .. } => {
                    // The user already wrote the name; advance past
                    // the matching declared param (best-effort).
                    pi += 1;
                }
                CallArg::Positional(value) => {
                    let Some(param) = params.get(pi) else { break };
                    pi += 1;
                    if param.variadic {
                        // Variadic slots accept any number of args;
                        // a single label in front of every remaining
                        // arg would be misleading.
                        break;
                    }
                    if let Expr::Ident(n) = &value.value {
                        if n == &param.name {
                            continue;
                        }
                    }
                    self.out.push(RawHint {
                        byte: value.span.start,
                        label: format!("{}:", param.name),
                        kind: InlayHintKind::PARAMETER,
                        padding_left: None,
                        padding_right: Some(true),
                    });
                }
            }
        }
    }

    // ── lightweight type inference (locals + constructor calls) ─────

    fn infer_type(&self, e: &Expr) -> Option<Type> {
        match e {
            Expr::Int(_) => Some(Type::Named("integer".into())),
            Expr::Float(_) => Some(Type::Named("float".into())),
            Expr::Bool(_) => Some(Type::Named("boolean".into())),
            Expr::Str(_) => Some(Type::Named("string".into())),
            Expr::Nil => None,
            Expr::Ident(name) => self
                .locals
                .iter()
                .rev()
                .find(|l| l.name == *name)
                .map(|l| l.ty.clone()),
            Expr::Call { callee, .. } => {
                if let Expr::Ident(name) = &callee.value {
                    if with_classes(|r| r.contains_key(name)) {
                        return Some(Type::Named(name.clone()));
                    }
                }
                None
            }
            Expr::MethodCall { obj, method, .. } => {
                let class = self.receiver_class(&obj.value)?;
                lookup_method(&class, method)?.return_ty
            }
            Expr::Self_ => self.enclosing_class.clone().map(Type::Named),
            _ => None,
        }
    }

    /// Best-effort: figure out which class a member-access receiver
    /// refers to. Mirrors the hover walker's `receiver_class` but
    /// limited to the cases inlay hints actually need.
    fn receiver_class(&self, obj: &Expr) -> Option<String> {
        match obj {
            Expr::Self_ => self.enclosing_class.clone(),
            Expr::Ident(name) => {
                if let Some(local) = self
                    .locals
                    .iter()
                    .rev()
                    .find(|l| l.name == *name)
                {
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

    fn callee_params(&self, callee: &Expr) -> Option<Vec<Param>> {
        match callee {
            Expr::Ident(name) => {
                if with_classes(|r| r.contains_key(name)) {
                    return lookup_method(name, "init").map(|sig| sig.params);
                }
                if let Some(class) = &self.enclosing_class {
                    if let Some(sig) = lookup_method(class, name) {
                        return Some(sig.params);
                    }
                }
                None
            }
            Expr::Member { obj, name } => {
                let class = self.receiver_class(&obj.value)?;
                lookup_method(&class, name).map(|sig| sig.params)
            }
            _ => None,
        }
    }
}

/// Render a `Type` as a short label suitable for an inlay hint. Returns
/// `None` for types we don't want to surface (`any` is noise; unnamed
/// fallbacks would be misleading).
fn render_type(ty: &Type) -> Option<String> {
    match ty {
        Type::Named(n) if n == "any" || n.is_empty() => None,
        Type::Named(n) => Some(n.clone()),
        Type::Nullable(inner) => render_type(inner).map(|s| format!("{s}?")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    //! Inlay hint tests. We bypass `Backend` entirely and run the
    //! walker against a parsed+analysed module, then assert on the
    //! resulting `(kind, byte, label)` triples.

    use super::*;

    fn raw_hints(src: &str) -> Vec<(InlayHintKind, usize, String)> {
        let tokens = saule_lexer::Lexer::new(src).tokenize().expect("lex");
        let module = saule_parser::parse(tokens).expect("parse");
        let _ = saule_semantic::analyze(&module);
        let mut cx = Cx {
            source: src,
            out: Vec::new(),
            locals: Vec::new(),
            enclosing_class: None,
        };
        cx.visit_module(&module);
        cx.out
            .into_iter()
            .map(|h| (h.kind, h.byte, h.label))
            .collect()
    }

    #[test]
    fn type_hint_for_inferred_local_from_constructor() {
        let src = "class Point\n  x: integer = 0\nend\n\nfn main()\n  local p = Point()\nend\n";
        let hints = raw_hints(src);
        let type_hints: Vec<_> = hints
            .iter()
            .filter(|(k, _, _)| *k == InlayHintKind::TYPE)
            .collect();
        assert_eq!(type_hints.len(), 1, "got {hints:?}");
        assert_eq!(type_hints[0].2, ": Point");
    }

    #[test]
    fn no_type_hint_when_already_annotated() {
        let src = "fn main()\n  local x: integer = 1\nend\n";
        let hints = raw_hints(src);
        let type_hints: Vec<_> = hints
            .iter()
            .filter(|(k, _, _)| *k == InlayHintKind::TYPE)
            .collect();
        assert!(type_hints.is_empty(), "got {hints:?}");
    }

    #[test]
    fn type_hint_for_int_literal() {
        let src = "fn main()\n  local n = 42\nend\n";
        let hints = raw_hints(src);
        let labels: Vec<&String> = hints
            .iter()
            .filter(|(k, _, _)| *k == InlayHintKind::TYPE)
            .map(|(_, _, l)| l)
            .collect();
        assert_eq!(labels, vec![&": integer".to_string()]);
    }

    #[test]
    fn parameter_hint_for_positional_call_within_class() {
        // Free top-level functions aren't resolved by inlay yet, so
        // exercise the param-hint path through a sibling-class call.
        let src = "class Calc\n  fn add(a: integer, b: integer) -> integer\n    return a + b\n  end\n  fn main()\n    local r = self.add(1, 2)\n  end\nend\n";
        let hints = raw_hints(src);
        let labels: Vec<&String> = hints
            .iter()
            .filter(|(k, _, _)| *k == InlayHintKind::PARAMETER)
            .map(|(_, _, l)| l)
            .collect();
        assert!(labels.contains(&&"a:".to_string()), "got {hints:?}");
        assert!(labels.contains(&&"b:".to_string()), "got {hints:?}");
    }

    #[test]
    fn parameter_hint_suppressed_when_arg_matches_param_name() {
        let src = "class Calc\n  fn add(a: integer, b: integer) -> integer\n    return a + b\n  end\n  fn main()\n    local a = 1\n    local b = 2\n    local r = self.add(a, b)\n  end\nend\n";
        let hints = raw_hints(src);
        let param_hints: Vec<_> = hints
            .iter()
            .filter(|(k, _, _)| *k == InlayHintKind::PARAMETER)
            .collect();
        assert!(param_hints.is_empty(), "got {param_hints:?}");
    }

    #[test]
    fn parameter_hint_for_class_constructor() {
        let src = "class Point\n  x: integer = 0\n  y: integer = 0\n  fn init(x: integer, y: integer)\n    self.x = x\n    self.y = y\n  end\nend\n\nfn main()\n  local p = Point(1, 2)\nend\n";
        let hints = raw_hints(src);
        let labels: Vec<&String> = hints
            .iter()
            .filter(|(k, _, _)| *k == InlayHintKind::PARAMETER)
            .map(|(_, _, l)| l)
            .collect();
        assert!(labels.contains(&&"x:".to_string()), "got {hints:?}");
        assert!(labels.contains(&&"y:".to_string()), "got {hints:?}");
    }
}

