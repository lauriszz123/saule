//! `textDocument/completion` — a parse-driven completion engine.
//!
//! The cursor position is turned into a real AST node rather than guessed at
//! from the surrounding characters: the partial word under the caret is
//! replaced with a unique **sentinel identifier** and the patched source is
//! parsed. Wherever that sentinel lands in the tree *is* the context —
//!
//! * `Expr::Member { obj, name: SENTINEL }` → complete members of `obj`
//! * `Type::Named(SENTINEL)`               → complete type names
//! * `Expr::Ident(SENTINEL)`               → complete values in scope
//! * `Decl::Class { extends: SENTINEL }`   → complete base classes
//!
//! Two things fall out of this for free. A caret inside a comment or a string
//! swallows the sentinel into a trivia/literal token, so it never appears in
//! the tree and no suggestions are offered — no textual guard needed. And `..`
//! (concatenation) parses as a binary operator rather than member access, so
//! it can't be mistaken for a `.` field lookup.
//!
//! Candidates are then drawn strictly from what is *visible* at that node: a
//! scope stack built while descending to it (parameters, `local`s declared
//! earlier in the enclosing blocks, loop and catch bindings, the enclosing
//! class's own members) plus the module's imports. Nothing is offered that the
//! position cannot actually use.

use saule_ast::{
    CallArg, ClassMember, Decl, Expr, LambdaBody, MatchBody, Module, Param, Spanned, Stmt, Type,
};
use saule_semantic::registry::{
    interface_extends, lookup_field_type, lookup_method, with_classes, with_enums, with_interfaces,
    ClassRegistry,
};
use saule_semantic::MethodSig;
use saule_typeck::sigs::{self, NativeSig};
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, CompletionResponse, Documentation, MarkupContent,
    MarkupKind, Position, Url,
};

use super::sighelp::render_type;
use super::{canonical, Backend};
use crate::line_index::LineIndex;

/// Identifier spliced in at the caret. Deliberately unlikely to collide with
/// real user code.
const SENTINEL: &str = "__saule_completion__";

impl Backend {
    pub(super) async fn completion_at(
        &self,
        uri: &Url,
        pos: Position,
    ) -> Option<CompletionResponse> {
        let entry = self.docs.get(uri.as_str())?;
        let source = entry.source.clone();
        drop(entry);

        let line_index = LineIndex::new(&source);
        let offset = line_index.offset(&source, pos);
        let (patched, prefix) = splice_sentinel(&source, offset)?;

        let module_dir = uri
            .to_file_path()
            .ok()
            .and_then(|p| canonical(&p))
            .and_then(|p| p.parent().map(|d| d.to_path_buf()));

        // The semantic passes write thread-local registries; serialise them.
        let _guard = self.analysis_lock.lock().await;
        if let Some(info) = self.project_info.lock().await.clone() {
            saule_interpreter::project::set(info);
        }

        let module = parse_tolerant(&patched)?;

        // Populate the class / enum / interface registries with this module's
        // declarations *and* everything it imports, so member lookups and type
        // names resolve across files.
        let seed = match &module_dir {
            Some(d) => saule_interpreter::module::collect_import_seed(&module, d),
            None => saule_semantic::ModuleSeed::default(),
        };
        let _ = saule_semantic::analyze_with_seed(&module, seed);

        let found = Walk::run(&module)?;

        let items = match &found.ctx {
            Ctx::Member(recv) => member_items(recv, &found),
            Ctx::TypeName => type_items(),
            Ctx::BaseClass { exclude } => class_items(exclude),
            Ctx::Interfaces { exclude } => interface_items(exclude),
            Ctx::Value { stmt_start } => value_items(&found, &module, *stmt_start),
            Ctx::ImportPath { quoted } => self.import_path_items(module_dir.as_deref(), *quoted),
            Ctx::ImportName { path } => import_name_items(path, module_dir.as_deref()),
        };

        Some(CompletionResponse::Array(filter(items, &prefix)))
    }

    /// Modules that can be imported from here: installed native packages plus
    /// every `.sau` file and folder module in the workspace, expressed the way
    /// the author is already spelling paths.
    fn import_path_items(&self, dir: Option<&std::path::Path>, quoted: bool) -> Vec<CompletionItem> {
        let sep = if quoted { "/" } else { "." };
        let mut items = Vec::new();

        // Only packages that actually require an import. Anything with
        // `auto_prelude` (the stdlib: `table`, `math`, `io`, …) is already
        // installed into every scope, so importing it is meaningless.
        for pkg in saule_interpreter::native_packages::all() {
            if pkg.auto_prelude {
                continue;
            }
            items.push(sorted(
                item(
                    pkg.name.to_string(),
                    CompletionItemKind::MODULE,
                    Some("native package".into()),
                ),
                "0",
            ));
        }
        for name in saule_interpreter::dynamic_packages::package_names() {
            let n = saule_interpreter::dynamic_packages::export_names(&name).len();
            items.push(sorted(
                item(
                    name,
                    CompletionItemKind::MODULE,
                    Some(format!("native package ({n} exported classes)")),
                ),
                "0",
            ));
        }

        // Workspace files, relative to this file's folder first, then to the
        // project's `src_dirs`. A folder holding `init.sau` is offered as the
        // folder itself, since that is what you import.
        let src_dirs: Vec<std::path::PathBuf> = saule_interpreter::project::get()
            .map(|p| p.src_dirs.clone())
            .unwrap_or_default();

        let mut seen: Vec<String> = Vec::new();
        for entry in self.workspace_files.iter() {
            let file = entry.key().clone();
            let is_barrel = file.file_stem().and_then(|s| s.to_str()) == Some("init");
            // For a barrel the importable thing is its directory.
            let target = if is_barrel {
                match file.parent() {
                    Some(p) => p.to_path_buf(),
                    None => continue,
                }
            } else {
                file.with_extension("")
            };

            let mut bases: Vec<&std::path::Path> = Vec::new();
            if let Some(d) = dir {
                bases.push(d);
            }
            for sd in &src_dirs {
                bases.push(sd.as_path());
            }

            let rel = bases
                .iter()
                .find_map(|base| target.strip_prefix(base).ok())
                .and_then(|r| r.to_str())
                .map(|r| r.replace('\\', "/"));

            let Some(rel) = rel else { continue };
            if rel.is_empty() {
                continue;
            }
            let label = rel.replace('/', sep);
            // A bare path can only be written with identifier segments.
            if !quoted
                && !label
                    .split('.')
                    .all(|seg| !seg.is_empty() && seg.chars().all(|c| c == '_' || c.is_alphanumeric()))
            {
                continue;
            }
            if seen.contains(&label) {
                continue;
            }
            seen.push(label.clone());

            let detail = if is_barrel {
                "folder module".to_string()
            } else {
                "module".to_string()
            };
            items.push(sorted(
                item(label, CompletionItemKind::FILE, Some(detail)),
                "1",
            ));
        }

        items
    }
}

