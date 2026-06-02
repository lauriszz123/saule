//! Hover-information lookup over a parsed [`Module`].
//!
//! Given a byte offset into the source, walks the AST to find the
//! smallest enclosing node and renders a Markdown blurb for it,
//! consulting the thread-local semantic registries (`saule_semantic`)
//! for class / interface / enum / method metadata.
//!
//! The caller must ensure the registries are populated for the module
//! before invoking [`hover_at`] — `Backend::hover` does this by running
//! `saule_semantic::analyze_with_seed` under the analysis lock, exactly
//! like the diagnostic pipeline.
//!
//! Resolution is intentionally conservative: we don't have a per-span
//! type table from typeck yet, so member / method hovers only fire when
//! the receiver is `self` or a known class name (static access). This
//! still covers the high-leverage cases (`fn` signatures, class /
//! interface / enum heads, parameters, `self.foo`, `Class.method`).
//!
//! Each match returns `(markdown, span)`; the LSP layer uses `span` for
//! the `Hover.range` field so editors can highlight the exact node.

use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;

use saule_ast::{
    ClassMember, Decl, EnumVariant, Expr, ImportNames, Method, MethodSig as AstMethodSig, Module,
    Param, Spanned, Stmt, Type,
};
use saule_semantic::{
    ClassInfo, MethodSig, lookup_field_type, lookup_method, with_classes, with_enums,
    with_interfaces,
};
use saule_typeck::sigs::NativeSig;

/// Out-of-band import information passed into [`hover_at_with`] so the
/// resolver can answer questions the AST + registries don't cover on
/// their own:
///
/// * `fn_sigs` — top-level functions imported from another `.sau` file
///   or a native package, keyed by the *local alias* under which they
///   appear in the importing module's scope. Free functions never make
///   it into the class / interface / enum registry, so without this map
///   hovering on `foo` after `import { foo } from "lib"` would fall
///   through to "unknown ident".
/// * `import_blurbs` — pre-rendered Markdown for each `import` statement
///   keyed by its source span. The cursor is matched against the keys
///   so hovering anywhere on `import Storage from "storage"` surfaces
///   "imports `Storage` from `…/storage.sau`" without re-resolving the
///   path during the AST walk.
#[derive(Default, Clone, Debug)]
pub struct ImportContext {
    pub fn_sigs: HashMap<String, String>,
    pub import_blurbs: Vec<(Range<usize>, String)>,
}

/// Find the most specific hover info for `offset` inside `module`.
///
/// Returns `None` if no AST node contains the offset (e.g. cursor on
/// pure whitespace at the top level) or if the deepest enclosing node
/// has no useful hover content (a literal, a `nil`, etc.).
///
/// Convenience wrapper around [`hover_at_with`] for callers that don't
/// have an [`ImportContext`] (currently just the unit tests — Backend
/// always builds one). Kept `pub` for that ergonomic.
#[allow(dead_code)]
pub fn hover_at(module: &Module, offset: usize) -> Option<(String, Range<usize>)> {
    hover_at_with(module, offset, &ImportContext::default())
}

/// Like [`hover_at`] but also consults `imports` when resolving bare
/// identifiers and import-statement spans. Backend::hover builds a
/// fresh context per request from the cached source's `import`
/// declarations.
pub fn hover_at_with(
    module: &Module,
    offset: usize,
    imports: &ImportContext,
) -> Option<(String, Range<usize>)> {
    let mut cx = Cx {
        offset,
        enclosing_class: None,
        best: None,
        imports,
        locals: Vec::new(),
    };
    cx.visit_module(module);
    cx.best.map(|h| (h.md, h.span))
}

/// Build an [`ImportContext`] for `module` by walking every `import`
/// statement, resolving the target file (or native package), and
/// extracting:
///
/// 1. Top-level free function signatures, keyed by the local alias the
///    importer sees them under.
/// 2. A pre-rendered "imports `X` from `Y`" blurb keyed by the import
///    statement's source span, so hovering anywhere on the statement
///    shows where the names come from.
///
/// Best-effort: any import that fails to resolve / read / parse is
/// silently skipped — semantic analysis or the runtime will surface
/// the user-facing error elsewhere. Native packages contribute their
/// `exports` list and "native package" label.
pub fn build_import_context(module: &Module, dir: Option<&Path>) -> ImportContext {
    let mut ctx = ImportContext::default();

    for stmt in &module.stmts {
        let Stmt::Decl(d) = &stmt.value else { continue };
        let Decl::Import { names, path } = &d.value else {
            continue;
        };

        // Native package — synthesise a blurb listing the exports we
        // know about; the function signatures themselves are already
        // registered globally with `saule_typeck::sigs`, so the
        // identifier resolver will find them via the native-sig path
        // without needing per-alias entries here.
        if let Some(pkg) = saule_interpreter::native_packages::lookup(path) {
            let exports: Vec<&'static str> = pkg.exports.to_vec();
            let aliases = aliases_for_native(&exports, names);
            ctx.import_blurbs.push((
                d.span.clone(),
                render_native_import_blurb(path, &aliases),
            ));
            continue;
        }

        let Some(dir) = dir else { continue };
        let Some(abs) =
            saule_interpreter::module::resolve_import_path(dir, path)
        else {
            ctx.import_blurbs
                .push((d.span.clone(), render_unresolved_import(path)));
            continue;
        };
        let Ok(source) = std::fs::read_to_string(&abs) else {
            ctx.import_blurbs
                .push((d.span.clone(), render_unresolved_import(path)));
            continue;
        };
        let Ok(tokens) = saule_lexer::Lexer::new(&source).tokenize() else {
            continue;
        };
        let Ok(imported) = saule_parser::parse(tokens) else {
            continue;
        };

        // Collect every top-level function the imported file declares,
        // keyed by its declared name. The alias map below decides
        // which ones (and under what local name) actually land in the
        // importing module's scope.
        let mut imported_fns: HashMap<String, String> = HashMap::new();
        for s in &imported.stmts {
            if let Stmt::Decl(d) = &s.value
                && let Decl::Function {
                    name,
                    type_params,
                    params,
                    return_ty,
                    ..
                } = &d.value
            {
                imported_fns.insert(
                    name.clone(),
                    render_function_sig(name, type_params, params, return_ty.as_ref()),
                );
            }
        }

        let aliases = aliases_for_file(&imported, names);
        for (orig, alias) in &aliases {
            if let Some(md) = imported_fns.get(orig) {
                // Re-render with the alias name so hovering on the
                // local binding shows the name the user actually
                // typed, not the upstream one.
                if alias != orig {
                    if let Some(rendered) = imported_fns.get(orig) {
                        ctx.fn_sigs.insert(
                            alias.clone(),
                            rendered.replacen(&format!("fn {orig}"), &format!("fn {alias}"), 1),
                        );
                        continue;
                    }
                }
                ctx.fn_sigs.insert(alias.clone(), md.clone());
            }
        }

        ctx.import_blurbs.push((
            d.span.clone(),
            render_file_import_blurb(path, &abs.display().to_string(), &aliases),
        ));
    }

    ctx
}

