//! The AST walk that locates the smallest enclosing call.
//!
//! [`Cx`] descends the tree tracking locals and the enclosing class, and
//! stops at the innermost `Expr::Call` / `Expr::MethodCall` / pipeline
//! stage whose argument span contains the cursor.

use saule_ast::{
    CallArg, ClassMember, Decl, Expr, LambdaBody, MatchBody, Method, Module, Param, Spanned, Stmt,
    TableEntry, Type,
};
use saule_semantic::{lookup_field_type, lookup_method, super_init_target, with_classes};
use std::collections::HashMap;

use super::*;

/// The callee written as it appears in the source: `add`, `Theme.of`,
/// `One.two.three`.
///
/// `None` for anything that isn't a plain chain of names — a call
/// result, an index, a parenthesised expression — where there is no
/// dotted path to show and the resolved owner is the better answer.
pub(crate) fn dotted_path(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Ident(n) => Some(n.clone()),
        Expr::Self_ => Some("self".to_string()),
        Expr::Member { obj, name } => Some(format!("{}.{name}", dotted_path(&obj.value)?)),
        _ => None,
    }
}

pub(crate) struct Cx<'a> {
    /// The document text, for deciding whether a call is written across
    /// lines. Empty when a caller has no source to give.
    pub(super) source: &'a str,
    pub(super) offset: usize,
    pub(super) locals: Vec<Local>,
    pub(super) enclosing_class: Option<String>,
    /// Top-level user functions discovered in the module. Populated
    /// during a single pre-pass before `visit_module` so call sites
    /// resolve regardless of declaration order.
    pub(super) user_fns: HashMap<String, (Vec<Param>, Option<Type>)>,
    /// `(enum, variant) -> fields` for tuple-style variants, which are
    /// called like constructors. Same pre-pass rationale as `user_fns`.
    pub(super) enum_variants: HashMap<(String, String), Vec<Param>>,
    /// Collection filter for [`Cx::record`]. `None` collects the calls
    /// containing the cursor; `Some(r)` collects everything nested
    /// inside `r`.
    pub(super) region: Option<std::ops::Range<usize>>,
    /// Innermost region containing the cursor in which an enclosing
    /// call's parameter list has stopped applying. Calls that opened
    /// outside it are dropped — see [`Cx::note_barrier`].
    pub(super) barrier: Option<std::ops::Range<usize>>,
    pub(super) hits: Vec<CallHit>,
}

pub(crate) struct Local {
    name: String,
    ty: Type,
}

