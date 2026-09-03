//! Locating the cursor and deciding what kind of completion the
//! position calls for (member access, type position, import name,
//! or a bare identifier).

use saule_ast::{
    CallArg, ClassMember, Decl, EnumVariant, Expr, LambdaBody, MatchBody, Module, Param, Spanned,
    Stmt, Type,
};

use super::*;

/// A binding visible at the caret.
pub(crate) struct Visible {
    pub(crate) name: String,
    /// Declared type, when the binding was annotated.
    pub(crate) ty: Option<Type>,
    /// Initialiser expression, kept so an un-annotated `local p = Player(…)`
    /// can still be resolved — type annotations are optional in Saule, so
    /// relying on `ty` alone would miss most real code.
    pub(crate) init: Option<Expr>,
    pub(crate) kind: &'static str,
}

pub(crate) enum Ctx {
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
    /// A class body, where a member is beginning. The modifiers already
    /// written are excluded — `static <caret>` has no second `static` to
    /// offer.
    ClassMember { is_static: bool, is_private: bool },
    /// `export <caret>` — only the four declaration keywords `export` can
    /// introduce. A bare identifier there parses as the start of an exported
    /// module variable, which is a name the author is inventing, so nothing
    /// else is worth offering.
    AfterExport,
    /// `case <caret>` — a match arm's pattern. `scrutinee` is what the
    /// `match` is over, so the variants of *its* enum can lead the list.
    Pattern { scrutinee: Option<Spanned<Expr>> },
    /// `case Colour.<caret>` — the enum is already named, so only its own
    /// variants belong here.
    VariantName { enum_name: String },
    /// `import * from <caret>` — offer importable modules. `quoted` mirrors
    /// how the author spelled the path so suggestions match their style.
    ImportPath { quoted: bool },
    /// `import <caret> from some.module` — offer that module's exports.
    ImportName { path: String },
}

pub(crate) struct Found {
    pub(crate) ctx: Ctx,
    pub(crate) scope: Vec<Visible>,
    pub(crate) class: Option<String>,
    /// Parameters of the call the caret sits directly inside, minus the
    /// ones already supplied — offered as `name: ` so writing an
    /// argument can complete the keyword rather than only the value.
    ///
    /// Empty unless the caret is a *positional* argument: inside
    /// `f(child: ca…)` the caret is past a `name:` and is typing a
    /// value, where parameter names are the wrong suggestion.
    pub(crate) named_params: Vec<Param>,
    /// The type the slot under the caret is declared to hold, when the caret
    /// is filling an argument. What the author writes there has to *be* one
    /// of these, so it is the strongest ordering signal completion has —
    /// stronger than how close the name is to what has been typed.
    pub(crate) expected: Option<Type>,
    /// True where a `case` can begin: inside a `match`, at the start of a
    /// statement. An arm body is ordinary statement territory *and* the
    /// place the next arm starts, and the tree cannot tell which one is
    /// being written — so both are offered.
    pub(crate) match_arm: bool,
}

/// Descends the tree to the sentinel, maintaining the scope stack.
pub(crate) struct Walk {
    scope: Vec<Visible>,
    class: Option<String>,
    found: Option<Found>,
    /// Top-level `fn` signatures, so a lambda argument's parameters can be
    /// refined from the slot they fill. An omitted annotation parses as `any`,
    /// and the callee is the only place the real type comes from — without
    /// this, `each(items) do (item)` offers `item: any`.
    user_fns: std::collections::HashMap<String, (Vec<Param>, Option<Type>)>,
    /// Unfilled parameters of the call currently being descended into.
    /// Saved and restored around each argument list, so a nested call
    /// shadows its parent — in `outer(a, inner(b…))` the caret is
    /// offered `inner`'s parameters, not `outer`'s.
    named_params: Vec<Param>,
    /// Declared type of the argument slot currently being descended into.
    /// Saved and restored around each argument the same way, so a nested
    /// call's slot shadows its parent's.
    expected: Option<Type>,
    /// Whether the statement position currently being walked sits inside a
    /// `match`. Saved and restored around each arm body.
    match_arm: bool,
}