/// Resolve which `(orig_name, local_alias)` pairs come into scope from
/// one file-based import statement. Mirrors the runtime's
/// `collect_import_aliases` but works against the parsed `Module`
/// directly so we don't pay the cost of re-traversing it through the
/// interpreter's helper API.
fn aliases_for_file(imported: &Module, names: &ImportNames) -> Vec<(String, String)> {
    match names {
        ImportNames::All => imported
            .stmts
            .iter()
            .filter_map(|s| match &s.value {
                Stmt::Decl(d) => exported_name(&d.value).map(|n| (n.to_string(), n.to_string())),
                _ => None,
            })
            .collect(),
        ImportNames::List(items) => items
            .iter()
            .map(|(orig, alias)| (orig.clone(), alias.clone().unwrap_or_else(|| orig.clone())))
            .collect(),
    }
}

fn aliases_for_native(exports: &[&'static str], names: &ImportNames) -> Vec<(String, String)> {
    match names {
        ImportNames::All => exports.iter().map(|n| ((*n).to_string(), (*n).to_string())).collect(),
        ImportNames::List(items) => items
            .iter()
            .map(|(orig, alias)| (orig.clone(), alias.clone().unwrap_or_else(|| orig.clone())))
            .collect(),
    }
}

fn exported_name(decl: &Decl) -> Option<&str> {
    match decl {
        Decl::Function { name, .. }
        | Decl::Class { name, .. }
        | Decl::Interface { name, .. }
        | Decl::Enum { name, .. } => Some(name),
        Decl::Import { .. } => None,
    }
}

fn render_native_import_blurb(pkg: &str, aliases: &[(String, String)]) -> String {
    let mut s = format!("```saule\n(native package) \"{pkg}\"");
    if !aliases.is_empty() {
        s.push_str("\n\nbrings into scope:\n");
        for (orig, alias) in aliases {
            s.push_str("  ");
            if alias == orig {
                s.push_str(orig);
            } else {
                s.push_str(orig);
                s.push_str(" as ");
                s.push_str(alias);
            }
            s.push('\n');
        }
    }
    s.push_str("```");
    s
}

fn render_file_import_blurb(
    path_literal: &str,
    abs_path: &str,
    aliases: &[(String, String)],
) -> String {
    let mut s = format!("```saule\n(import) \"{path_literal}\"\n```\n\n`{abs_path}`");
    if !aliases.is_empty() {
        s.push_str("\n\n```saule\n");
        s.push_str("brings into scope:\n");
        for (orig, alias) in aliases {
            s.push_str("  ");
            if alias == orig {
                s.push_str(orig);
            } else {
                s.push_str(orig);
                s.push_str(" as ");
                s.push_str(alias);
            }
            s.push('\n');
        }
        s.push_str("```");
    }
    s
}

fn render_unresolved_import(path: &str) -> String {
    format!("```saule\n(import) \"{path}\"  -- unresolved\n```")
}

struct Hit {
    span: Range<usize>,
    md: String,
}

/// One in-scope local binding (parameter, `local x =`, loop variable,
/// `try ... catch (e: T)` binding). Tracked as a flat stack — entering
/// a function/method/lambda saves the current stack and starts fresh,
/// exiting restores it. Block-level scoping inside a function is
/// approximated with a length-marker save/truncate idiom: precise
/// enough for hover, with no Vec<Vec<…>> overhead.
#[derive(Clone)]
struct LocalVar {
    name: String,
    ty: Type,
    kind: LocalKind,
}

#[derive(Clone, Copy)]
enum LocalKind {
    Param,
    Local,
    LoopVar,
    Catch,
}

struct Cx<'a> {
    offset: usize,
    enclosing_class: Option<String>,
    best: Option<Hit>,
    imports: &'a ImportContext,
    locals: Vec<LocalVar>,
}

impl<'a> Cx<'a> {
    /// Record `md` as the hover for `span` when `span` contains the
    /// cursor and is strictly narrower than any prior match.
    fn record(&mut self, span: Range<usize>, md: String) {
        if !contains(&span, self.offset) {
            return;
        }
        let new_w = span.end.saturating_sub(span.start);
        if let Some(b) = &self.best {
            let cur_w = b.span.end.saturating_sub(b.span.start);
            if new_w >= cur_w {
                return;
            }
        }
        self.best = Some(Hit { span, md });
    }

    /// Walk into a function/method/lambda body with a fresh local
    /// scope. Saves and restores the outer scope so a hover request
    /// inside a closure doesn't see locals from the enclosing function
    /// (which would be confusing) and vice versa.
    fn enter_function(&mut self, params: &[Param], body: impl FnOnce(&mut Self)) {
        let saved = std::mem::take(&mut self.locals);
        for p in params {
            self.locals.push(LocalVar {
                name: p.name.clone(),
                ty: p.ty.clone(),
                kind: LocalKind::Param,
            });
        }
        body(self);
        self.locals = saved;
    }

    /// Look up `name` in the current local scope (innermost first).
    /// Returns `None` for free identifiers — the caller falls through
    /// to the registry / native-sig path.
    fn lookup_local(&self, name: &str) -> Option<&LocalVar> {
        self.locals.iter().rev().find(|l| l.name == name)
    }