impl Cx<'_> {
    /// Whether the call is written across lines, measured from its
    /// opening paren to the last argument present. Drives the
    /// one-parameter-per-line rendering, so the popup matches the shape
    /// of the call the reader is looking at.
    ///
    /// Deliberately not the whole `args_span`. For a call the user has
    /// only just opened (`w.moveTo(|`), [`repair_parse`] closes it by
    /// appending to the *end of the document*, so that span swallows
    /// every line in between and would report a freshly-typed call as
    /// multi-line. A call with no arguments yet cannot be laid out
    /// across lines, so it never is.
    fn call_spans_lines(&self, args_span: &std::ops::Range<usize>, args: &[CallArgInfo]) -> bool {
        let Some(last) = args.iter().map(|a| a.span.end).max() else {
            return false;
        };
        let end = last.min(self.source.len());
        let start = args_span.start.min(end);
        self.source
            .get(start..end)
            .is_some_and(|s| s.contains('\n'))
    }

    /// The callee as the reader wrote it, with a leading `self`
    /// replaced by the class it stands for: `Theme.of`,
    /// `One.two.three`, and `Panel.reset` rather than `self.reset`.
    fn callee_display(&self, callee: &Expr) -> Option<String> {
        let path = dotted_path(callee)?;
        let Some(rest) = path.strip_prefix("self.") else {
            return Some(path);
        };
        match &self.enclosing_class {
            Some(class) => Some(format!("{class}.{rest}")),
            None => Some(path),
        }
    }

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

    /// Remember a region where the enclosing call's parameter list has
    /// stopped applying, when the cursor is inside it.
    ///
    /// `Column(children: {TextField(onChanged: fn(text: string)` opens
    /// three things that all stay lexically open until the very end of
    /// the expression, so a caret parked far inside is technically still
    /// within every one of them. Positionally the enclosing calls match;
    /// syntactically they are long since done saying anything useful.
    /// Two constructs end the conversation:
    ///
    /// * **A block-bodied lambda** (`fn(...) ... end`) — the caret is
    ///   writing statements in a new scope that runs to its own `end`,
    ///   not filling in an argument. The whole lambda counts, parameter
    ///   list included: while typing `fn(next: boolean)` you are
    ///   declaring the callback's parameters, and the enclosing widget
    ///   has nothing to say about those either. A `=>` lambda's body is
    ///   a single expression, still visibly one of the arguments, so
    ///   `map(xs, s => #s)` keeps reporting `map`.
    ///
    /// * **A table literal** (`{ ... }`) — its contents are data. The
    ///   caret between two entries of `children: {…}` is not positioned
    ///   at any parameter, and answering with the full `Column(...)`
    ///   list there describes a slot the reader already filled.
    ///
    /// Both are barriers only for calls that opened *before* them. A
    /// call written inside the region resolves normally — `Text(` inside
    /// a `children` table reports `Text`, which is the whole point.
    fn note_barrier(&mut self, region: std::ops::Range<usize>) {
        if !contains(&region, self.offset) {
            return;
        }
        // Deeper regions are visited later, but nesting order alone
        // isn't enough — a lambda inside a table and a table inside a
        // lambda both occur. Keep the one that starts latest, which is
        // the innermost containing the cursor either way.
        let narrower = self
            .barrier
            .as_ref()
            .is_none_or(|cur| region.start >= cur.start);
        if narrower {
            self.barrier = Some(region);
        }
    }

    pub(super) fn visit_module(&mut self, module: &Module) {
        for s in &module.stmts {
            self.visit_stmt(s);
        }
    }

    fn visit_stmt(&mut self, s: &Spanned<Stmt>) {
        match &s.value {
            Stmt::Local {
                name, ty, value, ..
            } => {
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
                    if let ClassMember::Field {
                        default: Some(d), ..
                    } = &m.value
                    {
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
            Expr::Cast { value, .. } => self.visit_expr(value),
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
                    let args_span = args_span(&callee.span, args, e.span.end);
                    let arg_infos = build_arg_infos(args);
                    self.record(CallHit {
                        callee: callee_ref,
                        enclosing_class: self.enclosing_class.clone(),
                        user_fn,
                        local_fn,
                        display: self.callee_display(&callee.value),
                        multiline: self.call_spans_lines(&args_span, &arg_infos),
                        args: arg_infos,
                        args_span,
                    });
                }
            }
            Expr::MethodCall { obj, method, args } => {
                self.visit_expr(obj);
                for a in args {
                    visit_arg(self, a);
                }
                if let Some(class) = self.receiver_class(&obj.value) {
                    let args_span = method_args_span(&obj.span, method, e.span.end);
                    // `obj:method(...)` — the receiver's own path, then
                    // the method, so a chain reads the way it was typed.
                    let display = self
                        .callee_display(&obj.value)
                        .map(|recv| format!("{recv}.{method}"));
                    let arg_infos = build_arg_infos(args);
                    self.record(CallHit {
                        callee: CalleeRef::Method {
                            class,
                            name: method.clone(),
                        },
                        enclosing_class: self.enclosing_class.clone(),
                        user_fn: None,
                        local_fn: None,
                        display,
                        multiline: self.call_spans_lines(&args_span, &arg_infos),
                        args: arg_infos,
                        args_span,
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
                    let args_span = args_start.min(st.span.end)..st.span.end;
                    let arg_infos = build_arg_infos(&st.args);
                    self.record(CallHit {
                        callee: CalleeRef::PipeStage(st.name.clone()),
                        enclosing_class: self.enclosing_class.clone(),
                        user_fn: self.user_fns.get(&st.name).cloned(),
                        local_fn: None,
                        // A stage's name is its own display; the piped
                        // receiver is upstream, not part of the callee.
                        display: None,
                        multiline: self.call_spans_lines(&args_span, &arg_infos),
                        args: arg_infos,
                        args_span,
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
                self.note_barrier(e.span.clone());
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
                if matches!(body, LambdaBody::Block(_)) {
                    self.note_barrier(e.span.clone());
                }
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
                    && let Some(fields) = self.enum_variants.get(&(enum_name.clone(), name.clone()))
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
pub(crate) fn class_of(ty: &Type) -> Option<String> {
    match ty {
        Type::Named(n) => Some(n.clone()),
        Type::Nullable(inner) => class_of(inner),
        _ => None,
    }
}

pub(crate) fn visit_arg(cx: &mut Cx, a: &CallArg) {
    match a {
        CallArg::Positional(e) | CallArg::Named { value: e, .. } => cx.visit_expr(e),
    }
}

pub(crate) fn build_arg_infos(args: &[CallArg]) -> Vec<CallArgInfo> {
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
pub(crate) fn args_span(
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
pub(crate) fn method_args_span(
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
pub(crate) fn contains(span: &std::ops::Range<usize>, offset: usize) -> bool {
    offset > span.start && offset < span.end
}

/// Pre-pass: collect every top-level `fn name(...)` declaration so the
/// signature-help walker can resolve free-call expressions whose target
/// is a user-defined function (not a class init, not a stdlib native).
pub(crate) fn collect_user_fns(module: &Module) -> HashMap<String, (Vec<Param>, Option<Type>)> {
    let mut out = HashMap::new();
    for s in &module.stmts {
        if let Stmt::Decl(d) = &s.value
            && let Decl::Function {
                name,
                params,
                return_ty,
                ..
            } = &d.value
        {
            out.insert(name.clone(), (params.clone(), return_ty.clone()));
        }
    }
    out
}

/// Pre-pass: collect every tuple-style enum variant's fields so
/// `Enum.Variant(...)` calls resolve like constructors.
pub(crate) fn collect_enum_variants(module: &Module) -> HashMap<(String, String), Vec<Param>> {
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
