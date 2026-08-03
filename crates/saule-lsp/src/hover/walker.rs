//! AST walker that picks the smallest enclosing node at a byte offset
//! and produces a Markdown blurb for it. The big stateful machinery
//! (`Cx`, scope tracking, every `visit_*` arm) lives here; rendering
//! helpers live in [`super::render`] and shared utilities in
//! [`super::util`].

use std::collections::HashMap;
use std::ops::Range;

use saule_ast::{
    CallArg, ClassMember, Decl, Expr, Method, Module, Param, Pattern, Spanned, Stmt, Type,
};
use saule_semantic::{
    lookup_field_type, lookup_method, super_init_target, with_classes, with_enums, with_interfaces,
};

use super::ImportContext;
use super::render::{
    collect_enum_variant_fields, render_class_full, render_class_head, render_enum_from_registry,
    render_enum_head, render_enum_variant_decl, render_field, render_function_sig,
    render_interface_from_registry, render_interface_head, render_interface_method,
    render_method_head, render_method_sig, render_native_sig_full, render_param,
    render_stdlib_module, render_type, render_variant_pattern, with_doc, with_param_doc,
};
use super::util::{
    collect_named_heads, contains, is_primitive, locate_word_in, named_type, render_unknown_member,
    resolve_member, strip_nullable_type,
};

/// Collapse an inferred type to the single value it yields in a
/// single-value context: a multi-return tuple becomes its first
/// component, anything else passes through, and `None` becomes `any`.
fn first_value_type(ty: Option<Type>) -> Type {
    match ty {
        Some(Type::Tuple(parts)) => parts
            .into_iter()
            .next()
            .unwrap_or_else(|| Type::Named("any".into())),
        Some(t) => t,
        None => Type::Named("any".into()),
    }
}

/// Drive the hover walker against `module` for `offset` and return the
/// best (smallest-span) blurb we found, if any.
pub(super) fn run(
    module: &Module,
    source: &str,
    offset: usize,
    imports: &ImportContext,
) -> Option<(String, Range<usize>)> {
    let mut cx = Cx {
        source,
        offset,
        enclosing_class: None,
        enclosing_return_ty: None,
        best: None,
        imports,
        locals: Vec::new(),
        enum_variant_fields: collect_enum_variant_fields(module),
        module_fns: collect_module_fns(module),
    };
    cx.visit_module(module);
    cx.best.map(|h| (h.md, h.span))
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
    Binding,
}

struct Cx<'a> {
    source: &'a str,
    offset: usize,
    enclosing_class: Option<String>,
    /// Declared return type of the innermost enclosing
    /// function/method. `None` when outside any function or when the
    /// function omitted its `-> T` annotation. Used to surface
    /// `return` keyword hover.
    enclosing_return_ty: Option<Type>,
    best: Option<Hit>,
    imports: &'a ImportContext,
    locals: Vec<LocalVar>,
    /// Tuple-variant payload field types, keyed by `(enum, variant)`.
    /// Populated once at the start of [`hover_at_with`] so pattern
    /// bindings inside `match` arms can be typed without re-walking
    /// every enum decl per arm.
    enum_variant_fields: HashMap<(String, String), Vec<Param>>,
    /// The free functions this module declares at top level, keyed by
    /// name. Free functions never reach the class / interface / enum
    /// registries, so without this a hover on a *call* to a sibling
    /// function (`map(...)` below `fn map<T, U>`) had nothing to
    /// resolve against and fell through to `None`.
    module_fns: HashMap<String, ModuleFn>,
}

/// A top-level `fn` of the module under the cursor, in the three shapes
/// the walker needs it: rendered for hover, as an AST parameter list for
/// named-argument hover, and as a [`NativeSig`] so the generic
/// instantiation machinery in `saule_typeck::sigs` can bind its type
/// parameters against actual argument types.
struct ModuleFn {
    md: String,
    params: Vec<Param>,
    sig: saule_typeck::sigs::NativeSig,
}

/// A resolved call target, in the shape named-argument hover needs it.
struct CalleeSig {
    /// How the callee reads at the call site — `Container`, `Theme.of`,
    /// `showMenu` — used to qualify the parameter in the popup.
    display: String,
    params: Vec<Param>,
    /// The callee's `---` block, so an `@param child` line reaches the
    /// hover on the `child:` key.
    doc: Option<saule_docs::DocBlock>,
}

/// Index every top-level `fn` in `module` by name. Declaration order
/// wins on duplicates, which matches how the rest of the pipeline
/// treats a redeclared name.
fn collect_module_fns(module: &Module) -> HashMap<String, ModuleFn> {
    let mut out = HashMap::new();
    for stmt in &module.stmts {
        if let Stmt::Decl(d) = &stmt.value
            && let Decl::Function {
                name,
                type_params,
                params,
                return_ty,
                ..
            } = &d.value
        {
            out.entry(name.clone()).or_insert_with(|| ModuleFn {
                md: render_function_sig(name, type_params, params, return_ty.as_ref()),
                params: params.clone(),
                sig: saule_typeck::sigs::NativeSig {
                    type_params: type_params.clone(),
                    params: params.iter().map(|p| p.ty.clone()).collect(),
                    variadic: None,
                    returns: return_ty.clone().into_iter().collect(),
                },
            });
        }
    }
    out
}