fn parse(src: &str) -> Option<Module> {
    saule_lexer::Lexer::new(src)
        .tokenize()
        .ok()
        .and_then(|t| saule_parser::parse(t).ok())
}

/// How many blocks we're willing to close for the author.
const MAX_REPAIR: usize = 8;

/// Code is written top-down, so the `end` closing the declaration the caret
/// sits in usually hasn't been typed yet — a plain parse of a class being
/// written fails, and completion would have nothing to work with. Close the
/// open blocks artificially and retry. Only reached when the source doesn't
/// parse as-is, so this can add suggestions but never change existing ones.
fn parse_tolerant(src: &str) -> Option<Module> {
    if let Some(m) = parse(src) {
        return Some(m);
    }
    let mut patched = src.to_string();
    for _ in 0..MAX_REPAIR {
        patched.push_str("\nend");
        if let Some(m) = parse(&patched) {
            return Some(m);
        }
    }
    None
}

/// Replace the partial identifier under the caret with [`SENTINEL`],
/// returning the patched source and the text the user had typed.
fn splice_sentinel(source: &str, offset: usize) -> Option<(String, String)> {
    let before = source.get(..offset)?;
    let start = before
        .char_indices()
        .rev()
        .find(|(_, c)| !(*c == '_' || c.is_alphanumeric()))
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    let prefix = before[start..].to_string();

    let mut patched = String::with_capacity(source.len() + SENTINEL.len());
    patched.push_str(&source[..start]);
    patched.push_str(SENTINEL);
    patched.push_str(&source[offset..]);
    Some((patched, prefix))
}

// ─── what the caret can see ─────────────────────────────────────────────────

/// A binding visible at the caret.
struct Visible {
    name: String,
    /// Declared type, when the binding was annotated.
    ty: Option<Type>,
    /// Initialiser expression, kept so an un-annotated `local p = Player(…)`
    /// can still be resolved — type annotations are optional in Saule, so
    /// relying on `ty` alone would miss most real code.
    init: Option<Expr>,
    kind: &'static str,
}

enum Ctx {
    /// `receiver.<caret>` — the receiver expression.
    Member(Spanned<Expr>),
    /// A type annotation or return type.
    TypeName,
    /// `class Foo extends <caret>` — a base class. `exclude` holds the names
    /// that would close a cycle in the inheritance chain.
    BaseClass { exclude: Vec<String> },
    /// `class Foo implements <caret>` or `interface Foo extends <caret>` —
    /// interfaces, minus the ones already named in the header.
    Interfaces { exclude: Vec<String> },
    /// A value position. `stmt_start` is true when the caret begins a
    /// statement, which is the only place statement keywords make sense.
    Value { stmt_start: bool },
    /// `import * from <caret>` — offer importable modules. `quoted` mirrors
    /// how the author spelled the path so suggestions match their style.
    ImportPath { quoted: bool },
    /// `import <caret> from some.module` — offer that module's exports.
    ImportName { path: String },
}

struct Found {
    ctx: Ctx,
    scope: Vec<Visible>,
    class: Option<String>,
}

/// Descends the tree to the sentinel, maintaining the scope stack.
struct Walk {
    scope: Vec<Visible>,
    class: Option<String>,
    found: Option<Found>,
}

impl Walk {
    fn run(module: &Module) -> Option<Found> {
        let mut w = Walk {
            scope: Vec::new(),
            class: None,
            found: None,
        };
        w.block(&module.stmts);
        w.found
    }

    fn record(&mut self, ctx: Ctx) {
        if self.found.is_none() {
            self.found = Some(Found {
                ctx,
                scope: self
                    .scope
                    .iter()
                    .map(|v| Visible {
                        name: v.name.clone(),
                        ty: v.ty.clone(),
                        init: v.init.clone(),
                        kind: v.kind,
                    })
                    .collect(),
                class: self.class.clone(),
            });
        }
    }

    fn bind(&mut self, name: &str, ty: Option<Type>, kind: &'static str) {
        self.bind_init(name, ty, None, kind);
    }

    fn bind_init(
        &mut self,
        name: &str,
        ty: Option<Type>,
        init: Option<Expr>,
        kind: &'static str,
    ) {
        if name != SENTINEL {
            self.scope.push(Visible {
                name: name.to_string(),
                ty,
                init,
                kind,
            });
        }
    }

    /// Walk a block, adding each statement's bindings only *after* visiting it
    /// — so a `local` is not visible inside its own initialiser, and names
    /// declared later in the block are not offered at the caret.
    fn block(&mut self, stmts: &[Spanned<Stmt>]) {
        let mark = self.scope.len();
        for s in stmts {
            // A bare sentinel statement means the caret starts a statement.
            if let Stmt::Expr(e) = &s.value {
                if matches!(&e.value, Expr::Ident(n) if n == SENTINEL) {
                    self.record(Ctx::Value { stmt_start: true });
                    continue;
                }
            }
            self.stmt(&s.value);
            self.declare(&s.value);
        }
        self.scope.truncate(mark);
    }