    /// Best-effort type inference for a `local x = <init>` site when
    /// the user didn't write an annotation. Handles the cases that
    /// account for the bulk of real-world `local`s in Saule code:
    ///
    /// * `Class(args)` — constructor call returns `Class`.
    /// * `obj:method(args)` — uses the registered method's return type.
    /// * `obj.field` — uses the field's declared type.
    /// * Existing local — propagates its known type.
    /// * `self` inside a method — the enclosing class.
    /// * Literal expressions — their primitive type.
    ///
    /// Anything else returns `None`; the caller falls back to `any`.
    fn infer_init_type(&self, init: &Expr) -> Option<Type> {
        match init {
            Expr::Call { callee, .. } => {
                if let Expr::Ident(name) = &callee.value {
                    if with_classes(|r| r.contains_key(name)) {
                        return Some(Type::Named(name.clone()));
                    }
                    // Non-constructor free call: consult native-sig
                    // returns or imported function signatures. We
                    // don't have ASTs for those, so return None and
                    // accept `any`.
                    if let Some(sig) = saule_typeck::sigs::lookup(name) {
                        return sig.returns.first().cloned();
                    }
                }
                None
            }
            Expr::MethodCall { obj, method, .. } => {
                let class = self.receiver_class(&obj.value)?;
                if let Some(sig) = lookup_method(&class, method) {
                    return sig.return_ty;
                }
                let qname = format!("{class}.{method}");
                if let Some(sig) = saule_typeck::sigs::lookup(&qname) {
                    return sig.returns.first().cloned();
                }
                None
            }
            Expr::Member { obj, name } | Expr::SafeMember { obj, name } => {
                let class = self.receiver_class(&obj.value)?;
                lookup_field_type(&class, name)
            }
            Expr::Ident(name) => self.lookup_local(name).map(|l| l.ty.clone()),
            Expr::Self_ => self
                .enclosing_class
                .as_ref()
                .map(|c| Type::Named(c.clone())),
            Expr::Str(_) => Some(Type::Named("string".into())),
            Expr::Int(_) => Some(Type::Named("integer".into())),
            Expr::Float(_) => Some(Type::Named("float".into())),
            Expr::Bool(_) => Some(Type::Named("boolean".into())),
            _ => None,
        }
    }

    fn visit_module(&mut self, m: &Module) {
        for s in &m.stmts {
            self.visit_stmt(s);
        }
    }

    fn visit_block(&mut self, b: &[Spanned<Stmt>]) {
        for s in b {
            self.visit_stmt(s);
        }
    }