impl Walk {
    /// Is `offset` inside an interface body?
    ///
    /// An interface body takes `fn` and nothing else, so a half-typed member
    /// is not a name the parser can keep — it is recorded as an error and
    /// synchronised past, leaving no sentinel in the tree for [`Walk`] to
    /// find. What does survive is the interface itself, spanning the body it
    /// was dropped from, and that is enough to place the caret.
    ///
    /// Only consulted when the walk found nothing, so a caret in a signature
    /// — a parameter's type, a return type — still resolves as itself.
    pub(crate) fn in_interface_body(module: &Module, offset: usize) -> bool {
        module.stmts.iter().any(|s| {
            let Stmt::Decl(d) = &s.value else { return false };
            let Decl::Interface {
                name, type_params, ..
            } = &d.value
            else {
                return false;
            };
            // The header, not the body: the name and the type parameters are
            // the author's to invent.
            if name == SENTINEL || type_params.iter().any(|p| p == SENTINEL) {
                return false;
            }
            d.span.contains(&offset)
        })
    }

    pub(crate) fn run(module: &Module) -> Option<Found> {
        let mut w = Walk {
            scope: Vec::new(),
            class: None,
            found: None,
            user_fns: crate::server::sighelp::walk::collect_user_fns(module),
            named_params: Vec::new(),
            expected: None,
            match_arm: false,
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
                named_params: std::mem::take(&mut self.named_params),
                expected: self.expected.clone(),
                match_arm: self.match_arm,
            });
        }
    }

    fn bind(&mut self, name: &str, ty: Option<Type>, kind: &'static str) {
        self.bind_init(name, ty, None, kind);
    }

    fn bind_init(&mut self, name: &str, ty: Option<Type>, init: Option<Expr>, kind: &'static str) {
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
        let mut after_match = false;
        for s in stmts {
            // A bare sentinel statement means the caret starts a statement.
            if let Stmt::Expr(e) = &s.value
                && matches!(&e.value, Expr::Ident(n) if n == SENTINEL)
            {
                // `match c` with no arm typed yet is not a `match` the parser
                // could finish: it stops at the missing `case` and the caret
                // lands here, as the statement *after* the match rather than
                // inside it. It is still an arm position, and `case` is still
                // the word being reached for.
                let outer = self.match_arm;
                self.match_arm = outer || after_match;
                self.record(Ctx::Value { stmt_start: true });
                self.match_arm = outer;
                continue;
            }
            after_match = matches!(
                &s.value,
                Stmt::Expr(e) if matches!(&e.value, Expr::Match { .. })
            );
            self.stmt(&s.value);
            self.declare(&s.value);
        }
        self.scope.truncate(mark);
    }

    /// Add the bindings a statement introduces into the enclosing block.
    fn declare(&mut self, s: &Stmt) {
        match s {
            Stmt::Local {
                name, ty, value, ..
            } => self.bind_init(
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
            // A recovery hole has no children to walk — but the statements
            // around it still do, which is the point of recovering at all.
            Stmt::Error => {}
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
            Stmt::Assign { target, value } | Stmt::CompoundAssign { target, value, .. } => {
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
                if extends.as_ref().is_some_and(|e| e.name == SENTINEL) {
                    self.record(Ctx::BaseClass {
                        exclude: vec![name.clone()],
                    });
                }
                if implements.iter().any(|i| i.name == SENTINEL) {
                    self.record(Ctx::Interfaces {
                        exclude: without_sentinel(implements),
                    });
                }
                let prev = self.class.replace(name.clone());
                for m in members {
                    match &m.value {
                        ClassMember::Field {
                            is_static,
                            is_private,
                            name,
                            ty,
                            default,
                        } => {
                            // A member that is still only a bare word: the
                            // parser read it as a field name, but it is just
                            // as likely to be a modifier or `fn` the author
                            // hasn't finished typing. The modifiers already
                            // consumed say which are still available.
                            if name == SENTINEL {
                                self.record(Ctx::ClassMember {
                                    is_static: *is_static,
                                    is_private: *is_private,
                                });
                                continue;
                            }
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
                if extends.iter().any(|e| e.name == SENTINEL) {
                    let mut exclude = without_sentinel(extends);
                    exclude.push(name.clone());
                    self.record(Ctx::Interfaces { exclude });
                }
                for m in methods {
                    self.params(&m.params);
                    self.ty(m.return_ty.as_ref());
                }
            }
            Decl::Enum {
                variants, methods, ..
            } => {
                // The payload of a tuple variant is a parameter list like any
                // other, and its annotations are type positions like any
                // other — `Text(value: str…)` wants the same names
                // `local x: str…` does. Walking only `methods` left the
                // variant list as the one place in the language where a type
                // annotation offered nothing: the sentinel landed in a
                // `Param` no arm visited, `Walk::run` found no context, and
                // the request answered `None`.
                for v in variants {
                    match &v.value {
                        EnumVariant::Tuple { fields, .. } => self.params(fields),
                        // `Alive = "aliv…"` — a discriminant is an ordinary
                        // expression, so the caret in one completes values.
                        EnumVariant::Valued(_, value) => self.expr(value),
                        EnumVariant::Bare(_) => {}
                    }
                }
                for m in methods {
                    self.params(&m.params);
                    self.ty(m.return_ty.as_ref());
                    self.block(&m.body);
                }
            }
            // Module variables are in scope for the whole file, so bind the
            // name before walking on — completion inside a later function
            // body should offer it.
            Decl::Variable {
                exported,
                name,
                ty,
                value,
                ..
            } => {
                // `export en…` parses as an exported variable being named,
                // because an identifier is all that distinguishes one from a
                // `fn` / `class` / `interface` / `enum` that hasn't been typed
                // yet. Nothing followed it, so the keyword is what's wanted.
                if *exported && name == SENTINEL && ty.is_none() && value.is_none() {
                    self.record(Ctx::AfterExport);
                    return;
                }
                self.ty(ty.as_ref());
                self.opt_expr(value.as_ref());
                self.bind_init(
                    name,
                    ty.clone(),
                    value.as_ref().map(|v| v.value.clone()),
                    "module variable",
                );
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
            Expr::Unary { rhs, .. } => self.expr(rhs),
            Expr::ForceUnwrap(inner) => self.expr(inner),
            // `x as T` — both halves can hold the caret. The target is a
            // type annotation like any other, so `v as <caret>` wants type
            // names rather than the values `Ctx::Value` would offer.
            Expr::Cast { value, ty, .. } => {
                self.expr(value);
                self.ty(Some(ty));
            }
            Expr::Binary { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            Expr::Index { obj, index } => {
                self.expr(obj);
                self.expr(index);
            }
            Expr::Call { callee, args, .. } => {
                self.expr(callee);
                self.call_args(&callee.value, args);
            }
            Expr::Table(entries) => {
                for entry in entries {
                    match entry {
                        saule_ast::TableEntry::Positional(v) => self.expr(v),
                        saule_ast::TableEntry::Field { value, .. } => self.expr(value),
                    }
                }
            }
            Expr::Lambda { params, body, .. } => self.lambda(params, body, None),
            Expr::Match { scrutinee, arms } => {
                self.expr(scrutinee);
                for arm in arms {
                    // The pattern is written before anything binds, so this
                    // comes first: `case Col…` is choosing what to match, not
                    // naming something.
                    match pattern_sentinel(&arm.pattern.value) {
                        Some(PatternHole::Whole) => self.record(Ctx::Pattern {
                            scrutinee: Some((**scrutinee).clone()),
                        }),
                        Some(PatternHole::Variant(enum_name)) => {
                            self.record(Ctx::VariantName { enum_name })
                        }
                        None => {}
                    }
                    let mark = self.scope.len();
                    bind_pattern(self, &arm.pattern.value);
                    if let Some(g) = &arm.guard {
                        self.expr(g);
                    }
                    // An arm body is also where the *next* arm starts, so a
                    // statement position inside one can be a `case`.
                    let outer = std::mem::replace(&mut self.match_arm, true);
                    match &arm.body {
                        MatchBody::Expr(e) => self.expr(e),
                        MatchBody::Block(b) => self.block(b),
                    }
                    self.match_arm = outer;
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

    /// [`Self::args`], but a lambda argument is walked against the parameter
    /// type of the slot it fills — including a trailing block, which takes the
    /// callback slot no other argument claimed.
    fn call_args(&mut self, callee: &Expr, args: &[CallArg]) {
        let Some(params) = self.callee_params(callee) else {
            self.args(args);
            return;
        };
        let param_slots = saule_ast::param_slots(&params);
        let slots = saule_ast::resolve_arg_slots(args, &param_slots);

        // Which parameters are still unspoken for. `resolve_arg_slots`
        // has already assigned every argument — positional by position,
        // named by name — so "unfilled" is just the complement, and it
        // stays right for the mixed forms (`Widget(x, key: k, ba…)`).
        // The caret's own argument doesn't count as filling anything.
        let sentinel_arg = args.iter().position(is_sentinel_positional);
        let filled: Vec<usize> = slots
            .iter()
            .enumerate()
            .filter(|(i, _)| Some(*i) != sentinel_arg)
            .filter_map(|(_, s)| *s)
            .collect();
        let unfilled: Vec<Param> = params
            .iter()
            .enumerate()
            .filter(|(i, _)| !filled.contains(i))
            .map(|(_, p)| p.clone())
            .collect();

        let outer = std::mem::replace(&mut self.named_params, unfilled);
        let outer_expected = self.expected.take();
        for (a, slot) in args.iter().zip(slots.iter()) {
            let (CallArg::Positional(e) | CallArg::Named { value: e, .. }) = a;
            let want = slot.and_then(|i| params.get(i)).map(|p| &p.ty);
            // What this slot is declared to hold, for as long as we are
            // inside it. A positional caret past the last declared parameter
            // has no slot and so no expectation, which is the honest answer.
            self.expected = want.cloned();
            match (&e.value, want) {
                (Expr::Lambda { params, body, .. }, Some(Type::Function { params: want, .. })) => {
                    self.lambda(params, body, Some(want))
                }
                // A named argument's value is a value position — the
                // keyword is already written, so parameter names are
                // not what the caret wants next. Its *type* still is:
                // `alignment: ⟨caret⟩` wants an `Alignment`.
                _ if matches!(a, CallArg::Named { .. }) => {
                    let inner = std::mem::take(&mut self.named_params);
                    self.expr(e);
                    self.named_params = inner;
                }
                _ => self.expr(e),
            }
            self.expected = None;
        }
        self.named_params = outer;
        self.expected = outer_expected;
    }

    /// The callee's declared parameters, for the callee shapes a lambda
    /// argument realistically appears under: a top-level `fn`, a method on the
    /// enclosing class, and a class constructor.
    fn callee_params(&self, callee: &Expr) -> Option<Vec<Param>> {
        match callee {
            Expr::Ident(name) => {
                if saule_semantic::with_classes(|r| r.contains_key(name)) {
                    return saule_semantic::lookup_method(name, "init").map(|s| s.params);
                }
                if let Some(class) = &self.class
                    && let Some(sig) = saule_semantic::lookup_method(class, name)
                {
                    return Some(sig.params);
                }
                self.user_fns.get(name).map(|(ps, _)| ps.clone())
            }
            // `recv.method(`, `Theme.of(` — resolve the receiver, then
            // the method on it. `lookup_method` walks the parent chain,
            // so an inherited modifier answers too.
            Expr::Member { obj, name } | Expr::SafeMember { obj, name } => {
                let class = self.receiver_class(&obj.value)?;
                saule_semantic::lookup_method(&class, name).map(|s| s.params)
            }
            _ => None,
        }
    }

    /// Which class a call's receiver denotes, for the shapes that turn
    /// up in a chain: `self`, a local, a bare class name, a constructor
    /// call, and — so chains resolve past their first link — a method
    /// call, whose class is that method's declared return type.
    fn receiver_class(&self, obj: &Expr) -> Option<String> {
        match obj {
            Expr::Self_ => self.class.clone(),
            Expr::Ident(name) => {
                if let Some(v) = self.scope.iter().rev().find(|v| v.name == *name)
                    && let Some(Type::Named(n)) = &v.ty
                {
                    return Some(n.clone());
                }
                saule_semantic::with_classes(|r| r.contains_key(name)).then(|| name.clone())
            }
            Expr::ForceUnwrap(inner) => self.receiver_class(&inner.value),
            Expr::Call { callee, .. } => match &callee.value {
                Expr::Ident(n) if saule_semantic::with_classes(|r| r.contains_key(n)) => {
                    Some(n.clone())
                }
                Expr::Member { obj, name } | Expr::SafeMember { obj, name } => {
                    let recv = self.receiver_class(&obj.value)?;
                    match saule_semantic::lookup_method(&recv, name)?.return_ty? {
                        Type::Named(n) => Some(n),
                        Type::Nullable(inner) => match *inner {
                            Type::Named(n) => Some(n),
                            _ => None,
                        },
                        _ => None,
                    }
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// Walk a lambda body with its parameters bound. `want` carries the
    /// parameter types the callee declared, which fill in any the writer
    /// omitted — an explicit annotation on the lambda always wins.
    fn lambda(&mut self, params: &[Param], body: &LambdaBody, want: Option<&[Type]>) {
        self.params(params);
        let mark = self.scope.len();
        for (i, p) in params.iter().enumerate() {
            let ty = match want.and_then(|w| w.get(i)) {
                Some(t) if matches!(&p.ty, Type::Named(n) if n == "any") => t.clone(),
                _ => p.ty.clone(),
            };
            self.bind(&p.name, Some(ty), "parameter");
        }
        match body {
            LambdaBody::Expr(e) => self.expr(e),
            LambdaBody::Block(b) => self.block(b),
        }
        self.scope.truncate(mark);
    }
}

/// Patterns bind names inside their arm (`case Event.Key(code) then …`).
/// Where a sentinel turned up inside a match pattern.
pub(crate) enum PatternHole {
    /// `case <caret>` — the whole pattern is still to be chosen.
    Whole,
    /// `case Colour.<caret>` — the enum is named, the variant is not.
    Variant(String),
}

/// Locate the sentinel in `pattern`, if it is there.
///
/// Only the two positions a suggestion can help with. A payload sub-pattern
/// (`case Event.Click(x, <caret>)`) binds a name the author is inventing,
/// which is exactly where completion should stay quiet.
pub(crate) fn pattern_sentinel(pattern: &saule_ast::Pattern) -> Option<PatternHole> {
    match pattern {
        saule_ast::Pattern::Bind(n) if n == SENTINEL => Some(PatternHole::Whole),
        saule_ast::Pattern::Variant {
            enum_name, variant, ..
        } if variant == SENTINEL => Some(PatternHole::Variant(enum_name.clone())),
        _ => None,
    }
}

pub(crate) fn bind_pattern(w: &mut Walk, p: &saule_ast::Pattern) {
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

/// A positional argument that is nothing but the caret — the shape that
/// means "the author is starting a fresh argument here".
fn is_sentinel_positional(arg: &CallArg) -> bool {
    matches!(arg, CallArg::Positional(e) if matches!(&e.value, Expr::Ident(n) if n == SENTINEL))
}