    /// Add the bindings a statement introduces into the enclosing block.
    fn declare(&mut self, s: &Stmt) {
        match s {
            Stmt::Local { name, ty, value, .. } => self.bind_init(
                name,
                ty.clone(),
                value.as_ref().map(|v| v.value.clone()),
                "local",
            ),
            Stmt::LocalMulti { names, values } => {
                // Only pair names with values when the arity matches; a
                // multi-return call feeding several names is not 1:1.
                let paired = names.len() == values.len();
                for (i, (n, _, ty)) in names.iter().enumerate() {
                    let init = paired.then(|| values[i].value.clone());
                    self.bind_init(n, ty.clone(), init, "local");
                }
            }
            _ => {}
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Local { ty, value, .. } => {
                self.ty(ty.as_ref());
                self.opt_expr(value.as_ref());
            }
            Stmt::LocalMulti { names, values } => {
                for (_, _, ty) in names {
                    self.ty(ty.as_ref());
                }
                self.exprs(values);
            }
            Stmt::Assign { target, value } => {
                self.expr(target);
                self.expr(value);
            }
            Stmt::AssignMulti { targets, values } => {
                self.exprs(targets);
                self.exprs(values);
            }
            Stmt::Expr(e) => self.expr(e),
            Stmt::Return(es) => self.exprs(es),
            Stmt::Throw(e) => self.expr(e),
            Stmt::If {
                cond,
                then_block,
                elseifs,
                else_block,
            } => {
                self.expr(cond);
                self.block(then_block);
                for (c, b) in elseifs {
                    self.expr(c);
                    self.block(b);
                }
                if let Some(b) = else_block {
                    self.block(b);
                }
            }
            Stmt::While { cond, body } => {
                self.expr(cond);
                self.block(body);
            }
            Stmt::Repeat { body, cond } => {
                self.block(body);
                self.expr(cond);
            }
            Stmt::ForNumeric {
                var,
                var_ty,
                from,
                to,
                step,
                body,
            } => {
                self.ty(var_ty.as_ref());
                self.expr(from);
                self.expr(to);
                self.opt_expr(step.as_ref());
                let mark = self.scope.len();
                self.bind(var, var_ty.clone(), "loop variable");
                self.block(body);
                self.scope.truncate(mark);
            }
            Stmt::ForIn { vars, iter, body } => {
                for (_, ty) in vars {
                    self.ty(ty.as_ref());
                }
                self.expr(iter);
                let mark = self.scope.len();
                for (n, ty) in vars {
                    self.bind(n, ty.clone(), "loop variable");
                }
                self.block(body);
                self.scope.truncate(mark);
            }
            Stmt::Try {
                body,
                catch_var,
                catch_ty,
                catch_body,
            } => {
                self.block(body);
                self.ty(Some(catch_ty));
                let mark = self.scope.len();
                self.bind(catch_var, Some(catch_ty.clone()), "caught error");
                self.block(catch_body);
                self.scope.truncate(mark);
            }
            Stmt::Decl(d) => self.decl(&d.value),
            Stmt::Break | Stmt::Continue => {}
        }
    }

    fn decl(&mut self, d: &Decl) {
        match d {
            Decl::Function {
                params,
                return_ty,
                body,
                ..
            } => {
                self.params(params);
                self.ty(return_ty.as_ref());
                let mark = self.scope.len();
                for p in params {
                    self.bind(&p.name, Some(p.ty.clone()), "parameter");
                }
                self.block(body);
                self.scope.truncate(mark);
            }
            Decl::Class {
                name,
                extends,
                implements,
                members,
                ..
            } => {
                if extends.as_deref() == Some(SENTINEL) {
                    self.record(Ctx::BaseClass {
                        exclude: vec![name.clone()],
                    });
                }
                if implements.iter().any(|i| i == SENTINEL) {
                    self.record(Ctx::Interfaces {
                        exclude: without_sentinel(implements),
                    });
                }
                let prev = self.class.replace(name.clone());
                for m in members {
                    match &m.value {
                        ClassMember::Field { ty, default, .. } => {
                            self.ty(Some(ty));
                            self.opt_expr(default.as_ref());
                        }
                        ClassMember::Method(method) => {
                            self.params(&method.params);
                            self.ty(method.return_ty.as_ref());
                            let mark = self.scope.len();
                            for p in &method.params {
                                self.bind(&p.name, Some(p.ty.clone()), "parameter");
                            }
                            self.block(&method.body);
                            self.scope.truncate(mark);
                        }
                    }
                }
                self.class = prev;
            }
            Decl::Interface {
                name,
                extends,
                methods,
                ..
            } => {
                if extends.iter().any(|e| e == SENTINEL) {
                    let mut exclude = without_sentinel(extends);
                    exclude.push(name.clone());
                    self.record(Ctx::Interfaces { exclude });
                }
                for m in methods {
                    self.params(&m.params);
                    self.ty(m.return_ty.as_ref());
                }
            }
            Decl::Enum { methods, .. } => {
                for m in methods {
                    self.params(&m.params);
                    self.ty(m.return_ty.as_ref());
                    self.block(&m.body);
                }
            }
            Decl::Import {
                names,
                path,
                quoted,
            } => {
                // The sentinel lands inside the path (bare or quoted) when the
                // caret is choosing a module...
                if path.contains(SENTINEL) {
                    self.record(Ctx::ImportPath { quoted: *quoted });
                } else if let saule_ast::ImportNames::List(items) = names {
                    // ...and in the name list when it is choosing what to pull
                    // in. An `as` alias is a fresh name, so it gets nothing.
                    if items.iter().any(|(n, _)| n == SENTINEL) {
                        self.record(Ctx::ImportName { path: path.clone() });
                    }
                }
            }
        }
    }