    fn visit_stmt(&mut self, s: &Spanned<Stmt>) {
        match &s.value {
            Stmt::Decl(d) => self.visit_decl(d),
            Stmt::Local { name, ty, value } => {
                if let Some(v) = value {
                    self.visit_expr(v);
                }
                let resolved = ty
                    .clone()
                    .or_else(|| value.as_ref().and_then(|v| self.infer_init_type(&v.value)))
                    .unwrap_or_else(|| Type::Named("any".into()));
                self.locals.push(LocalVar {
                    name: name.clone(),
                    ty: resolved,
                    kind: LocalKind::Local,
                });
            }
            Stmt::LocalMulti { names, values } => {
                for v in values {
                    self.visit_expr(v);
                }
                for (i, (name, ty)) in names.iter().enumerate() {
                    let resolved = ty
                        .clone()
                        .or_else(|| {
                            values
                                .get(i)
                                .and_then(|v| self.infer_init_type(&v.value))
                        })
                        .unwrap_or_else(|| Type::Named("any".into()));
                    self.locals.push(LocalVar {
                        name: name.clone(),
                        ty: resolved,
                        kind: LocalKind::Local,
                    });
                }
            }
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
                let mark = self.locals.len();
                self.visit_block(then_block);
                self.locals.truncate(mark);
                for (c, b) in elseifs {
                    self.visit_expr(c);
                    let mark = self.locals.len();
                    self.visit_block(b);
                    self.locals.truncate(mark);
                }
                if let Some(eb) = else_block {
                    let mark = self.locals.len();
                    self.visit_block(eb);
                    self.locals.truncate(mark);
                }
            }
            Stmt::While { cond, body } => {
                self.visit_expr(cond);
                let mark = self.locals.len();
                self.visit_block(body);
                self.locals.truncate(mark);
            }
            Stmt::Repeat { body, cond } => {
                let mark = self.locals.len();
                self.visit_block(body);
                self.visit_expr(cond);
                self.locals.truncate(mark);
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
                if let Some(st) = step {
                    self.visit_expr(st);
                }
                let mark = self.locals.len();
                self.locals.push(LocalVar {
                    name: var.clone(),
                    ty: var_ty.clone().unwrap_or_else(|| Type::Named("integer".into())),
                    kind: LocalKind::LoopVar,
                });
                self.visit_block(body);
                self.locals.truncate(mark);
            }
            Stmt::ForIn { vars, iter, body } => {
                self.visit_expr(iter);
                let mark = self.locals.len();
                for (name, ty) in vars {
                    self.locals.push(LocalVar {
                        name: name.clone(),
                        ty: ty.clone().unwrap_or_else(|| Type::Named("any".into())),
                        kind: LocalKind::LoopVar,
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
                body,
                catch_var,
                catch_ty,
                catch_body,
            } => {
                let mark = self.locals.len();
                self.visit_block(body);
                self.locals.truncate(mark);
                let mark = self.locals.len();
                self.locals.push(LocalVar {
                    name: catch_var.clone(),
                    ty: catch_ty.clone(),
                    kind: LocalKind::Catch,
                });
                self.visit_block(catch_body);
                self.locals.truncate(mark);
            }
            Stmt::Break | Stmt::Continue => {}
        }
    }

    fn visit_decl(&mut self, d: &Spanned<Decl>) {
        // Skip the whole subtree when the declaration's span doesn't
        // even contain the cursor — saves an analysis on every
        // unrelated top-level item in a long file.
        if !contains(&d.span, self.offset) {
            return;
        }
        match &d.value {
            Decl::Function {
                name,
                type_params,
                params,
                return_ty,
                body,
                ..
            } => {
                self.record(
                    d.span.clone(),
                    render_function_sig(name, type_params, params, return_ty.as_ref()),
                );
                for p in params {
                    self.record(p.span.clone(), render_param(p));
                    if let Some(def) = &p.default {
                        self.visit_expr(def);
                    }
                }
                let params = params.clone();
                self.enter_function(&params, |this| this.visit_block(body));
            }
            Decl::Class {
                name,
                extends,
                implements,
                members,
                ..
            } => {
                // Prefer the registry view (uniform with how `Ident`
                // hover renders the same class), falling back to the
                // raw AST head when the registry is empty — e.g. when
                // hover is invoked on a file whose semantic pass
                // hasn't run yet.
                let md = with_classes(|r| r.get(name).cloned())
                    .map(|info| render_class_full(name, &info))
                    .unwrap_or_else(|| render_class_head(name, extends.as_deref(), implements));
                self.record(d.span.clone(), md);
                let prev = self.enclosing_class.replace(name.clone());
                for m in members {
                    self.visit_member(m);
                }
                self.enclosing_class = prev;
            }
            Decl::Interface {
                name,
                extends,
                methods,
                ..
            } => {
                self.record(d.span.clone(), render_interface_head(name, extends, methods));
            }
            Decl::Enum {
                name,
                variants,
                methods,
                ..
            } => {
                self.record(d.span.clone(), render_enum_head(name, variants));
                let prev = self.enclosing_class.replace(name.clone());
                for m in methods {
                    self.visit_method(m, name);
                }
                self.enclosing_class = prev;
            }
            Decl::Import { .. } => {
                // Best match wins: walk the precomputed blurbs and
                // record any whose span contains the cursor. Spans
                // come from the `Spanned<Decl>` itself, so they cover
                // the full statement.
                for (span, md) in &self.imports.import_blurbs {
                    if contains(span, self.offset) {
                        self.record(span.clone(), md.clone());
                    }
                }
            }
        }
    }

    fn visit_member(&mut self, m: &Spanned<ClassMember>) {
        if !contains(&m.span, self.offset) {
            return;
        }
        match &m.value {
            ClassMember::Field {
                is_static,
                is_private,
                name,
                ty,
                default,
            } => {
                let owner = self.enclosing_class.as_deref().unwrap_or("");
                self.record(
                    m.span.clone(),
                    render_field(owner, *is_static, *is_private, name, ty),
                );
                if let Some(def) = default {
                    self.visit_expr(def);
                }
            }
            ClassMember::Method(meth) => {
                let owner = self.enclosing_class.clone().unwrap_or_default();
                self.visit_method(meth, &owner);
            }
        }
    }

    fn visit_method(&mut self, m: &Method, owner: &str) {
        if !contains(&m.span, self.offset) {
            return;
        }
        self.record(m.span.clone(), render_method_head(owner, m));
        for p in &m.params {
            self.record(p.span.clone(), render_param(p));
            if let Some(def) = &p.default {
                self.visit_expr(def);
            }
        }
        let params = m.params.clone();
        self.enter_function(&params, |this| this.visit_block(&m.body));
    }

    fn visit_expr(&mut self, e: &Spanned<Expr>) {
        if !contains(&e.span, self.offset) {
            return;
        }
        // Record whatever this node resolves to *before* recursing, so
        // narrower children get a chance to shadow this one.
        if let Some(md) = self.expr_md(&e.value) {
            self.record(e.span.clone(), md);
        }
        match &e.value {
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
            Expr::Call { callee, args } => {
                self.visit_expr(callee);
                for a in args {
                    self.visit_call_arg(a);
                }
            }
            Expr::MethodCall { obj, args, .. } => {
                self.visit_expr(obj);
                for a in args {
                    self.visit_call_arg(a);
                }
            }
            Expr::ForceUnwrap(inner) => self.visit_expr(inner),
            Expr::Table(entries) => {
                for entry in entries {
                    match entry {
                        saule_ast::TableEntry::Positional(v) => self.visit_expr(v),
                        saule_ast::TableEntry::Field { key, value } => {
                            self.visit_expr(key);
                            self.visit_expr(value);
                        }
                    }
                }
            }
            Expr::Lambda { params, body, .. } => {
                for p in params {
                    self.record(p.span.clone(), render_param(p));
                    if let Some(def) = &p.default {
                        self.visit_expr(def);
                    }
                }
                let params = params.clone();
                self.enter_function(&params, |this| match body {
                    saule_ast::LambdaBody::Expr(b) => this.visit_expr(b),
                    saule_ast::LambdaBody::Block(b) => this.visit_block(b),
                });
            }
            Expr::Match { scrutinee, arms } => {
                self.visit_expr(scrutinee);
                for arm in arms {
                    if let Some(g) = &arm.guard {
                        self.visit_expr(g);
                    }
                    match &arm.body {
                        saule_ast::MatchBody::Expr(e) => self.visit_expr(e),
                        saule_ast::MatchBody::Block(b) => self.visit_block(b),
                    }
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
            Expr::Int(_)
            | Expr::Float(_)
            | Expr::Bool(_)
            | Expr::Str(_)
            | Expr::Nil
            | Expr::Ident(_)
            | Expr::Self_ => {}
        }
    }

    fn visit_call_arg(&mut self, a: &saule_ast::CallArg) {
        match a {
            saule_ast::CallArg::Positional(e) => self.visit_expr(e),
            saule_ast::CallArg::Named { value, .. } => self.visit_expr(value),
        }
    }

    /// Render a Markdown blurb for `expr` if we can resolve it from the
    /// registries / surrounding context. Returns `None` for literals
    /// and unknown names — callers should leave hover empty in that
    /// case rather than emit a misleading placeholder.
    fn expr_md(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Self_ => self
                .enclosing_class
                .as_ref()
                .map(|c| format!("```saule\n(self): {c}\n```")),
            Expr::Ident(name) => {
                // Locals shadow globals — same precedence rule the
                // resolver enforces. Render with a kind-specific
                // label so users can tell at a glance whether the
                // cursor is on a parameter, loop var, etc.
                if let Some(local) = self.lookup_local(name) {
                    let label = match local.kind {
                        LocalKind::Param => "(parameter)",
                        LocalKind::Local => "(local)",
                        LocalKind::LoopVar => "(loop var)",
                        LocalKind::Catch => "(error)",
                    };
                    return Some(format!(
                        "```saule\n{label} {name}: {ty}\n```",
                        ty = render_type(&local.ty)
                    ));
                }
                self.resolve_ident(name)
            }
            Expr::Member { obj, name } | Expr::SafeMember { obj, name } => {
                let class = self.receiver_class(&obj.value)?;
                resolve_member(&class, name, false)
            }
            Expr::MethodCall { obj, method, .. } => {
                let class = self.receiver_class(&obj.value)?;
                resolve_member(&class, method, true)
            }
            _ => None,
        }
    }

    /// Resolve a bare identifier to a hover blurb. Tries class /
    /// interface / enum registries (which include builtins and
    /// seed-imported classes), then falls back to the native-signature
    /// registry for stdlib free functions and modules, then finally to
    /// the per-request import context for top-level functions imported
    /// from another `.sau` file. Returns `None` for names we can't tie
    /// to anything (locals, parameters, unknown idents).
    fn resolve_ident(&self, name: &str) -> Option<String> {
        if let Some(info) = with_classes(|r| r.get(name).cloned()) {
            return Some(render_class_full(name, &info));
        }
        if with_interfaces(|r| r.contains_key(name)) {
            let extends = with_interfaces(|r| r.get(name).cloned()).unwrap_or_default();
            return Some(render_interface_from_registry(name, &extends));
        }
        if with_enums(|r| r.contains_key(name)) {
            let info = with_enums(|r| r.get(name).cloned())?;
            let variants: Vec<(String, usize)> =
                info.variants.iter().map(|(n, a)| (n.clone(), *a)).collect();
            return Some(render_enum_from_registry(name, &variants));
        }
        if let Some(sig) = saule_typeck::sigs::lookup(name) {
            return Some(format!(
                "```saule\nfn {name}{}\n```",
                render_native_sig_full(&sig)
            ));
        }
        if saule_typeck::sigs::is_value_type(name) {
            return Some(render_stdlib_module(name, "type"));
        }
        if saule_typeck::sigs::is_module(name) {
            return Some(render_stdlib_module(name, "module"));
        }
        // Imported user function — final fallback. The caller built
        // this map from the current module's `import` declarations.
        if let Some(md) = self.imports.fn_sigs.get(name) {
            return Some(md.clone());
        }
        None
    }

    /// Best-effort: figure out which class a member-access receiver
    /// refers to. Handles `self`, bare class-name references (static
    /// access like `Math.sqrt`), and chained `Class.foo.bar` where the
    /// inner field's declared type is a named class.
    fn receiver_class(&self, obj: &Expr) -> Option<String> {
        match obj {
            Expr::Self_ => self.enclosing_class.clone(),
            Expr::Ident(name) => {
                // Locals first — `newEntry.setDone(...)` resolves
                // through the local's declared/inferred type.
                if let Some(local) = self.lookup_local(name) {
                    return named_type(&local.ty);
                }
                if with_classes(|r| r.contains_key(name))
                    || with_enums(|r| r.contains_key(name))
                    || saule_typeck::sigs::is_module(name)
                    || saule_typeck::sigs::is_value_type(name)
                {
                    Some(name.clone())
                } else {
                    None
                }
            }
            Expr::Member { obj: inner, name } => {
                let inner_class = self.receiver_class(&inner.value)?;
                let ty = lookup_field_type(&inner_class, name)?;
                named_type(&ty)
            }
            Expr::Call { callee, .. } => {
                // `Class(args).foo` — constructor call returns the class.
                if let Expr::Ident(name) = &callee.value
                    && with_classes(|r| r.contains_key(name))
                {
                    return Some(name.clone());
                }
                None
            }
            Expr::MethodCall { obj: inner, method, .. } => {
                // `obj:method(args).foo` — chase the method's
                // registered return type.
                let inner_class = self.receiver_class(&inner.value)?;
                let sig = lookup_method(&inner_class, method)?;
                named_type(sig.return_ty.as_ref()?)
            }
            _ => None,
        }
    }
}

fn contains(r: &Range<usize>, o: usize) -> bool {
    r.start <= o && o <= r.end
}

// ──────────────────────────────────────────────────────────────────────────────
// Identifier / member resolution
// ──────────────────────────────────────────────────────────────────────────────

// ──────────────────────────────────────────────────────────────────────────────
// Member resolution
// ──────────────────────────────────────────────────────────────────────────────

/// Look up a member or method on `class` and render it. `is_call`
/// nudges the formatter toward the method shape when both a method and
/// a same-named field exist (rare, but `lookup_method` returns first).
fn resolve_member(class: &str, name: &str, is_call: bool) -> Option<String> {
    if is_call {
        if let Some(sig) = lookup_method(class, name) {
            return Some(render_method_sig(class, name, &sig));
        }
    }
    if let Some(ty) = lookup_field_type(class, name) {
        return Some(format!(
            "```saule\n(field) {class}.{name}: {ty}\n```",
            ty = render_type(&ty)
        ));
    }
    if let Some(sig) = lookup_method(class, name) {
        return Some(render_method_sig(class, name, &sig));
    }
    // Stdlib fallback: `Math.sqrt`, `String.byte`, etc.
    let qname = format!("{class}.{name}");
    if let Some(sig) = saule_typeck::sigs::lookup(&qname) {
        return Some(format!(
            "```saule\nfn {qname}{}\n```",
            render_native_sig_full(&sig)
        ));
    }
    if saule_typeck::sigs::has_member(class, name) {
        // Member is known but its signature wasn't registered (typed
        // value field like `Math.pi`). We can't say more than "yes,
        // it exists" — better than silent hover-fail though.
        return Some(format!("```saule\n(member) {class}.{name}\n```"));
    }
    None
}

fn named_type(ty: &Type) -> Option<String> {
    match ty {
        Type::Named(n) => Some(n.clone()),
        Type::Nullable(inner) => named_type(inner),
        _ => None,
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Markdown rendering
// ──────────────────────────────────────────────────────────────────────────────

fn render_function_sig(
    name: &str,
    type_params: &[String],
    params: &[Param],
    return_ty: Option<&Type>,
) -> String {
    let mut s = String::from("```saule\nfn ");
    s.push_str(name);
    if !type_params.is_empty() {
        s.push('<');
        s.push_str(&type_params.join(", "));
        s.push('>');
    }
    s.push('(');
    s.push_str(&params.iter().map(render_param_inline).collect::<Vec<_>>().join(", "));
    s.push(')');
    if let Some(rt) = return_ty {
        s.push_str(" -> ");
        s.push_str(&render_type(rt));
    }
    s.push_str("\n```");
    s
}

fn render_method_head(owner: &str, m: &Method) -> String {
    let sig = MethodSig {
        is_static: m.is_static,
        is_private: m.is_private,
        type_params: m.type_params.clone(),
        params: m.params.clone(),
        return_ty: m.return_ty.clone(),
    };
    render_method_sig(owner, &m.name, &sig)
}

fn render_method_sig(owner: &str, name: &str, sig: &MethodSig) -> String {
    let mut s = String::from("```saule\n");
    if sig.is_private {
        s.push_str("private ");
    }
    if sig.is_static {
        s.push_str("static ");
    }
    s.push_str("fn ");
    if !owner.is_empty() {
        s.push_str(owner);
        s.push('.');
    }
    s.push_str(name);
    if !sig.type_params.is_empty() {
        s.push('<');
        s.push_str(&sig.type_params.join(", "));
        s.push('>');
    }
    s.push('(');
    s.push_str(
        &sig.params
            .iter()
            .map(render_param_inline)
            .collect::<Vec<_>>()
            .join(", "),
    );
    s.push(')');
    if let Some(rt) = &sig.return_ty {
        s.push_str(" -> ");
        s.push_str(&render_type(rt));
    }
    s.push_str("\n```");
    s
}

fn render_param(p: &Param) -> String {
    let mut s = String::from("```saule\n(parameter) ");
    if p.variadic {
        s.push_str("...");
    }
    s.push_str(&p.name);
    s.push_str(": ");
    s.push_str(&render_type(&p.ty));
    if p.default.is_some() {
        s.push_str(" = …");
    }
    s.push_str("\n```");
    s
}

fn render_param_inline(p: &Param) -> String {
    let mut s = String::new();
    if p.variadic {
        s.push_str("...");
    }
    s.push_str(&p.name);
    s.push_str(": ");
    s.push_str(&render_type(&p.ty));
    if p.default.is_some() {
        s.push_str(" = …");
    }
    s
}

fn render_field(owner: &str, is_static: bool, is_private: bool, name: &str, ty: &Type) -> String {
    let mut s = String::from("```saule\n(field) ");
    if is_private {
        s.push_str("private ");
    }
    if is_static {
        s.push_str("static ");
    }
    if !owner.is_empty() {
        s.push_str(owner);
        s.push('.');
    }
    s.push_str(name);
    s.push_str(": ");
    s.push_str(&render_type(ty));
    s.push_str("\n```");
    s
}

fn render_class_head(name: &str, extends: Option<&str>, implements: &[String]) -> String {
    let mut s = format!("```saule\nclass {name}");
    if let Some(p) = extends {
        s.push_str(" extends ");
        s.push_str(p);
    }
    if !implements.is_empty() {
        s.push_str(" implements ");
        s.push_str(&implements.join(", "));
    }
    s.push_str("\n```");
    s
}

/// Render a class with its full public surface — heading, then a body
/// listing every non-private field and method in alphabetical order.
/// This is the same format used for `Ident` hover on a class name and
/// for hover on a `class` declaration head, so the two views agree.
fn render_class_full(name: &str, info: &ClassInfo) -> String {
    let mut s = format!("```saule\nclass {name}");
    if let Some(p) = &info.parent {
        s.push_str(" extends ");
        s.push_str(p);
    }
    if !info.implements.is_empty() {
        s.push_str(" implements ");
        s.push_str(&info.implements.join(", "));
    }

    // Public surface — `info.members` is the canonical visibility map.
    // Sort lexicographically so the same class always renders the same
    // way regardless of HashMap iteration order.
    let mut public: Vec<&String> = info
        .members
        .iter()
        .filter_map(|(n, priv_)| if *priv_ { None } else { Some(n) })
        .collect();
    public.sort();

    if !public.is_empty() {
        s.push_str(" {\n");
        for member in public {
            if let Some(sig) = info.methods.get(member) {
                s.push_str("  ");
                if sig.is_static {
                    s.push_str("static ");
                }
                s.push_str("fn ");
                s.push_str(member);
                if !sig.type_params.is_empty() {
                    s.push('<');
                    s.push_str(&sig.type_params.join(", "));
                    s.push('>');
                }
                s.push('(');
                s.push_str(
                    &sig.params
                        .iter()
                        .map(render_param_inline)
                        .collect::<Vec<_>>()
                        .join(", "),
                );
                s.push(')');
                if let Some(rt) = &sig.return_ty {
                    s.push_str(" -> ");
                    s.push_str(&render_type(rt));
                }
            } else if let Some(ty) = info.field_types.get(member) {
                s.push_str("  ");
                s.push_str(member);
                s.push_str(": ");
                s.push_str(&render_type(ty));
            } else {
                // Inherited or otherwise unsourced — still surface the
                // name so the hover doesn't lie about the API.
                s.push_str("  ");
                s.push_str(member);
            }
            s.push('\n');
        }
        s.push('}');
    }
    s.push_str("\n```");
    s
}

/// Hover for a stdlib static-class identifier (`Math`, `String`, …) or
/// a value type (`File`). Lists every member known to
/// `saule_typeck::sigs`, looking up signatures where available so the
/// reader sees `fn sqrt(number) -> float` instead of just `sqrt`.
fn render_stdlib_module(name: &str, kind: &str) -> String {
    let mut members = saule_typeck::sigs::module_members(name);
    members.sort();
    let mut s = format!("```saule\n{kind} {name}");
    if members.is_empty() {
        s.push_str("\n```");
        return s;
    }
    s.push_str(" {\n");
    for m in &members {
        let qname = format!("{name}.{m}");
        if let Some(sig) = saule_typeck::sigs::lookup(&qname) {
            s.push_str("  fn ");
            s.push_str(m);
            s.push_str(&render_native_sig_full(&sig));
        } else {
            // Value field with no registered call signature
            // (e.g. `Math.pi`, `Os.sep`).
            s.push_str("  ");
            s.push_str(m);
        }
        s.push('\n');
    }
    s.push_str("}\n```");
    s
}

/// Render a `NativeSig` as `[<T, U>](Type1, Type2, ...Variadic) -> Ret`.
/// Native signatures don't carry parameter names, so we print types
/// only — the user gets arity, types, and return shape, which is what
/// most stdlib calls actually need.
fn render_native_sig_full(sig: &NativeSig) -> String {
    let mut s = String::new();
    if !sig.type_params.is_empty() {
        s.push('<');
        s.push_str(&sig.type_params.join(", "));
        s.push('>');
    }
    s.push('(');
    let mut parts: Vec<String> = sig.params.iter().map(render_type).collect();
    if let Some(v) = &sig.variadic {
        parts.push(format!("...{}", render_type(v)));
    }
    s.push_str(&parts.join(", "));
    s.push(')');
    if !sig.returns.is_empty() {
        s.push_str(" -> ");
        if sig.returns.len() == 1 {
            s.push_str(&render_type(&sig.returns[0]));
        } else {
            // Multi-return: surface as a tuple.
            s.push('(');
            s.push_str(
                &sig.returns
                    .iter()
                    .map(render_type)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            s.push(')');
        }
    }
    s
}

fn render_interface_head(name: &str, extends: &[String], methods: &[AstMethodSig]) -> String {
    let mut s = format!("```saule\ninterface {name}");
    if !extends.is_empty() {
        s.push_str(" extends ");
        s.push_str(&extends.join(", "));
    }
    if !methods.is_empty() {
        s.push_str(" {\n");
        for m in methods {
            s.push_str("  fn ");
            s.push_str(&m.name);
            s.push('(');
            s.push_str(
                &m.params
                    .iter()
                    .map(render_param_inline)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            s.push(')');
            if let Some(rt) = &m.return_ty {
                s.push_str(" -> ");
                s.push_str(&render_type(rt));
            }
            s.push('\n');
        }
        s.push('}');
    }
    s.push_str("\n```");
    s
}

fn render_interface_from_registry(name: &str, extends: &[String]) -> String {
    render_interface_head(name, extends, &[])
}

fn render_enum_head(name: &str, variants: &[Spanned<EnumVariant>]) -> String {
    let mut s = format!("```saule\nenum {name} {{\n");
    for v in variants {
        s.push_str("  ");
        match &v.value {
            EnumVariant::Bare(n) => s.push_str(n),
            EnumVariant::Valued(n, _) => {
                s.push_str(n);
                s.push_str(" = …");
            }
            EnumVariant::Tuple { name, fields } => {
                s.push_str(name);
                s.push('(');
                s.push_str(
                    &fields
                        .iter()
                        .map(render_param_inline)
                        .collect::<Vec<_>>()
                        .join(", "),
                );
                s.push(')');
            }
        }
        s.push('\n');
    }
    s.push_str("}\n```");
    s
}

fn render_enum_from_registry(name: &str, variants: &[(String, usize)]) -> String {
    let mut s = format!("```saule\nenum {name} {{\n");
    for (vn, arity) in variants {
        s.push_str("  ");
        s.push_str(vn);
        if *arity > 0 {
            s.push('(');
            s.push_str(&"_, ".repeat(*arity));
            // Trim trailing ", "
            s.truncate(s.len() - 2);
            s.push(')');
        }
        s.push('\n');
    }
    s.push_str("}\n```");
    s
}

/// Local copy of the type pretty-printer. Kept here (rather than reused
/// from `saule-semantic::return_check`) to avoid widening that crate's
/// public surface for what amounts to one display helper.
fn render_type(ty: &Type) -> String {
    match ty {
        Type::Named(n) => n.clone(),
        Type::Nullable(inner) => format!("{}?", render_type(inner)),
        Type::Table { key: None, value } => format!("table<{}>", render_type(value)),
        Type::Table {
            key: Some(k),
            value,
        } => format!("table<{}, {}>", render_type(k), render_type(value)),
        Type::Tuple(parts) => {
            let inner: Vec<_> = parts.iter().map(render_type).collect();
            format!("({})", inner.join(", "))
        }
        Type::Function { params, ret } => {
            let p: Vec<_> = params.iter().map(render_type).collect();
            format!("fn({}) -> {}", p.join(", "), render_type(ret))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Once;

    /// Install the interpreter's stdlib registry hooks exactly once.
    /// Without this, `saule_typeck::sigs::lookup` and friends are empty
    /// and stdlib hover tests can't find anything.
    fn init_stdlib() {
        static ONCE: Once = Once::new();
        ONCE.call_once(saule_interpreter::init);
    }

    /// Lex + parse + analyse `src` (so the registries are populated)
    /// and return whatever hover_at produces at the byte offset of the
    /// first occurrence of `needle` (offset = needle.start + 1, i.e. a
    /// position inside the token rather than at its left edge).
    fn hover(src: &str, needle: &str) -> Option<String> {
        init_stdlib();
        let pos = src.find(needle).expect("needle not found") + 1;
        let tokens = saule_lexer::Lexer::new(src).tokenize().ok()?;
        let module = saule_parser::parse(tokens).ok()?;
        let _ = saule_semantic::analyze(&module);
        hover_at(&module, pos).map(|(md, _)| md)
    }

    /// As [`hover`] but the cursor is placed `offset` chars past the
    /// start of `needle`, useful when the relevant token isn't the one
    /// at `needle`'s left edge.
    fn hover_at_offset(src: &str, needle: &str, offset: usize) -> Option<String> {
        init_stdlib();
        let pos = src.find(needle).expect("needle not found") + offset;
        let tokens = saule_lexer::Lexer::new(src).tokenize().ok()?;
        let module = saule_parser::parse(tokens).ok()?;
        let _ = saule_semantic::analyze(&module);
        hover_at(&module, pos).map(|(md, _)| md)
    }

    #[test]
    fn hovers_top_level_function() {
        let src = "fn add(a: integer, b: integer) -> integer\n  return a + b\nend\n";
        let md = hover(src, "add").unwrap();
        assert!(md.contains("fn add"), "got: {md}");
        assert!(md.contains("a: integer"), "got: {md}");
        assert!(md.contains("-> integer"), "got: {md}");
    }

    #[test]
    fn hovers_parameter() {
        let src = "fn add(a: integer, b: integer) -> integer\n  return a + b\nend\n";
        let md = hover(src, "a: integer").unwrap();
        assert!(md.contains("(parameter)"), "got: {md}");
        assert!(md.contains("a: integer"), "got: {md}");
    }

    #[test]
    fn hovers_class_head() {
        let src = "\
class Point
  x: integer = 0
  y: integer = 0
end
";
        let head = hover(src, "Point").unwrap();
        assert!(head.contains("class Point"), "got: {head}");
    }

    #[test]
    fn hovers_self_field() {
        let src = "\
class Point
  x: integer = 0
  fn get_x() -> integer
    return self.x
  end
end
";
        // Position the cursor on `.x` inside `self.x`.
        let needle = "self.x\n  end";
        let pos = src.find(needle).unwrap() + "self.".len() + 1;
        let tokens = saule_lexer::Lexer::new(src).tokenize().unwrap();
        let module = saule_parser::parse(tokens).unwrap();
        let _ = saule_semantic::analyze(&module);
        let md = hover_at(&module, pos).map(|(md, _)| md).unwrap();
        assert!(md.contains("Point.x"), "got: {md}");
        assert!(md.contains(": integer"), "got: {md}");
    }

    #[test]
    fn hovers_static_method_call() {
        let src = "\
class Counter
  static fn make() -> integer
    return 42
  end
end

fn use_it() -> integer
  return Counter.make()
end
";
        let pos = src.find("Counter.make()").unwrap() + "Counter.".len() + 1;
        let tokens = saule_lexer::Lexer::new(src).tokenize().unwrap();
        let module = saule_parser::parse(tokens).unwrap();
        let _ = saule_semantic::analyze(&module);
        let md = hover_at(&module, pos).map(|(md, _)| md).unwrap();
        assert!(md.contains("static"), "got: {md}");
        assert!(md.contains("Counter.make"), "got: {md}");
    }

    #[test]
    fn class_hover_lists_public_members() {
        let src = "\
class Point
  x: integer = 0
  y: integer = 0
  local secret: integer = 0
  fn move(dx: integer, dy: integer) -> nothing
    self.x = self.x + dx
    self.y = self.y + dy
  end
  local fn _hidden() -> nothing
  end
end
";
        let md = hover(src, "Point").unwrap();
        assert!(md.contains("class Point"), "got: {md}");
        assert!(md.contains("x: integer"), "got: {md}");
        assert!(md.contains("y: integer"), "got: {md}");
        assert!(md.contains("fn move"), "got: {md}");
        // Private members must not leak.
        assert!(!md.contains("secret"), "got: {md}");
        assert!(!md.contains("_hidden"), "got: {md}");
    }

    #[test]
    fn hovers_stdlib_free_function() {
        // `print` is a prelude name with a registered native sig.
        let src = "fn main() -> nothing\n  print(\"hi\")\nend\n";
        let md = hover_at_offset(src, "print(", 1).unwrap();
        assert!(md.contains("fn print"), "got: {md}");
    }

    #[test]
    fn hovers_stdlib_module_member() {
        // `Math.sqrt` should resolve through the native-sig registry
        // since `Math` isn't a real class in the semantic registry.
        let src = "\
fn root() -> float
  return Math.sqrt(2.0)
end
";
        let md = hover_at_offset(src, "Math.sqrt", "Math.".len() + 1).unwrap();
        assert!(md.contains("Math.sqrt"), "got: {md}");
        assert!(md.contains("->"), "got: {md}");
    }

    #[test]
    fn hovers_stdlib_module_name() {
        let src = "\
fn root() -> float
  return Math.sqrt(2.0)
end
";
        let md = hover_at_offset(src, "Math.sqrt", 1).unwrap();
        assert!(md.contains("module Math") || md.contains("type Math"), "got: {md}");
        // Module body should list at least one known member.
        assert!(md.contains("sqrt"), "got: {md}");
    }

    /// End-to-end: write two `.sau` files into a tempdir, import the
    /// first from the second, and confirm hover on the imported class
    /// name surfaces its full definition (the user's reported case).
    #[test]
    fn hovers_imported_class_from_disk() {
        init_stdlib();
        let dir = std::env::temp_dir().join(format!(
            "saule-lsp-hover-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let storage_path = dir.join("storage.sau");
        std::fs::write(
            &storage_path,
            "\
class Storage
  name: string = \"\"
  fn save(payload: string) -> nothing
  end
end
",
        )
        .unwrap();

        let app_src = "\
import Storage from \"storage\"

fn run() -> nothing
  local s: Storage = Storage()
end
";
        let tokens = saule_lexer::Lexer::new(app_src).tokenize().unwrap();
        let module = saule_parser::parse(tokens).unwrap();

        // Mirror what `Backend::hover_at` does: collect the seed,
        // analyse, build the import context, then hover.
        let seed = saule_interpreter::module::collect_import_seed(&module, &dir);
        let _ = saule_semantic::analyze_with_seed(&module, seed);
        let imports = build_import_context(&module, Some(&dir));

        // Cursor on the constructor call `Storage()` (the type
        // ascription `: Storage` isn't visited — type nodes don't
        // carry hover info, only expressions do).
        let needle = "Storage()";
        let pos = app_src.find(needle).unwrap() + 1;
        let md = hover_at_with(&module, pos, &imports).map(|(m, _)| m).unwrap();
        assert!(md.contains("class Storage"), "got: {md}");
        assert!(md.contains("name: string"), "got: {md}");
        assert!(md.contains("fn save"), "got: {md}");

        // Hovering on the import statement itself surfaces the path.
        let import_pos = app_src.find("import Storage").unwrap() + 2;
        let md = hover_at_with(&module, import_pos, &imports)
            .map(|(m, _)| m)
            .unwrap();
        assert!(md.contains("(import)"), "got: {md}");
        assert!(md.contains("Storage"), "got: {md}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Importing a top-level free function should make hover on a call
    /// site surface its signature, even though free functions don't go
    /// through the semantic class registry.
    #[test]
    fn hovers_imported_free_function() {
        init_stdlib();
        let dir = std::env::temp_dir().join(format!(
            "saule-lsp-hover-fn-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        std::fs::write(
            dir.join("util.sau"),
            "\
fn greet(name: string) -> string
  return \"hi \" .. name
end
",
        )
        .unwrap();

        let app_src = "\
import greet from \"util\"

fn main() -> nothing
  print(greet(\"world\"))
end
";
        let tokens = saule_lexer::Lexer::new(app_src).tokenize().unwrap();
        let module = saule_parser::parse(tokens).unwrap();

        let seed = saule_interpreter::module::collect_import_seed(&module, &dir);
        let _ = saule_semantic::analyze_with_seed(&module, seed);
        let imports = build_import_context(&module, Some(&dir));

        let pos = app_src.find("greet(\"world\")").unwrap() + 1;
        let md = hover_at_with(&module, pos, &imports)
            .map(|(m, _)| m)
            .unwrap();
        assert!(md.contains("fn greet"), "got: {md}");
        assert!(md.contains("name: string"), "got: {md}");
        assert!(md.contains("-> string"), "got: {md}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The user's reported case: an unannotated `local newEntry =
    /// Entry(...)` followed by method calls on `newEntry`. Hover on
    /// the local should surface its inferred type, and method-call
    /// hover should resolve through it.
    #[test]
    fn hovers_local_inferred_from_constructor() {
        let src = "\
class Entry
  todo: string = \"\"
  done: boolean = false
  fn setDone(value: boolean) -> nothing
    self.done = value
  end
end

fn use_it() -> nothing
  local newEntry = Entry()
  newEntry.setDone(true)
end
";
        // Hover on the local-binding use site (the second `newEntry`).
        let pos = src.find("newEntry.setDone").unwrap() + 1;
        let tokens = saule_lexer::Lexer::new(src).tokenize().unwrap();
        let module = saule_parser::parse(tokens).unwrap();
        let _ = saule_semantic::analyze(&module);
        let md = hover_at(&module, pos).map(|(m, _)| m).unwrap();
        assert!(md.contains("(local)"), "got: {md}");
        assert!(md.contains("newEntry: Entry"), "got: {md}");

        // Hover on the `setDone` member should resolve via the
        // local's inferred type back to the method signature.
        let pos = src.find("newEntry.setDone").unwrap() + "newEntry.".len() + 1;
        let md = hover_at(&module, pos).map(|(m, _)| m).unwrap();
        assert!(md.contains("Entry.setDone"), "got: {md}");
        assert!(md.contains("value: boolean"), "got: {md}");
    }

    /// Annotated `local s: Storage = ...` should give the same hover
    /// info as the inferred case via the type ascription.
    #[test]
    fn hovers_local_with_annotation() {
        let src = "\
class Storage
  fn save() -> nothing
  end
end

fn run() -> nothing
  local s: Storage = Storage()
  s.save()
end
";
        let pos = src.find("s.save()").unwrap();
        let tokens = saule_lexer::Lexer::new(src).tokenize().unwrap();
        let module = saule_parser::parse(tokens).unwrap();
        let _ = saule_semantic::analyze(&module);
        let md = hover_at(&module, pos).map(|(m, _)| m).unwrap();
        assert!(md.contains("(local)"), "got: {md}");
        assert!(md.contains("s: Storage"), "got: {md}");
    }
}