impl crate::exprty::TypeSource for Cx<'_> {
    fn type_of(&self, expr: &Expr) -> Option<Type> {
        self.infer_init_type(expr)
    }

    fn stage_sig(&self, name: &str) -> Option<saule_typeck::sigs::NativeSig> {
        self.module_fns
            .get(name)
            .map(|f| f.sig.clone())
            .or_else(|| saule_typeck::sigs::lookup(name))
    }

    fn arg_types(&self, args: &[CallArg]) -> Vec<Option<Type>> {
        self.positional_arg_types(args)
    }
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

    /// The span of a declaration's *name* token, for anchoring the
    /// declaration's own hover.
    ///
    /// Recording a declaration against its full span — header *and*
    /// body — makes every byte inside that body which no narrower node
    /// claims resolve to the declaration's signature: a comment, a blank
    /// line, the gap between two arguments. The popup then presents a
    /// confident description of a symbol the cursor is nowhere near,
    /// which is the single largest source of hovers that read as random.
    /// Anchored to the name token instead, those positions correctly
    /// produce no hover at all.
    ///
    /// The span runs from the start of the declaration to the end of its
    /// name, so the introducing keyword and any `export` / `local` /
    /// `static` modifier hover as part of the thing they introduce —
    /// pointing at `fn` and getting nothing would be its own small
    /// annoyance. It is the *body* that must be excluded, and is.
    ///
    /// `head_end` bounds the search to the declaration header so a body
    /// that happens to mention the name (recursion, a same-named local)
    /// can't win the match. Falls back to the full span when the name
    /// isn't locatable, which is what keeps the source-less [`hover_at`]
    /// entry point answering as it always has.
    fn decl_name_span(&self, span: &Range<usize>, name: &str, head_end: usize) -> Range<usize> {
        let end = head_end.clamp(span.start, span.end);
        match locate_word_in(self.source, &(span.start..end), name) {
            Some(m) => span.start..m.end,
            None => span.clone(),
        }
    }

    /// What `for … in iter` binds, given how many variables the loop
    /// declares. Returns one type per variable, shortest-first; an empty
    /// vec when the iterable's type says nothing useful.
    ///
    /// A single variable takes the element: `for item in items` over
    /// `table<T>` binds `T`. Two variables take key and value, with the
    /// key defaulting to `integer` for an array-style `table<V>` that
    /// declares no key type.
    fn iteration_types(&self, iter: &Expr, vars: usize) -> Vec<Type> {
        let ty = match self.infer_init_type(iter) {
            Some(Type::Nullable(inner)) => *inner,
            Some(t) => t,
            None => return Vec::new(),
        };
        let Type::Table { key, value } = ty else {
            return Vec::new();
        };
        if vars >= 2 {
            let k = key
                .map(|k| *k)
                .unwrap_or_else(|| Type::Named("integer".into()));
            vec![k, *value]
        } else {
            vec![*value]
        }
    }

    /// Record the hover for a loop variable at its binding site, found
    /// by locating the name inside the loop header `search`. Renders
    /// identically to a use of the same variable inside the body, so
    /// `for i` and the `i` two lines down agree.
    ///
    /// Also makes the type ascription's own head (`for x: Color in ...`)
    /// hoverable, matching what parameters and locals already do.
    fn record_loop_var(&mut self, search: &Range<usize>, name: &str, ty: &Type) {
        let Some(span) = locate_word_in(self.source, search, name) else {
            return;
        };
        self.record(
            span,
            format!(
                "```saule\n(loop var) {name}: {ty}\n```",
                ty = render_type(ty)
            ),
        );
        self.record_type_idents_in(ty, search);
    }

    /// The `---` doc comment attached to the declaration starting at
    /// `anchor`, or `None` when it has none (or an empty one).
    ///
    /// Callers reach for this once per declaration and thread the
    /// result through to the parameter loop, so a hover on a parameter
    /// can surface its `@param` line without re-scanning the source.
    fn doc_at(&self, anchor: usize) -> Option<saule_docs::DocBlock> {
        saule_docs::extract(self.source, anchor).filter(|d| !d.is_empty())
    }

    /// Walk into a lambda body, *keeping* the enclosing scope and
    /// stacking the lambda's own parameters on top of it.
    ///
    /// A lambda is a closure: the names around it are exactly the names
    /// its body can use. Starting it with a fresh scope — as this used
    /// to — meant every captured name went unresolved inside the body,
    /// and hover fell through to the next node out. So a cursor on
    /// `rebuild()` or `scratch` inside `onChanged: fn(next: boolean)`
    /// answered with the *lambda's own type*, `(expr): fn(boolean) ->
    /// any`, which describes the callback rather than the token the
    /// cursor is on.
    ///
    /// Function and method declarations still reset — see
    /// [`Cx::enter_function_with_return`]. Their bodies genuinely do not
    /// see a sibling method's locals.
    fn enter_lambda(&mut self, params: &[Param], body: impl FnOnce(&mut Self)) {
        let mark = self.locals.len();
        for p in params {
            self.locals.push(LocalVar {
                name: p.name.clone(),
                ty: p.ty.clone(),
                kind: LocalKind::Param,
            });
        }
        body(self);
        self.locals.truncate(mark);
    }

    /// Like [`Cx::enter_lambda`] but for a `fn` / method declaration:
    /// the outer scope is replaced rather than extended, since a method
    /// body cannot see a sibling's locals. Also tracks the declared
    /// return type so a hover on the `return` keyword inside the body
    /// can surface it.
    fn enter_function_with_return(
        &mut self,
        params: &[Param],
        return_ty: Option<&Type>,
        body: impl FnOnce(&mut Self),
    ) {
        let saved_locals = std::mem::take(&mut self.locals);
        let saved_ret = self.enclosing_return_ty.take();
        self.enclosing_return_ty = return_ty.cloned();
        for p in params {
            self.locals.push(LocalVar {
                name: p.name.clone(),
                ty: p.ty.clone(),
                kind: LocalKind::Param,
            });
        }
        body(self);
        self.locals = saved_locals;
        self.enclosing_return_ty = saved_ret;
    }

    /// Surface hover info for every named-type head reachable from
    /// `ty` (including nullable / table / tuple / function components)
    /// that appears as an identifier inside `search_span` of the
    /// source. Used to make type ascriptions on params, class fields,
    /// return types, etc. resolve the same way bare identifier hovers
    /// do — without baking individual `ty_span` fields into every AST
    /// node that carries a `Type`.
    fn record_type_idents_in(&mut self, ty: &Type, search_span: &Range<usize>) {
        let mut names: Vec<String> = Vec::new();
        collect_named_heads(ty, &mut names);
        for name in names {
            // Skip primitives — they have no useful hover and would
            // mask the parent expression's blurb otherwise.
            if is_primitive(&name) {
                continue;
            }
            if let Some(span) = locate_word_in(self.source, search_span, &name) {
                if !contains(&span, self.offset) {
                    continue;
                }
                if let Some(md) = self.resolve_ident(&name) {
                    self.record(span, md);
                }
            }
        }
    }

    /// Like [`record_type_idents_in`] but takes a list of bare names
    /// (e.g. the parents listed after `extends` / `implements`) and
    /// finds each one inside `search_span`. Names may legitimately
    /// repeat, so we use the first occurrence — that's good enough
    /// for the cases we care about (class headers don't reference the
    /// same parent twice).
    fn record_named_idents_in(&mut self, names: &[String], search_span: &Range<usize>) {
        for name in names {
            if let Some(span) = locate_word_in(self.source, search_span, name) {
                if !contains(&span, self.offset) {
                    continue;
                }
                if let Some(md) = self.resolve_ident(name) {
                    self.record(span, md);
                }
            }
        }
    }

    /// Look up `name` in the current local scope (innermost first).
    /// Returns `None` for free identifiers — the caller falls through
    /// to the registry / native-sig path.
    fn lookup_local(&self, name: &str) -> Option<&LocalVar> {
        self.locals.iter().rev().find(|l| l.name == name)
    }

    /// Infer the types of a call's positional arguments (in order;
    /// `None` where inference can't produce a type). Named arguments are
    /// skipped — mirrors how the typechecker binds generics from
    /// positional args only.
    fn positional_arg_types(&self, args: &[CallArg]) -> Vec<Option<Type>> {
        args.iter()
            .filter_map(|a| match a {
                CallArg::Positional(e) => Some(self.infer_init_type(&e.value)),
                CallArg::Named { .. } => None,
            })
            .collect()
    }

    /// Expand a value-expression list into the flat list of value types it
    /// produces, using Lua-style multi-assign semantics (mirrors the
    /// interpreter's `eval_expr_list`): every expression contributes exactly
    /// one value *except the last*, whose tuple components (a multi-return)
    /// spread into several. Non-final expressions are in single-value context,
    /// so a tuple there collapses to its first component.
    fn spread_value_types(&self, values: &[Spanned<Expr>]) -> Vec<Type> {
        let mut out = Vec::new();
        let n = values.len();
        for (i, v) in values.iter().enumerate() {
            let ty = self.infer_init_type(&v.value);
            if i + 1 == n {
                match ty {
                    Some(Type::Tuple(parts)) => out.extend(parts),
                    other => out.push(first_value_type(other)),
                }
            } else {
                out.push(first_value_type(ty));
            }
        }
        out
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
            // `x as T` is always `T?` — the cast can fail, and the
            // nullable result is what forces the caller to handle it.
            Expr::Cast { ty, .. } => Some(Type::Nullable(Box::new(ty.clone()))),
            Expr::Call { callee, args } => {
                if let Expr::Ident(name) = &callee.value {
                    if with_classes(|r| r.contains_key(name)) {
                        return Some(Type::Named(name.clone()));
                    }
                    // Call to a function declared at top level in this
                    // same file. Its type parameters are bound from the
                    // actual argument types, so `map(table<string>, …)`
                    // infers `table<U>`'s `U` rather than leaving the
                    // local at bare `any`.
                    if let Some(f) = self.module_fns.get(name) {
                        let arg_types = self.positional_arg_types(args);
                        return saule_typeck::sigs::instantiate_returns(&f.sig, &arg_types)
                            .into_iter()
                            .next()
                            // A type parameter the arguments didn't pin
                            // down would come back as its own bare name
                            // (`table<U>`). That's "unknown", not a
                            // type — fall through to `any`.
                            .filter(|t| {
                                !saule_typeck::sigs::mentions_unbound_param(t, &f.sig.type_params)
                            });
                    }
                    // Non-constructor free call: consult native-sig
                    // returns or imported function signatures. We
                    // don't have ASTs for those, so return None and
                    // accept `any`.
                    if let Some(sig) = saule_typeck::sigs::lookup(name) {
                        let arg_types = self.positional_arg_types(args);
                        return saule_typeck::sigs::instantiate_returns(&sig, &arg_types)
                            .into_iter()
                            .next();
                    }
                    // Sibling free function inside a class body —
                    // reach through the enclosing-class registry the
                    // same way `resolve_ident` does for hover.
                    if let Some(class) = &self.enclosing_class
                        && let Some(sig) = lookup_method(class, name)
                    {
                        let arg_types = self.positional_arg_types(args);
                        return saule_typeck::sigs::instantiate_method_return(&sig, &arg_types);
                    }
                }
                // `recv.method(args)` — dot-call on an instance or
                // module. Resolve the receiver's class and chase the
                // method's return type the same way `MethodCall` does.
                if let Expr::Member { obj, name } = &callee.value {
                    let class = self.receiver_class(&obj.value)?;
                    if let Some(sig) = lookup_method(&class, name) {
                        let arg_types = self.positional_arg_types(args);
                        return saule_typeck::sigs::instantiate_method_return(&sig, &arg_types);
                    }
                    let qname = format!("{class}.{name}");
                    if let Some(sig) = saule_typeck::sigs::lookup(&qname) {
                        let arg_types = self.positional_arg_types(args);
                        return saule_typeck::sigs::instantiate_returns(&sig, &arg_types)
                            .into_iter()
                            .next();
                    }
                }
                None
            }
            Expr::MethodCall { obj, method, args } => {
                let class = self.receiver_class(&obj.value)?;
                if let Some(sig) = lookup_method(&class, method) {
                    let arg_types = self.positional_arg_types(args);
                    return saule_typeck::sigs::instantiate_method_return(&sig, &arg_types);
                }
                let qname = format!("{class}.{method}");
                if let Some(sig) = saule_typeck::sigs::lookup(&qname) {
                    let arg_types = self.positional_arg_types(args);
                    return saule_typeck::sigs::instantiate_returns(&sig, &arg_types)
                        .into_iter()
                        .next();
                }
                None
            }
            Expr::Member { obj, name } | Expr::SafeMember { obj, name } => {
                let class = self.receiver_class(&obj.value)?;
                lookup_field_type(&class, name)
            }
            Expr::Index { obj, .. } => {
                // Element type of `table<V>` / `table<K, V>`. Anything
                // else (e.g. indexing a class instance) we don't try
                // to resolve here — typeck handles that statically.
                let obj_ty = self.infer_init_type(&obj.value)?;
                match obj_ty {
                    Type::Table { value, .. } => Some(*value),
                    Type::Nullable(inner) => match *inner {
                        Type::Table { value, .. } => Some(*value),
                        _ => None,
                    },
                    _ => None,
                }
            }
            Expr::ForceUnwrap(inner) => {
                let ty = self.infer_init_type(&inner.value)?;
                Some(strip_nullable_type(ty))
            }
            // Operators are typed by the rules in [`crate::exprty`], shared
            // with the inlay walker and mirrored from the checker.
            Expr::Unary { op, rhs } => crate::exprty::unary_type(self, *op, &rhs.value),
            Expr::Binary { op, lhs, rhs } => {
                crate::exprty::binary_type(self, *op, &lhs.value, &rhs.value)
            }
            Expr::Lambda {
                params,
                return_ty,
                body,
            } => Some(Type::Function {
                params: params.iter().map(|p| p.ty.clone()).collect(),
                ret: Box::new(
                    return_ty
                        .clone()
                        .or_else(|| self.infer_lambda_return(params, body))
                        .unwrap_or_else(|| Type::Named("any".into())),
                ),
            }),
            Expr::Table(entries) => Some(self.infer_table_literal(entries)),
            Expr::Match { arms, .. } => {
                // First arm's body type is a decent approximation;
                // typeck enforces that all arms agree.
                arms.first().and_then(|arm| match &arm.body {
                    saule_ast::MatchBody::Expr(e) => self.infer_init_type(&e.value),
                    saule_ast::MatchBody::Block(b) => match b.last().map(|s| &s.value) {
                        Some(Stmt::Expr(e)) => self.infer_init_type(&e.value),
                        Some(Stmt::Return(rs)) => {
                            rs.first().and_then(|e| self.infer_init_type(&e.value))
                        }
                        _ => None,
                    },
                })
            }
            Expr::Pipe { source, stages } => crate::exprty::pipe_type(self, &source.value, stages),
            Expr::Ident(name) => {
                if let Some(local) = self.lookup_local(name) {
                    return Some(local.ty.clone());
                }
                // Bare class / module / value-type name — its "value"
                // type is the named class itself. Useful for `let s =
                // Storage` (no call) and indirectly for method-chain
                // inference upstream.
                if with_classes(|r| r.contains_key(name))
                    || ((saule_typeck::sigs::is_module(name)
                        || saule_typeck::sigs::is_value_type(name))
                        && saule_semantic::prelude::contains(name))
                {
                    return Some(Type::Named(name.clone()));
                }
                None
            }
            Expr::Self_ => self
                .enclosing_class
                .as_ref()
                .map(|c| Type::Named(c.clone())),
            Expr::Str(_) => Some(Type::Named("string".into())),
            Expr::Int(_) => Some(Type::Named("integer".into())),
            Expr::Float(_) => Some(Type::Named("float".into())),
            Expr::Bool(_) => Some(Type::Named("boolean".into())),
            Expr::Nil => Some(Type::Nullable(Box::new(Type::Named("any".into())))),
        }
    }

    /// Best-effort return type of an unannotated expression-bodied
    /// lambda (`s => #s` is `fn(any) -> integer`). This is what lets a
    /// generic call like `map(items, s => #s)` bind the callback's
    /// result type instead of settling for `table<any>`.
    ///
    /// Two deliberate limits. Block-bodied lambdas are skipped: their
    /// `return` statements would need the full statement walk, and a
    /// wrong answer here is worse than `any`. And a lambda whose
    /// parameters shadow an in-scope binding is skipped too — the body
    /// is inferred against the *enclosing* scope (the parameters aren't
    /// pushed, since inference runs behind `&self`), so a shadowed name
    /// would silently resolve to the outer variable's type.
    fn infer_lambda_return(&self, params: &[Param], body: &saule_ast::LambdaBody) -> Option<Type> {
        let saule_ast::LambdaBody::Expr(e) = body else {
            return None;
        };
        if params.iter().any(|p| self.lookup_local(&p.name).is_some()) {
            return None;
        }
        Some(first_value_type(self.infer_init_type(&e.value)))
    }

    /// Infer a `table<V>` (array literal) or `table<K, V>` (map literal)
    /// from a table-constructor's entries, so generic natives like
    /// `Util.map(table<T>, …)` can bind their element type. Falls back to a
    /// bare `table` when the entries are empty or their types disagree.
    fn infer_table_literal(&self, entries: &[saule_ast::TableEntry]) -> Type {
        use saule_ast::TableEntry;
        let mut value_ty: Option<Type> = None;
        let mut key_ty: Option<Type> = None;
        let mut has_field = false;
        let mut consistent = true;
        for entry in entries {
            let (k, v) = match entry {
                TableEntry::Positional(v) => (None, self.infer_init_type(&v.value)),
                TableEntry::Field { key, value } => {
                    has_field = true;
                    (
                        self.infer_init_type(&key.value),
                        self.infer_init_type(&value.value),
                    )
                }
            };
            if let Some(vt) = v {
                match &value_ty {
                    Some(existing) if existing != &vt => consistent = false,
                    _ => value_ty = Some(vt),
                }
            }
            if let Some(kt) = k {
                match &key_ty {
                    Some(existing) if existing != &kt => consistent = false,
                    _ => key_ty = Some(kt),
                }
            }
        }
        match value_ty {
            Some(v) if consistent => Type::Table {
                key: if has_field {
                    key_ty.map(Box::new)
                } else {
                    None
                },
                value: Box::new(v),
            },
            _ => Type::Named("table".into()),
        }
    }

    /// Refine a bare structural annotation (`table` / `function`) against the
    /// initializer's inferred shape: `local nums: table = {1, 2}` becomes
    /// `table<integer>`. A `nil`-side or mismatched-kind value leaves the
    /// declared type untouched. Mirrors typeck's `refine_bare_binding`.
    fn refine_bare_annotation(&self, decl: Type, value: Option<Type>) -> Type {
        let Type::Named(name) = &decl else {
            return decl;
        };
        let Some(value_ty) = value else {
            return decl;
        };
        let inner = match &value_ty {
            Type::Nullable(i) => (**i).clone(),
            other => other.clone(),
        };
        let matches_kind = matches!(
            (name.as_str(), &inner),
            ("table", Type::Table { .. }) | ("function", Type::Function { .. })
        );
        if matches_kind { inner } else { decl }
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
            Stmt::Local {
                name,
                name_span,
                ty,
                ty_span,
                value,
            } => {
                if let Some(v) = value {
                    self.visit_expr(v);
                }
                let resolved = match ty.clone() {
                    // A bare structural annotation (`table` / `function`)
                    // is refined against the initializer's inferred shape
                    // so generic natives can bind the element type
                    // (`local nums: table = {1, 2}` -> `table<integer>`).
                    Some(t) => self.refine_bare_annotation(
                        t,
                        value.as_ref().and_then(|v| self.infer_init_type(&v.value)),
                    ),
                    // A single binding takes one value; a multi-return
                    // (tuple) collapses to its first component here.
                    None => first_value_type(
                        value.as_ref().and_then(|v| self.infer_init_type(&v.value)),
                    ),
                };
                // Surface a hover blurb when the cursor is on the
                // declaring identifier itself (`local due: int? = …`
                // -> hover on `due`). Without this the walker would
                // only fire on later *uses* of the name.
                self.record(
                    name_span.clone(),
                    format!(
                        "```saule\n(local) {name}: {ty}\n```",
                        ty = render_type(&resolved)
                    ),
                );
                // Cursor on the type ascription itself
                // (`local s: Storage = …` -> hover on `Storage`):
                // resolve the head named type through the same
                // identifier path that handles bare class / interface
                // / enum references in expressions.
                if let (Some(span), Some(t)) = (ty_span, ty.as_ref())
                    && let Some(head) = named_type(t)
                    && let Some(md) = self.resolve_ident(&head)
                {
                    self.record(span.clone(), md);
                }
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
                let spread = self.spread_value_types(values);
                for (i, (name, name_span, ty)) in names.iter().enumerate() {
                    let resolved = ty
                        .clone()
                        .or_else(|| spread.get(i).cloned())
                        .unwrap_or_else(|| Type::Named("any".into()));
                    // Surface a hover blurb when the cursor is on the
                    // declaring identifier itself (`local q, r = …`
                    // -> hover on `q`), mirroring `Stmt::Local`.
                    self.record(
                        name_span.clone(),
                        format!(
                            "```saule\n(local) {name}: {ty}\n```",
                            ty = render_type(&resolved)
                        ),
                    );
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
                let ty = var_ty
                    .clone()
                    .unwrap_or_else(|| Type::Named("integer".into()));
                // The binding site itself. Uses within the body resolve
                // through `lookup_local`, but `for i: integer = 1, 20`
                // has no span-tracked node of its own — without this the
                // declaration is the one place in the loop where hovering
                // `i` falls through to the enclosing function.
                self.record_loop_var(&(s.span.start..from.span.start), var, &ty);
                let mark = self.locals.len();
                self.locals.push(LocalVar {
                    name: var.clone(),
                    ty,
                    kind: LocalKind::LoopVar,
                });
                self.visit_block(body);
                self.locals.truncate(mark);
            }
            Stmt::ForIn { vars, iter, body } => {
                self.visit_expr(iter);
                let mark = self.locals.len();
                let header = s.span.start..iter.span.start;
                // What the iterable yields, for the vars that were left
                // unannotated. `for item in items` over a `table<T>` is
                // the normal way to write a loop, and defaulting those
                // to `any` reported the one thing hover already knew to
                // be wrong.
                let yielded = self.iteration_types(&iter.value, vars.len());
                for (i, (name, ty)) in vars.iter().enumerate() {
                    let ty = ty
                        .clone()
                        .or_else(|| yielded.get(i).cloned())
                        .unwrap_or_else(|| Type::Named("any".into()));
                    self.record_loop_var(&header, name, &ty);
                    self.locals.push(LocalVar {
                        name: name.clone(),
                        ty,
                        kind: LocalKind::LoopVar,
                    });
                }
                self.visit_block(body);
                self.locals.truncate(mark);
            }
            Stmt::Return(es) => {
                // Hover on the `return` keyword surfaces the enclosing
                // function's declared return type. The keyword sits at
                // the start of the statement span; we use the first
                // 6 bytes (`return`) as the hit region, falling back
                // to the whole stmt span when the source is empty
                // (unit tests that don't pass a source string).
                if let Some(rt) = self.enclosing_return_ty.clone() {
                    let kw_end = (s.span.start + "return".len()).min(s.span.end);
                    let kw_span = s.span.start..kw_end;
                    self.record(
                        kw_span,
                        format!("```saule\n(return) -> {ty}\n```", ty = render_type(&rt)),
                    );
                }
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
                let doc = self.doc_at(d.span.start);
                let head_end = params
                    .first()
                    .map(|p| p.span.start)
                    .or_else(|| body.first().map(|s| s.span.start))
                    .unwrap_or(d.span.end);
                let name_span = self.decl_name_span(&d.span, name, head_end);
                self.record(
                    name_span,
                    with_doc(
                        render_function_sig(name, type_params, params, return_ty.as_ref()),
                        doc.as_ref(),
                    ),
                );
                for p in params {
                    self.record(
                        p.span.clone(),
                        with_param_doc(render_param(p), doc.as_ref(), &p.name),
                    );
                    // Type ascription on the param: `x: Storage` —
                    // make `Storage` itself hoverable.
                    self.record_type_idents_in(&p.ty, &p.span);
                    if let Some(def) = &p.default {
                        self.visit_expr(def);
                    }
                }
                if let Some(rt) = return_ty {
                    // Locate the return type slice between the closing
                    // `)` of the param list and the start of the body.
                    let after = params.last().map(|p| p.span.end).unwrap_or(d.span.start);
                    let before = body.first().map(|s| s.span.start).unwrap_or(d.span.end);
                    self.record_type_idents_in(rt, &(after..before));
                }
                let params = params.clone();
                let return_ty = return_ty.clone();
                self.enter_function_with_return(&params, return_ty.as_ref(), |this| {
                    this.visit_block(body)
                });
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
                let doc = self.doc_at(d.span.start);
                let header_end = members.first().map(|m| m.span.start).unwrap_or(d.span.end);
                let name_span = self.decl_name_span(&d.span, name, header_end);
                self.record(name_span, with_doc(md, doc.as_ref()));
                // Resolve `extends X` / `implements Y, Z` parent and
                // interface names within the class header (between
                // the class name and the first member). We use the
                // first occurrence of each name within the decl span
                // as the hover target.
                let header_span = d.span.start..header_end;
                if let Some(parent) = extends {
                    self.record_named_idents_in(std::slice::from_ref(parent), &header_span);
                }
                self.record_named_idents_in(implements, &header_span);
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
                let doc = self.doc_at(d.span.start);
                let head_end = methods.first().map(|m| m.span.start).unwrap_or(d.span.end);
                let name_span = self.decl_name_span(&d.span, name, head_end);
                self.record(
                    name_span,
                    with_doc(render_interface_head(name, extends, methods), doc.as_ref()),
                );
                // Same idea as class `extends` — locate parent
                // interface names within the interface decl span.
                self.record_named_idents_in(extends, &d.span);
                // Each bodiless method is a hover target of its own so
                // a `---` block written above it has somewhere to go.
                for m in methods {
                    if !contains(&m.span, self.offset) {
                        continue;
                    }
                    let mdoc = self.doc_at(m.span.start);
                    self.record(
                        m.span.clone(),
                        with_doc(render_interface_method(name, m), mdoc.as_ref()),
                    );
                    for p in &m.params {
                        self.record(
                            p.span.clone(),
                            with_param_doc(render_param(p), mdoc.as_ref(), &p.name),
                        );
                        self.record_type_idents_in(&p.ty, &p.span);
                    }
                }
            }
            Decl::Enum {
                name,
                variants,
                methods,
                ..
            } => {
                let doc = self.doc_at(d.span.start);
                let head_end = variants.first().map(|v| v.span.start).unwrap_or(d.span.end);
                let name_span = self.decl_name_span(&d.span, name, head_end);
                self.record(
                    name_span,
                    with_doc(render_enum_head(name, variants), doc.as_ref()),
                );
                for v in variants {
                    if !contains(&v.span, self.offset) {
                        continue;
                    }
                    let vdoc = self.doc_at(v.span.start);
                    self.record(
                        v.span.clone(),
                        with_doc(render_enum_variant_decl(name, &v.value), vdoc.as_ref()),
                    );
                    if let saule_ast::EnumVariant::Tuple { fields, .. } = &v.value {
                        for p in fields {
                            self.record(
                                p.span.clone(),
                                with_param_doc(render_param(p), vdoc.as_ref(), &p.name),
                            );
                        }
                    }
                }
                let prev = self.enclosing_class.replace(name.clone());
                for m in methods {
                    self.visit_method(m, name);
                }
                self.enclosing_class = prev;
            }
            Decl::Import { names, .. } => {
                // Best match wins: walk the precomputed blurbs and
                // record any whose span contains the cursor. Spans
                // come from the `Spanned<Decl>` itself, so they cover
                // the full statement.
                for (span, md) in &self.imports.import_blurbs {
                    if contains(span, self.offset) {
                        self.record(span.clone(), md.clone());
                    }
                }
                // Per-name resolution: if the cursor is on one of the
                // listed import names, prefer its specific class /
                // interface / enum / function blurb over the generic
                // statement-wide one. Aliased imports (`X as Y`)
                // resolve through the alias name the user typed.
                if let saule_ast::ImportNames::List(items) = names {
                    for (orig, alias) in items {
                        let local = alias.as_deref().unwrap_or(orig);
                        // Local alias span — the name the importer
                        // sees and would hover.
                        if let Some(span) = locate_word_in(self.source, &d.span, local)
                            && contains(&span, self.offset)
                            && let Some(md) = self.resolve_ident(local)
                        {
                            self.record(span, md);
                        }
                        // Original (upstream) name when an alias was
                        // used: `import X as Y` — both should hover.
                        if alias.is_some()
                            && let Some(span) = locate_word_in(self.source, &d.span, orig)
                            && contains(&span, self.offset)
                            && let Some(md) = self.resolve_ident(orig)
                        {
                            self.record(span, md);
                        }
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
                let doc = self.doc_at(m.span.start);
                self.record(
                    m.span.clone(),
                    with_doc(
                        render_field(owner, *is_static, *is_private, name, ty),
                        doc.as_ref(),
                    ),
                );
                // Make `due: integer?` -> hover on `integer` resolve
                // through the same path bare ident hovers use.
                self.record_type_idents_in(ty, &m.span);
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
        let doc = self.doc_at(m.span.start);
        let head_end = m
            .params
            .first()
            .map(|p| p.span.start)
            .or_else(|| m.body.first().map(|s| s.span.start))
            .unwrap_or(m.span.end);
        let name_span = self.decl_name_span(&m.span, &m.name, head_end);
        self.record(
            name_span,
            with_doc(render_method_head(owner, m), doc.as_ref()),
        );
        for p in &m.params {
            self.record(
                p.span.clone(),
                with_param_doc(render_param(p), doc.as_ref(), &p.name),
            );
            self.record_type_idents_in(&p.ty, &p.span);
            if let Some(def) = &p.default {
                self.visit_expr(def);
            }
        }
        if let Some(rt) = &m.return_ty {
            let after = m.params.last().map(|p| p.span.end).unwrap_or(m.span.start);
            let before = m.body.first().map(|s| s.span.start).unwrap_or(m.span.end);
            self.record_type_idents_in(rt, &(after..before));
        }
        let params = m.params.clone();
        let return_ty = m.return_ty.clone();
        self.enter_function_with_return(&params, return_ty.as_ref(), |this| {
            this.visit_block(&m.body)
        });
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
            // Recurse into the operand so the cast is transparent for
            // hover; the target type's own idents are handled below.
            Expr::Cast { value, ty } => {
                self.visit_expr(value);
                self.record_type_idents_in(ty, &e.span);
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
            Expr::Call { callee, args } => {
                self.visit_expr(callee);
                let sig = self.callee_sig(&callee.value);
                for a in args {
                    self.visit_call_arg_with_params(a, sig.as_ref());
                }
            }
            Expr::MethodCall { obj, method, args } => {
                self.visit_expr(obj);
                let sig = self.receiver_class(&obj.value).and_then(|class| {
                    let m = lookup_method(&class, method)?;
                    let key = format!("{class}.{method}");
                    Some(CalleeSig {
                        params: m.params,
                        doc: self.imports.docs.get(&key).cloned(),
                        display: key,
                    })
                });
                for a in args {
                    self.visit_call_arg_with_params(a, sig.as_ref());
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
                self.enter_lambda(&params, |this| match body {
                    saule_ast::LambdaBody::Expr(b) => this.visit_expr(b),
                    saule_ast::LambdaBody::Block(b) => this.visit_block(b),
                });
            }
            Expr::Match { scrutinee, arms } => {
                self.visit_expr(scrutinee);
                let scrut_ty = self.infer_init_type(&scrutinee.value);
                for arm in arms {
                    let mark = self.locals.len();
                    // Bind first so the recursive `visit_pattern`
                    // walk can render `Bind` names through the
                    // local-scope path with their inferred type.
                    self.bind_pattern(&arm.pattern.value, scrut_ty.as_ref());
                    self.visit_pattern(&arm.pattern);
                    if let Some(g) = &arm.guard {
                        self.visit_expr(g);
                    }
                    match &arm.body {
                        saule_ast::MatchBody::Expr(e) => self.visit_expr(e),
                        saule_ast::MatchBody::Block(b) => self.visit_block(b),
                    }
                    self.locals.truncate(mark);
                }
            }
            Expr::Pipe { source, stages } => {
                self.visit_expr(source);
                for st in stages {
                    // Stdlib pipe stages (`|> Math.sqrt`) carry only
                    // positional types — no parameter names — so we
                    // can't resolve named-arg keys against them.
                    for a in &st.args {
                        self.visit_call_arg_with_params(a, None);
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

    #[allow(dead_code)]
    fn visit_call_arg(&mut self, a: &saule_ast::CallArg) {
        match a {
            saule_ast::CallArg::Positional(e) => self.visit_expr(e),
            saule_ast::CallArg::Named { value, .. } => self.visit_expr(value),
        }
    }

    /// Like [`visit_call_arg`] but also surfaces hover info on a named
    /// argument's *key* (`storage.add(item, dueDate: due)` -> hovering
    /// on `dueDate` shows the parameter declaration). `callee` carries
    /// the resolved callee when we found one, so the key can be rendered
    /// as the parameter it actually is.
    fn visit_call_arg_with_params(&mut self, a: &saule_ast::CallArg, callee: Option<&CalleeSig>) {
        match a {
            saule_ast::CallArg::Positional(e) => self.visit_expr(e),
            saule_ast::CallArg::Named { name, value } => {
                let key_search = value.span.start.saturating_sub(name.len() + 4)..value.span.start;
                if let Some(span) = locate_word_in(self.source, &key_search, name)
                    && contains(&span, self.offset)
                {
                    self.record(span, self.render_named_arg(name, callee));
                }
                self.visit_expr(value);
            }
        }
    }

    /// The hover for a named argument's key.
    ///
    /// A named-argument key *is* the callee's parameter, so it renders
    /// as one — same `(parameter)` label, same `= …` default marker as
    /// the declaration site, and qualified by the callee's name.
    ///
    /// The qualifier is what makes this readable in Flutter-shaped code.
    /// A single build method here holds eight `child:` keys belonging to
    /// six different widgets; unqualified they all rendered the identical
    /// `child: Widget?` blurb, so the popup confirmed nothing about
    /// which one the cursor was on. `Container.child` does.
    fn render_named_arg(&self, name: &str, callee: Option<&CalleeSig>) -> String {
        let Some(c) = callee else {
            // Callee unresolved — say only what we actually know rather
            // than inventing a type.
            return format!("```saule\n(parameter) {name}\n```");
        };
        let Some(p) = c.params.iter().find(|p| p.name == name) else {
            // Callee resolved but has no such parameter. Naming the miss
            // is the same service `render_unknown_member` performs, and
            // it stops a wider node from answering in its place.
            return format!(
                "```saule\n(unknown) {owner}.{name}\n```\nNo parameter `{name}` on `{owner}`.",
                owner = c.display
            );
        };
        let mut s = String::from("```saule\n(parameter) ");
        if !c.display.is_empty() {
            s.push_str(&c.display);
            s.push('.');
        }
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
        with_param_doc(s, c.doc.as_ref(), name)
    }

    /// Best-effort: resolve `callee` to the name and parameter list
    /// named-argument hover needs. Handles the common shapes —
    /// constructor calls, sibling free / static / method calls, and
    /// functions imported from another file — without trying to be
    /// exhaustive.
    fn callee_sig(&self, callee: &Expr) -> Option<CalleeSig> {
        let sig = |display: String, params: Vec<Param>, doc_key: &str| CalleeSig {
            params,
            doc: self.imports.docs.get(doc_key).cloned(),
            display,
        };
        match callee {
            Expr::Ident(name) => {
                // Constructor: `init` method, falling through to a
                // bare `Class()` call (which uses no init params).
                if with_classes(|r| r.contains_key(name)) {
                    let params = lookup_method(name, "init").map(|s| s.params)?;
                    // Constructor prose is conventionally written on the
                    // class, so try that before `Class.init`.
                    let key = if self.imports.docs.get(name).is_some() {
                        name.clone()
                    } else {
                        format!("{name}.init")
                    };
                    return Some(sig(name.clone(), params, &key));
                }
                if let Some(class) = &self.enclosing_class
                    && let Some(m) = lookup_method(class, name)
                {
                    return Some(sig(
                        format!("{class}.{name}"),
                        m.params,
                        &format!("{class}.{name}"),
                    ));
                }
                // Sibling top-level `fn` — the one free-call shape where
                // we do have the declared parameter *names*.
                if let Some(f) = self.module_fns.get(name) {
                    return Some(sig(name.clone(), f.params.clone(), name));
                }
                // A free function imported from another `.sau` file,
                // including through a re-export barrel — the
                // `showDialog(builder: …)` helpers a UI file is full of.
                // The seed registers these by local alias, so no import
                // walk is needed here.
                if let Some(f) = saule_semantic::lookup_function(name) {
                    return Some(sig(name.clone(), f.params, name));
                }
                // Native sigs (`Math.sqrt`, `print`) know positional
                // types but no parameter names, so they can't drive
                // named-arg hover. Treat them as unresolved.
                None
            }
            Expr::Member { obj, name } => {
                if let Some((owner, m)) = self.super_target(name, &obj.value) {
                    let key = format!("{owner}.init");
                    return Some(sig(format!("{owner}.init"), m.params, &key));
                }
                let class = self.receiver_class(&obj.value)?;
                let m = lookup_method(&class, name)?;
                let key = format!("{class}.{name}");
                Some(sig(key.clone(), m.params, &key))
            }
            _ => None,
        }
    }

    /// Walk a `match` pattern, recording hover info for the parts that
    /// have something useful to say:
    ///
    /// * `Variant { enum_name, variant, fields }` — render the variant
    ///   shape (`(variant) Enum.Variant(field: T, ...)`) and recurse
    ///   into the sub-patterns.
    /// * `Tuple(parts)` — recurse only.
    /// * `Bind(name)` — no hover here; the binding is rendered through
    ///   the local-scope path once it's been pushed by `bind_pattern`.
    /// * Literal patterns — no hover (matches today's behaviour for
    ///   literal expressions).
    fn visit_pattern(&mut self, p: &Spanned<Pattern>) {
        if !contains(&p.span, self.offset) {
            return;
        }
        match &p.value {
            Pattern::Variant {
                enum_name,
                variant,
                fields,
            } => {
                self.record(
                    p.span.clone(),
                    render_variant_pattern(enum_name, variant, fields, &self.enum_variant_fields),
                );
                for f in fields {
                    self.visit_pattern(f);
                }
            }
            Pattern::Tuple(parts) => {
                for f in parts {
                    self.visit_pattern(f);
                }
            }
            Pattern::Bind(name) => {
                // Look up the just-pushed binding so the hover shows
                // its inferred type (`(binding) task: Task?`, etc.).
                if let Some(local) = self.lookup_local(name) {
                    self.record(
                        p.span.clone(),
                        format!(
                            "```saule\n(binding) {name}: {ty}\n```",
                            ty = render_type(&local.ty)
                        ),
                    );
                } else {
                    self.record(p.span.clone(), format!("```saule\n(binding) {name}\n```"));
                }
            }
            Pattern::Wildcard => {
                self.record(p.span.clone(), "```saule\n(wildcard) _\n```".to_string());
            }
            Pattern::Nil => {
                self.record(p.span.clone(), "```saule\n(pattern) nil\n```".to_string());
            }
            Pattern::Int(_) | Pattern::Float(_) | Pattern::Bool(_) | Pattern::Str(_) => {}
        }
    }

    /// Push every name introduced by `pat` onto the local scope, using
    /// `scrut_ty` to type top-level `Bind` and tuple bindings. Variant
    /// payload bindings are typed from the enum's recorded field
    /// types. Anything we can't type defaults to `any`.
    fn bind_pattern(&mut self, pat: &Pattern, scrut_ty: Option<&Type>) {
        match pat {
            Pattern::Bind(name) => {
                // Strip the nullable wrapper: `case nil` is the only
                // arm that handles nil, so any other arm — including
                // a bare `case binding` — implies the value is
                // non-nil. Mirrors `saule-typeck`'s arm-binding rule
                // so hover types match diagnostics.
                let ty = scrut_ty
                    .map(|t| strip_nullable_type(t.clone()))
                    .unwrap_or_else(|| Type::Named("any".into()));
                self.locals.push(LocalVar {
                    name: name.clone(),
                    ty,
                    kind: LocalKind::Binding,
                });
            }
            Pattern::Variant {
                enum_name,
                variant,
                fields,
            } => {
                let field_tys: Vec<Type> = self
                    .enum_variant_fields
                    .get(&(enum_name.clone(), variant.clone()))
                    .map(|ps| ps.iter().map(|p| p.ty.clone()).collect())
                    .unwrap_or_default();
                for (i, sub) in fields.iter().enumerate() {
                    let sub_ty = field_tys.get(i);
                    self.bind_pattern(&sub.value, sub_ty);
                }
            }
            Pattern::Tuple(parts) => {
                let elems: Option<&[Type]> = match scrut_ty {
                    Some(Type::Tuple(parts)) => Some(parts.as_slice()),
                    _ => None,
                };
                for (i, sub) in parts.iter().enumerate() {
                    let sub_ty = elems.and_then(|e| e.get(i));
                    self.bind_pattern(&sub.value, sub_ty);
                }
            }
            Pattern::Wildcard
            | Pattern::Nil
            | Pattern::Int(_)
            | Pattern::Float(_)
            | Pattern::Bool(_)
            | Pattern::Str(_) => {}
        }
    }

    /// Render a Markdown blurb for `expr` if we can resolve it from the
    /// registries / surrounding context. Returns `None` for literals
    /// and unknown names.
    ///
    /// `None` does **not** mean "no hover here". [`Cx::record`] keeps the
    /// narrowest span containing the cursor, so declining to answer for a
    /// node just lets a wider one — in practice the enclosing `fn` — win
    /// instead. For a token the user pointed at directly that reads as a
    /// confident, wrong answer about an unrelated symbol.
    ///
    /// So: return `None` only where the node itself is genuinely nothing
    /// to talk about (a literal, a `nil`). Where we know enough to say
    /// something is *wrong* — a member that isn't on its receiver — say
    /// that, via [`render_unknown_member`]. It is both the more useful
    /// answer and the one that stops the fallback.
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
                        LocalKind::Binding => "(binding)",
                    };
                    return Some(format!(
                        "```saule\n{label} {name}: {ty}\n```",
                        ty = render_type(&local.ty)
                    ));
                }
                self.resolve_ident(name)
            }
            Expr::Member { obj, name } | Expr::SafeMember { obj, name } => {
                // `self.super(...)` isn't a member access — surface the
                // parent constructor it delegates to instead.
                if let Some((owner, sig)) = self.super_target(name, &obj.value) {
                    let enclosing = self.enclosing_class.clone().unwrap_or_default();
                    return Some(format!(
                        "{sig}\nParent constructor, delegated to by `self.super(...)` in `{enclosing}`.",
                        sig = render_method_sig(&owner, "init", &sig)
                    ));
                }
                let class = self.receiver_class(&obj.value)?;
                Some(
                    resolve_member(&class, name, false, &self.imports.docs)
                        .unwrap_or_else(|| render_unknown_member(&class, name)),
                )
            }
            Expr::MethodCall { obj, method, .. } => {
                let class = self.receiver_class(&obj.value)?;
                Some(
                    resolve_member(&class, method, true, &self.imports.docs)
                        .unwrap_or_else(|| render_unknown_member(&class, method)),
                )
            }
            // A literal is its own documentation. Answering `(expr):
            // string` for a cursor inside a sentence of prose is the
            // kind of hover that fires where the reader asked nothing,
            // and the type it reports was never in doubt.
            Expr::Int(_) | Expr::Float(_) | Expr::Bool(_) | Expr::Str(_) | Expr::Nil => None,
            // Generic fallback: surface the inferred type for any
            // expression we can reason about (`#args` -> integer,
            // `args[1]` -> string, `not foo` -> boolean, lambdas,
            // call results, etc.). Without this, hovering on the
            // gaps between named children returned `None`.
            other => self
                .infer_init_type(other)
                .map(|ty| format!("```saule\n(expr): {ty}\n```", ty = render_type(&ty))),
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
        // Type declarations resolve through the registries, which hold
        // no source text — pull any `---` block from the doc index so a
        // hover on a *usage* reads the same as one on the declaration.
        let doc = self.imports.docs.get(name);
        if let Some(info) = with_classes(|r| r.get(name).cloned()) {
            return Some(with_doc(render_class_full(name, &info), doc));
        }
        if with_interfaces(|r| r.contains_key(name)) {
            let extends = with_interfaces(|r| r.get(name).cloned()).unwrap_or_default();
            return Some(with_doc(
                render_interface_from_registry(name, &extends),
                doc,
            ));
        }
        if with_enums(|r| r.contains_key(name)) {
            let info = with_enums(|r| r.get(name).cloned())?;
            let variants: Vec<(String, usize)> = info
                .variants
                .iter()
                .map(|(n, v)| (n.clone(), v.arity()))
                .collect();
            return Some(with_doc(render_enum_from_registry(name, &variants), doc));
        }
        // Free function declared at top level in *this* module. Checked
        // ahead of the native signatures because a local declaration
        // shadows a stdlib name of the same spelling for the rest of
        // the file, and the hover should follow the same rule.
        if let Some(f) = self.module_fns.get(name) {
            return Some(with_doc(f.md.clone(), doc));
        }
        if let Some(sig) = saule_typeck::sigs::lookup(name) {
            return Some(format!(
                "```saule\nfn {name}{}\n```",
                render_native_sig_full(&sig)
            ));
        }
        if saule_typeck::sigs::is_value_type(name) && saule_semantic::prelude::contains(name) {
            return Some(render_stdlib_module(name, "type"));
        }
        if saule_typeck::sigs::is_module(name) && saule_semantic::prelude::contains(name) {
            return Some(render_stdlib_module(name, "module"));
        }
        // Bare identifier inside a class body — try resolving as a
        // method or field of the enclosing class. Covers calling a
        // sibling static (`help()` from inside `Main.main()`) and
        // referencing a sibling instance member without `self.`.
        if let Some(class) = &self.enclosing_class
            && let Some(md) = resolve_member(class, name, false, &self.imports.docs)
        {
            return Some(md);
        }
        // A top-level function imported from another file — final
        // fallback. `analyze_with_seed` registers these under the local
        // alias, following re-export barrels, so a name reached through
        // `import * from UIKit` resolves the same as a direct one.
        if let Some(sig) = saule_semantic::lookup_function(name) {
            return Some(with_doc(
                render_function_sig(name, &sig.type_params, &sig.params, sig.return_ty.as_ref()),
                doc,
            ));
        }
        None
    }

    /// `(owner, sig)` of the constructor a `self.super(...)` written in
    /// the current class delegates to, when `name` / `obj` are exactly
    /// that form. `None` for every other member access.
    fn super_target(&self, name: &str, obj: &Expr) -> Option<(String, saule_semantic::MethodSig)> {
        if name != "super" || !matches!(obj, Expr::Self_) {
            return None;
        }
        super_init_target(self.enclosing_class.as_deref()?)
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
                    || ((saule_typeck::sigs::is_module(name)
                        || saule_typeck::sigs::is_value_type(name))
                        && saule_semantic::prelude::contains(name))
                {
                    Some(name.clone())
                } else {
                    None
                }
            }
            Expr::Member { obj: inner, name } | Expr::SafeMember { obj: inner, name } => {
                let inner_class = self.receiver_class(&inner.value)?;
                let ty = lookup_field_type(&inner_class, name)?;
                named_type(&ty)
            }
            // `x!.foo` — the unwrap doesn't change which class the
            // receiver names, it only drops the nullability, and
            // `named_type` already looks through `Nullable`.
            //
            // This arm matters more than it looks: a nullable has to be
            // unwrapped before anything can be read off it, so every
            // hover through one — which is most tree-walking code —
            // used to fall through to `None` here and get answered with
            // the *enclosing function* instead. That isn't a missing
            // hover, it's a wrong one: it names a symbol that has
            // nothing to do with the token under the cursor.
            Expr::ForceUnwrap(inner) => self.receiver_class(&inner.value),
            // `(x as T).foo` — the cast names the class outright, which
            // is the whole reason to have written it.
            Expr::Cast { ty, .. } => named_type(ty),
            Expr::Call { callee, .. } => {
                // `Class(args).foo` — constructor call returns the class.
                if let Expr::Ident(name) = &callee.value
                    && with_classes(|r| r.contains_key(name))
                {
                    return Some(name.clone());
                }
                None
            }
            Expr::MethodCall {
                obj: inner, method, ..
            } => {
                // `obj:method(args).foo` — chase the method's
                // registered return type.
                let inner_class = self.receiver_class(&inner.value)?;
                let sig = lookup_method(&inner_class, method)?;
                named_type(sig.return_ty.as_ref()?)
            }
            Expr::Index { obj: inner, .. } => {
                // `tbl[i].foo` — the receiver's class is the element
                // type of the indexed `table<…>` (or the `value` half
                // of a `table<K, V>`). Walk the inner expression to
                // get its declared type, then peel one level.
                let ty = self.infer_init_type(&inner.value)?;
                match ty {
                    Type::Table { value, .. } => named_type(&value),
                    other => named_type(&other),
                }
            }
            _ => None,
        }
    }
}