    fn params(&mut self, params: &[Param]) {
        for p in params {
            self.ty(Some(&p.ty));
            self.opt_expr(p.default.as_ref());
        }
    }

    /// A sentinel anywhere inside a type annotation means the caret is
    /// writing a type.
    fn ty(&mut self, ty: Option<&Type>) {
        if ty.is_some_and(type_mentions_sentinel) {
            self.record(Ctx::TypeName);
        }
    }

    fn exprs(&mut self, es: &[Spanned<Expr>]) {
        for e in es {
            self.expr(e);
        }
    }

    fn opt_expr(&mut self, e: Option<&Spanned<Expr>>) {
        if let Some(e) = e {
            self.expr(e);
        }
    }

    fn expr(&mut self, e: &Spanned<Expr>) {
        match &e.value {
            Expr::Ident(n) if n == SENTINEL => {
                self.record(Ctx::Value { stmt_start: false });
            }
            Expr::Member { obj, name } | Expr::SafeMember { obj, name } => {
                if name == SENTINEL {
                    self.record(Ctx::Member((**obj).clone()));
                } else {
                    self.expr(obj);
                }
            }
            Expr::MethodCall { obj, method, args } => {
                if method == SENTINEL {
                    self.record(Ctx::Member((**obj).clone()));
                } else {
                    self.expr(obj);
                    self.args(args);
                }
            }
            Expr::Unary { rhs, .. } => self.expr(rhs),
            Expr::ForceUnwrap(inner) => self.expr(inner),
            Expr::Binary { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            Expr::Index { obj, index } => {
                self.expr(obj);
                self.expr(index);
            }
            Expr::Call { callee, args } => {
                self.expr(callee);
                self.args(args);
            }
            Expr::Table(entries) => {
                for entry in entries {
                    match entry {
                        saule_ast::TableEntry::Positional(v) => self.expr(v),
                        saule_ast::TableEntry::Field { value, .. } => self.expr(value),
                    }
                }
            }
            Expr::Lambda { params, body, .. } => {
                self.params(params);
                let mark = self.scope.len();
                for p in params {
                    self.bind(&p.name, Some(p.ty.clone()), "parameter");
                }
                match body {
                    LambdaBody::Expr(e) => self.expr(e),
                    LambdaBody::Block(b) => self.block(b),
                }
                self.scope.truncate(mark);
            }
            Expr::Match { scrutinee, arms } => {
                self.expr(scrutinee);
                for arm in arms {
                    let mark = self.scope.len();
                    bind_pattern(self, &arm.pattern.value);
                    if let Some(g) = &arm.guard {
                        self.expr(g);
                    }
                    match &arm.body {
                        MatchBody::Expr(e) => self.expr(e),
                        MatchBody::Block(b) => self.block(b),
                    }
                    self.scope.truncate(mark);
                }
            }
            Expr::Pipe { source, stages } => {
                self.expr(source);
                for st in stages {
                    self.args(&st.args);
                }
            }
            _ => {}
        }
    }

    fn args(&mut self, args: &[CallArg]) {
        for a in args {
            match a {
                CallArg::Positional(e) => self.expr(e),
                CallArg::Named { value, .. } => self.expr(value),
            }
        }
    }
}

/// Patterns bind names inside their arm (`case Event.Key(code) then …`).
fn bind_pattern(w: &mut Walk, p: &saule_ast::Pattern) {
    use saule_ast::Pattern as P;
    match p {
        P::Bind(n) => w.bind(n, None, "pattern binding"),
        P::Variant { fields, .. } => {
            for f in fields {
                bind_pattern(w, &f.value);
            }
        }
        P::Tuple(items) => {
            for i in items {
                bind_pattern(w, &i.value);
            }
        }
        _ => {}
    }
}

/// The names in a header list the author has already committed to.
fn without_sentinel(names: &[String]) -> Vec<String> {
    names.iter().filter(|n| *n != SENTINEL).cloned().collect()
}

fn type_mentions_sentinel(ty: &Type) -> bool {
    match ty {
        Type::Named(n) => n == SENTINEL,
        Type::Nullable(inner) => type_mentions_sentinel(inner),
        Type::Table { key, value } => {
            key.as_ref().is_some_and(|k| type_mentions_sentinel(k))
                || type_mentions_sentinel(value)
        }
        Type::Tuple(items) => items.iter().any(type_mentions_sentinel),
        Type::Function { params, ret } => {
            params.iter().any(type_mentions_sentinel) || type_mentions_sentinel(ret)
        }
    }
}

// ─── receiver inference ─────────────────────────────────────────────────────

/// What a `receiver.` expression turned out to be.
enum Recv {
    /// An instance — offer public instance members.
    Instance(String),
    /// A class used by name — offer public statics.
    Static(String),
    /// `self` — offer everything, including private members.
    SelfClass(String),
    /// A stdlib module or native-package class (`Math`, `Window`, …).
    Module(String),
    /// An enum type — offer its variants.
    Enum(String),
}

fn infer(expr: &Expr, found: &Found) -> Option<Recv> {
    infer_d(expr, found, 0)
}

/// Chasing un-annotated bindings (`local a = b`, `local b = Player(…)`) can
/// hop from one initialiser to the next, so cap the depth.
const MAX_INFER_DEPTH: usize = 8;

fn infer_d(expr: &Expr, found: &Found, depth: usize) -> Option<Recv> {
    if depth > MAX_INFER_DEPTH {
        return None;
    }
    match expr {
        // `self` is a keyword with its own node, not an identifier.
        Expr::Self_ => found.class.clone().map(Recv::SelfClass),
        Expr::Ident(n) => {
            // `self` is also legal as an explicit parameter name.
            if n == "self" {
                return found.class.clone().map(Recv::SelfClass);
            }
            // A binding in scope shadows any global of the same name.
            if let Some(v) = found.scope.iter().rev().find(|v| &v.name == n) {
                if let Some(c) = v.ty.as_ref().and_then(class_of) {
                    return Some(Recv::Instance(c));
                }
                // Un-annotated: infer from what it was initialised with.
                return v
                    .init
                    .as_ref()
                    .and_then(|e| infer_d(e, found, depth + 1));
            }
            if with_classes(|r| r.contains_key(n)) {
                return Some(Recv::Static(n.clone()));
            }
            if with_enums(|r| r.contains_key(n)) {
                return Some(Recv::Enum(n.clone()));
            }
            if sigs::is_module(n) {
                return Some(Recv::Module(n.clone()));
            }
            None
        }
        // `a.b.` / `self.player.` — the field's declared type carries on.
        Expr::Member { obj, name } | Expr::SafeMember { obj, name } => {
            let owner = owner_class_d(&obj.value, found, depth + 1)?;
            lookup_field_type(&owner, name)
                .as_ref()
                .and_then(class_of)
                .map(Recv::Instance)
        }
        // `Player("x").` / `make().` — the callee's return type.
        Expr::Call { callee, .. } => match &callee.value {
            Expr::Ident(n) if with_classes(|r| r.contains_key(n)) => {
                Some(Recv::Instance(n.clone()))
            }
            Expr::Ident(n) => sig_return(&sigs::lookup(n)?).and_then(named).map(Recv::Instance),
            Expr::Member { obj, name } => match infer_d(&obj.value, found, depth + 1)? {
                Recv::Module(m) => sig_return(&sigs::lookup(&format!("{m}.{name}"))?)
                    .and_then(named)
                    .map(Recv::Instance),
                Recv::Instance(c) | Recv::Static(c) | Recv::SelfClass(c) => lookup_method(&c, name)?
                    .return_ty
                    .as_ref()
                    .and_then(class_of)
                    .map(Recv::Instance),
                Recv::Enum(_) => None,
            },
            _ => None,
        },
        Expr::MethodCall { obj, method, .. } => {
            let owner = owner_class_d(&obj.value, found, depth + 1)?;
            lookup_method(&owner, method)?
                .return_ty
                .as_ref()
                .and_then(class_of)
                .map(Recv::Instance)
        }
        // `maybe!.` — force-unwrap keeps the underlying type.
        Expr::ForceUnwrap(inner) => infer_d(&inner.value, found, depth + 1),
        _ => None,
    }
}

/// The class that owns members reached through `expr`.
fn owner_class_d(expr: &Expr, found: &Found, depth: usize) -> Option<String> {
    match infer_d(expr, found, depth)? {
        Recv::Instance(c) | Recv::Static(c) | Recv::SelfClass(c) => Some(c),
        _ => None,
    }
}

/// A type's class name, seeing through `T?`.
fn class_of(ty: &Type) -> Option<String> {
    match ty {
        Type::Named(n) => Some(n.clone()),
        Type::Nullable(inner) => class_of(inner),
        _ => None,
    }
}

fn sig_return(sig: &NativeSig) -> Option<Type> {
    sig.returns.first().cloned()
}

fn named(ty: Type) -> Option<String> {
    class_of(&ty)
}

// ─── candidates ─────────────────────────────────────────────────────────────

fn member_items(recv: &Spanned<Expr>, found: &Found) -> Vec<CompletionItem> {
    match infer(&recv.value, found) {
        Some(Recv::SelfClass(c)) => class_members(&c, Visibility::IncludePrivate, MemberSet::All),
        Some(Recv::Instance(c)) => class_members(&c, Visibility::PublicOnly, MemberSet::Instance),
        Some(Recv::Static(c)) => class_members(&c, Visibility::PublicOnly, MemberSet::Static),
        Some(Recv::Module(m)) => module_members(&m),
        Some(Recv::Enum(e)) => enum_variants(&e),
        None => Vec::new(),
    }
}

#[derive(PartialEq)]
enum Visibility {
    PublicOnly,
    IncludePrivate,
}

#[derive(PartialEq)]
enum MemberSet {
    All,
    Instance,
    Static,
}

/// Members of `class` and its ancestors, filtered by visibility and by
/// whether the access was through an instance or the class itself.
fn class_members(class: &str, vis: Visibility, set: MemberSet) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut current = Some(class.to_string());

    while let Some(cls) = current {
        let (members, parent) = with_classes(|r| {
            r.get(&cls)
                .map(|i| (i.members.clone(), i.parent.clone()))
                .unwrap_or_default()
        });

        for (name, is_private) in members {
            if is_private && vis == Visibility::PublicOnly {
                continue;
            }
            if seen.contains(&name) {
                continue;
            }

            let method = lookup_method(&cls, &name);
            // `init` is called as `Player(...)`, never as a member.
            if name == "init" {
                continue;
            }
            if let Some(sig) = &method {
                let wanted = match set {
                    MemberSet::All => true,
                    MemberSet::Instance => !sig.is_static,
                    MemberSet::Static => sig.is_static,
                };
                if !wanted {
                    continue;
                }
            }
            seen.push(name.clone());

            let detail = match &method {
                Some(sig) => render_method_sig(&name, sig),
                None => lookup_field_type(&cls, &name)
                    .as_ref()
                    .map(render_type)
                    .unwrap_or_else(|| "field".into()),
            };
            let kind = if method.is_some() {
                CompletionItemKind::METHOD
            } else {
                CompletionItemKind::FIELD
            };
            let doc = (cls != class).then(|| format!("inherited from `{cls}`"));
            items.push(doc_of(item(name, kind, Some(detail)), doc));
        }
        current = parent;
    }

    items.sort_by(|a, b| a.label.cmp(&b.label));
    items
}

fn module_members(module: &str) -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = sigs::module_members(module)
        .into_iter()
        .map(|m| {
            let sig = sigs::lookup(&format!("{module}.{m}"));
            let kind = if sig.is_some() {
                CompletionItemKind::METHOD
            } else {
                CompletionItemKind::FIELD
            };
            let detail = sig.map(|s| render_native_sig(&m, &s));
            item(m, kind, detail)
        })
        .collect();
    items.sort_by(|a, b| a.label.cmp(&b.label));
    items
}

fn enum_variants(name: &str) -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = with_enums(|r| {
        r.get(name)
            .map(|e| {
                e.variants
                    .keys()
                    .map(|v| {
                        item(
                            v.clone(),
                            CompletionItemKind::ENUM_MEMBER,
                            Some(format!("{name}.{v}")),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    });
    items.sort_by(|a, b| a.label.cmp(&b.label));
    items
}

/// The names a given module actually exports — so `import <caret> from x`
/// offers exactly what `x` provides, and nothing else.
fn import_name_items(path: &str, dir: Option<&std::path::Path>) -> Vec<CompletionItem> {
    // Native packages describe their surface directly.
    if let Some(pkg) = saule_interpreter::native_packages::lookup(path) {
        return pkg
            .exports
            .iter()
            .map(|n| {
                item(
                    (*n).to_string(),
                    CompletionItemKind::CLASS,
                    Some("native package export".into()),
                )
            })
            .collect();
    }
    if saule_interpreter::dynamic_packages::is_dynamic_package(path) {
        return saule_interpreter::dynamic_packages::export_names(path)
            .into_iter()
            .map(|n| {
                item(
                    n,
                    CompletionItemKind::CLASS,
                    Some("native package class".into()),
                )
            })
            .collect();
    }

    let Some(dir) = dir else { return Vec::new() };
    let Some(abs) = saule_interpreter::module::resolve_import_path(dir, path) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    let mut seen = Vec::new();
    collect_exports(&abs, 0, &mut out, &mut seen);
    out.sort_by(|a, b| a.label.cmp(&b.label));
    out
}

/// Exported declarations of the module at `abs`. An `init.sau` barrel
/// re-exports whatever it imports, so those targets are followed too.
fn collect_exports(
    abs: &std::path::Path,
    depth: usize,
    out: &mut Vec<CompletionItem>,
    seen: &mut Vec<String>,
) {
    if depth > 4 || seen.iter().any(|s| s == &abs.display().to_string()) {
        return;
    }
    seen.push(abs.display().to_string());

    let Ok(src) = std::fs::read_to_string(abs) else {
        return;
    };
    let Some(module) = parse(&src) else { return };

    for stmt in &module.stmts {
        let Stmt::Decl(d) = &stmt.value else { continue };
        let (name, kind) = match &d.value {
            Decl::Function {
                exported: true,
                name,
                params,
                return_ty,
                ..
            } => {
                out.push(item(
                    name.clone(),
                    CompletionItemKind::FUNCTION,
                    Some(render_fn_sig(name, params, return_ty.as_ref())),
                ));
                continue;
            }
            Decl::Class {
                exported: true,
                name,
                ..
            } => (name, CompletionItemKind::CLASS),
            Decl::Interface {
                exported: true,
                name,
                ..
            } => (name, CompletionItemKind::INTERFACE),
            Decl::Enum {
                exported: true,
                name,
                ..
            } => (name, CompletionItemKind::ENUM),
            // A barrel publishes what it imports; follow those.
            Decl::Import { path, .. } => {
                if abs.file_stem().and_then(|s| s.to_str()) == Some("init") {
                    if let Some(parent) = abs.parent() {
                        if let Some(next) =
                            saule_interpreter::module::resolve_import_path(parent, path)
                        {
                            collect_exports(&next, depth + 1, out, seen);
                        }
                    }
                }
                continue;
            }
            _ => continue,
        };
        out.push(item(name.clone(), kind, Some(format!("{kind:?}").to_lowercase())));
    }
}

/// Classes usable as a base class. A class can't extend itself, and can't
/// extend one of its own descendants either — that would close a cycle in the
/// chain — so both are left out.
fn class_items(exclude: &[String]) -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = with_classes(|r| {
        r.iter()
            .filter(|(name, _)| !exclude.iter().any(|e| descends_from(r, name, e)))
            .map(|(name, info)| {
                let detail = match &info.parent {
                    Some(p) => format!("class extends {p}"),
                    None => "class".to_string(),
                };
                item(name.clone(), CompletionItemKind::CLASS, Some(detail))
            })
            .collect()
    });
    items.sort_by(|a, b| a.label.cmp(&b.label));
    items
}

/// Is `class` `ancestor`, or one of its descendants?
fn descends_from(reg: &ClassRegistry, class: &str, ancestor: &str) -> bool {
    let mut cur = Some(class.to_string());
    let mut hops = 0;
    while let Some(name) = cur {
        if name == ancestor {
            return true;
        }
        // A pre-existing cycle in the registry would otherwise spin forever.
        hops += 1;
        if hops > MAX_INHERITANCE_DEPTH {
            return false;
        }
        cur = reg.get(&name).and_then(|i| i.parent.clone());
    }
    false
}

const MAX_INHERITANCE_DEPTH: usize = 64;

/// Interfaces, minus the ones already named in the header — and, for
/// `interface X extends`, minus anything that already extends `X`.
fn interface_items(exclude: &[String]) -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = with_interfaces(|r| {
        r.iter()
            .filter(|(name, _)| !exclude.iter().any(|e| interface_extends(name, e)))
            .map(|(name, extends)| {
                let detail = if extends.is_empty() {
                    "interface".to_string()
                } else {
                    format!("interface extends {}", extends.join(", "))
                };
                item(name.clone(), CompletionItemKind::INTERFACE, Some(detail))
            })
            .collect()
    });
    items.sort_by(|a, b| a.label.cmp(&b.label));
    items
}

/// Only type names — never values or keywords.
fn type_items() -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = PRIMITIVES
        .iter()
        .map(|t| {
            item(
                (*t).to_string(),
                CompletionItemKind::KEYWORD,
                Some("primitive".into()),
            )
        })
        .collect();

    with_classes(|r| {
        for n in r.keys() {
            items.push(item(n.clone(), CompletionItemKind::CLASS, Some("class".into())));
        }
    });
    with_interfaces(|r| {
        for n in r.keys() {
            items.push(item(
                n.clone(),
                CompletionItemKind::INTERFACE,
                Some("interface".into()),
            ));
        }
    });
    with_enums(|r| {
        for n in r.keys() {
            items.push(item(n.clone(), CompletionItemKind::ENUM, Some("enum".into())));
        }
    });
    items
}

/// Values usable at the caret: bindings in scope first, then the enclosing
/// class's own members, then module-level and imported names.
fn value_items(found: &Found, module: &Module, stmt_start: bool) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    // Innermost bindings win, so walk the stack in reverse and skip shadowed
    // names.
    let mut seen: Vec<String> = Vec::new();
    for v in found.scope.iter().rev() {
        if seen.contains(&v.name) {
            continue;
        }
        seen.push(v.name.clone());
        let detail = v
            .ty
            .as_ref()
            .map(|t| format!("{}: {}", v.kind, render_type(t)))
            .unwrap_or_else(|| v.kind.to_string());
        items.push(sorted(
            item(v.name.clone(), CompletionItemKind::VARIABLE, Some(detail)),
            "0",
        ));
    }

    // Inside a method every class member is reachable by bare name.
    if let Some(class) = &found.class {
        items.push(sorted(
            item(
                "self".into(),
                CompletionItemKind::KEYWORD,
                Some(format!("the current {class}")),
            ),
            "0",
        ));
        for i in class_members(class, Visibility::IncludePrivate, MemberSet::All) {
            items.push(sorted(i, "1"));
        }
    }

    // Module-level functions declared in this file.
    for stmt in &module.stmts {
        if let Stmt::Decl(d) = &stmt.value {
            if let Decl::Function {
                name,
                params,
                return_ty,
                ..
            } = &d.value
            {
                if name != SENTINEL {
                    items.push(sorted(
                        item(
                            name.clone(),
                            CompletionItemKind::FUNCTION,
                            Some(render_fn_sig(name, params, return_ty.as_ref())),
                        ),
                        "2",
                    ));
                }
            }
        }
    }

    // Classes and enums in scope (this file plus imports, courtesy of the
    // seeded registries). A class name doubles as its constructor.
    with_classes(|r| {
        for n in r.keys() {
            let detail = lookup_method(n, "init")
                .map(|s| render_method_sig(n, &s))
                .unwrap_or_else(|| "class".into());
            items.push(sorted(
                item(n.clone(), CompletionItemKind::CLASS, Some(detail)),
                "3",
            ));
        }
    });
    with_enums(|r| {
        for n in r.keys() {
            items.push(sorted(
                item(n.clone(), CompletionItemKind::ENUM, Some("enum".into())),
                "3",
            ));
        }
    });

    // Names the auto-prelude packages contribute (`Math`, `Io`, `IoMode`, …).
    // Taken from the packages themselves rather than a hardcoded list, so this
    // can't drift as the stdlib grows.
    for pkg in saule_interpreter::native_packages::all() {
        if !pkg.auto_prelude {
            continue;
        }
        for name in pkg.exports {
            items.push(sorted(
                item(
                    (*name).to_string(),
                    CompletionItemKind::MODULE,
                    Some("stdlib".into()),
                ),
                "4",
            ));
        }
    }

    for name in saule_interpreter::stdlib::all_prelude_names() {
        let detail = sigs::lookup(name)
            .map(|s| render_native_sig(name, &s))
            .unwrap_or_else(|| "prelude".into());
        items.push(sorted(
            item(name.to_string(), CompletionItemKind::FUNCTION, Some(detail)),
            "4",
        ));
    }

    // Keywords only where a statement can begin — offering `end` or `then`
    // in the middle of an expression is just noise.
    if stmt_start {
        for kw in STATEMENT_KEYWORDS {
            items.push(sorted(
                item((*kw).to_string(), CompletionItemKind::KEYWORD, None),
                "5",
            ));
        }
    }

    dedup(items)
}

// ─── rendering ──────────────────────────────────────────────────────────────

fn render_native_sig(name: &str, sig: &NativeSig) -> String {
    let mut params: Vec<String> = sig.params.iter().map(render_type).collect();
    if let Some(v) = &sig.variadic {
        params.push(format!("...{}", render_type(v)));
    }
    let ret = if sig.returns.is_empty() {
        "nil".to_string()
    } else {
        sig.returns
            .iter()
            .map(render_type)
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!("fn {name}({}) -> {ret}", params.join(", "))
}

fn render_method_sig(name: &str, sig: &MethodSig) -> String {
    let params: Vec<String> = sig
        .params
        .iter()
        .map(|p| format!("{}: {}", p.name, render_type(&p.ty)))
        .collect();
    let ret = sig
        .return_ty
        .as_ref()
        .map(render_type)
        .unwrap_or_else(|| "nil".into());
    let kw = if sig.is_static { "static fn" } else { "fn" };
    format!("{kw} {name}({}) -> {ret}", params.join(", "))
}

fn render_fn_sig(name: &str, params: &[Param], ret: Option<&Type>) -> String {
    let ps: Vec<String> = params
        .iter()
        .map(|p| format!("{}: {}", p.name, render_type(&p.ty)))
        .collect();
    let r = ret.map(render_type).unwrap_or_else(|| "nil".into());
    format!("fn {name}({}) -> {r}", ps.join(", "))
}

// ─── item plumbing ──────────────────────────────────────────────────────────

fn item(label: String, kind: CompletionItemKind, detail: Option<String>) -> CompletionItem {
    CompletionItem {
        sort_text: Some(format!("1{label}")),
        label,
        kind: Some(kind),
        detail,
        ..Default::default()
    }
}

fn sorted(mut i: CompletionItem, bucket: &str) -> CompletionItem {
    i.sort_text = Some(format!("{bucket}{}", i.label));
    i
}

fn doc_of(mut i: CompletionItem, doc: Option<String>) -> CompletionItem {
    if let Some(d) = doc {
        i.documentation = Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: d,
        }));
    }
    i
}

/// The client filters too, but trimming here keeps the payload small.
fn filter(items: Vec<CompletionItem>, prefix: &str) -> Vec<CompletionItem> {
    if prefix.is_empty() {
        return items;
    }
    let lower = prefix.to_lowercase();
    items
        .into_iter()
        .filter(|i| i.label.to_lowercase().starts_with(&lower))
        .collect()
}

fn dedup(mut items: Vec<CompletionItem>) -> Vec<CompletionItem> {
    let mut seen: Vec<String> = Vec::new();
    items.retain(|i| {
        if seen.contains(&i.label) {
            false
        } else {
            seen.push(i.label.clone());
            true
        }
    });
    items
}

const PRIMITIVES: &[&str] = &[
    "integer", "float", "string", "boolean", "table", "function", "userdata", "thread", "any",
    "nil",
];

/// Keywords that can begin a statement.
const STATEMENT_KEYWORDS: &[&str] = &[
    "local", "if", "for", "while", "repeat", "return", "break", "continue", "try", "throw", "fn",
    "class", "interface", "enum", "export", "import", "match", "do",
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Complete at the `@` marker. Mirrors the request handler minus the
    /// cross-file seeding, which needs a real workspace.
    fn complete(marked: &str) -> Vec<String> {
        let offset = marked.find('@').expect("no `@` caret marker");
        let src = marked.replace('@', "");
        let (patched, prefix) = splice_sentinel(&src, offset).expect("splice");
        let Some(module) = parse_tolerant(&patched) else {
            return Vec::new();
        };
        let _ = saule_semantic::analyze(&module);
        let Some(found) = Walk::run(&module) else {
            return Vec::new();
        };
        let items = match &found.ctx {
            Ctx::BaseClass { exclude } => class_items(exclude),
            Ctx::Interfaces { exclude } => interface_items(exclude),
            Ctx::TypeName => type_items(),
            _ => Vec::new(),
        };
        filter(items, &prefix)
            .into_iter()
            .map(|i| i.label)
            .collect()
    }

    const DECLS: &str = "\
class Entity end
class Actor extends Entity end
interface Drawable
    fn draw()
end
interface Sized
    fn size() -> integer
end
enum Colour Red, Green end
";

    /// Classes and nothing else — the interfaces and the enum declared above
    /// are not base classes.
    #[test]
    fn extends_offers_classes_only() {
        let got = complete(&format!("{DECLS}class Player extends @"));
        assert!(got.contains(&"Entity".to_string()));
        assert!(got.contains(&"Actor".to_string()));
        for absent in ["Drawable", "Sized", "Colour", "string"] {
            assert!(!got.contains(&absent.to_string()), "{absent} offered");
        }
    }

    #[test]
    fn extends_filters_by_prefix() {
        let got = complete(&format!("{DECLS}class Player extends En@"));
        assert_eq!(got, vec!["Entity"]);
    }

    /// A class can extend neither itself nor one of its own descendants.
    #[test]
    fn extends_excludes_cycles() {
        let got = complete(&format!("{DECLS}class Entity2 extends @\nend\n"));
        assert!(got.contains(&"Actor".to_string()));

        let got = complete(&format!("{DECLS}class Entity extends @"));
        assert!(!got.contains(&"Entity".to_string()));
        assert!(!got.contains(&"Actor".to_string()), "Actor extends Entity");
    }

    /// Interfaces — including the built-in ones — and nothing else. The
    /// classes and the enum declared above must not show up.
    #[test]
    fn implements_offers_interfaces_only() {
        let got = complete(&format!("{DECLS}class Player implements @"));
        assert!(got.contains(&"Drawable".to_string()));
        assert!(got.contains(&"Sized".to_string()));
        assert!(got.contains(&"Iterable".to_string()), "built-in interface");
        for absent in ["Entity", "Actor", "Colour", "string"] {
            assert!(!got.contains(&absent.to_string()), "{absent} offered");
        }
    }

    /// Later entries in the list drop what's already been named.
    #[test]
    fn implements_drops_names_already_listed() {
        let got = complete(&format!("{DECLS}class Player implements Drawable, @"));
        assert!(!got.contains(&"Drawable".to_string()));
        assert!(got.contains(&"Sized".to_string()));
    }

    #[test]
    fn interface_extends_offers_interfaces_minus_self() {
        let got = complete(&format!("{DECLS}interface Widget extends @"));
        assert!(got.contains(&"Drawable".to_string()));
        assert!(got.contains(&"Sized".to_string()));

        let got = complete(&format!("{DECLS}interface Drawable extends @"));
        assert!(!got.contains(&"Drawable".to_string()));
        assert!(got.contains(&"Sized".to_string()));
    }

    /// The header still parses once the body is there.
    #[test]
    fn works_with_a_complete_class_body() {
        let got = complete(&format!(
            "{DECLS}class Player extends @\n    name: string\n    fn go() end\nend\n"
        ));
        assert!(got.contains(&"Entity".to_string()));
        assert!(got.contains(&"Actor".to_string()));
    }

    /// The header keywords themselves are never suggested.
    #[test]
    fn nothing_before_the_keyword() {
        assert!(complete(&format!("{DECLS}class Player @")).is_empty());
    }

    /// Type positions are unaffected — they still see enums and primitives.
    #[test]
    fn type_position_still_offers_everything() {
        let got = complete(&format!("{DECLS}class Player\n    hue: @\nend\n"));
        assert!(got.contains(&"Colour".to_string()));
        assert!(got.contains(&"string".to_string()));
        assert!(got.contains(&"Drawable".to_string()));
    }
}
