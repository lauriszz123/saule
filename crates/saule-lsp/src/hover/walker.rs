//! AST walker that picks the smallest enclosing node at a byte offset
//! and produces a Markdown blurb for it. The big stateful machinery
//! (`Cx`, scope tracking, every `visit_*` arm) lives here; rendering
//! helpers live in [`super::render`] and shared utilities in
//! [`super::util`].

mod decls;
mod exprs;
mod infer;
mod scope;

use std::collections::HashMap;
use std::ops::Range;

use saule_ast::{CallArg, Decl, Expr, Module, Param, Spanned, Stmt, Type};

use super::ImportContext;
use super::render::{collect_enum_variant_fields, render_function_sig, render_type};
use super::util::locate_word_in;

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
    /// The callee's own generic parameters, so an argument's expected
    /// type can be instantiated from the other arguments rather than
    /// reported as the bare `T` the signature spells.
    type_params: Vec<String>,
    /// The callee's `---` block, so an `@param child` line reaches the
    /// hover on the `child:` key.
    doc: Option<saule_docs::DocBlock>,
}

impl CalleeSig {
    /// The type each argument slot expects, with this callee's generics
    /// bound from the argument types.
    fn expected_arg_types(&self, arg_types: &[Option<Type>]) -> Vec<Option<Type>> {
        saule_typeck::sigs::instantiate_params(
            &saule_typeck::sigs::NativeSig {
                type_params: self.type_params.clone(),
                bounds: Vec::new(),
                params: self.params.iter().map(|p| p.ty.clone()).collect(),
                variadic: None,
                returns: Vec::new(),
            },
            arg_types,
        )
    }
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
            // The written return type, or the one the semantic pass
            // inferred from the body. This map is consulted ahead of the
            // registry for a call to a sibling function, so reading only
            // the AST here would hide an inferred type behind the very
            // shortcut that exists to make sibling calls resolve.
            let resolved = return_ty
                .clone()
                .or_else(|| saule_semantic::lookup_function(name)?.return_ty);
            out.entry(name.clone()).or_insert_with(|| ModuleFn {
                md: render_function_sig(name, type_params, params, resolved.as_ref()),
                params: params.clone(),
                sig: saule_typeck::sigs::NativeSig {
                    type_params: type_params.clone(),
                    bounds: Vec::new(),
                    params: params.iter().map(|p| p.ty.clone()).collect(),
                    variadic: None,
                    returns: resolved.clone().into_iter().collect(),
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
            // A recovery hole has no children to walk.
            Stmt::Error => {}
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
                    // The bare structural annotation `table` is
                    // refined against the initializer's inferred shape
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
                // resolve through the same identifier path that handles
                // bare class / interface / enum references in
                // expressions. Every named head reachable from the
                // annotation counts, not just its outermost one —
                // `local blocks: table<Block>` spells `Block` inside a
                // `table`, and matching only the head meant that name
                // (the interesting half) had no hover at all.
                if let (Some(span), Some(t)) = (ty_span, ty.as_ref()) {
                    self.record_type_idents_in(t, span);
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
                    // `local a: Point, b: Point = …` — each ascription
                    // lives between its own name and the next one.
                    // `LocalMulti` tracks no `ty_span`, so it is found
                    // by scanning that slice.
                    if let Some(t) = ty {
                        let end = names
                            .get(i + 1)
                            .map(|(_, next, _)| next.start)
                            .or_else(|| values.first().map(|v| v.span.start))
                            .unwrap_or(s.span.end);
                        self.record_type_idents_in(t, &(name_span.end..end.max(name_span.end)));
                    }
                    self.locals.push(LocalVar {
                        name: name.clone(),
                        ty: resolved,
                        kind: LocalKind::Local,
                    });
                }
            }
            Stmt::Assign { target, value } | Stmt::CompoundAssign { target, value, .. } => {
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
                // The `catch e: T` clause is a binding site with no
                // span-tracked node of its own — the same hole loop
                // variables had. Scan the clause itself (after the last
                // body statement, before the catch block) so a local in
                // the `try` body sharing the name can't be mistaken for
                // it.
                let clause = body.last().map(|b| b.span.end).unwrap_or(s.span.start)
                    ..catch_body
                        .first()
                        .map(|c| c.span.start)
                        .unwrap_or(s.span.end);
                if clause.start <= clause.end {
                    if let Some(span) = locate_word_in(self.source, &clause, catch_var) {
                        self.record(
                            span,
                            format!(
                                "```saule\n(error) {catch_var}: {ty}\n```",
                                ty = render_type(catch_ty)
                            ),
                        );
                    }
                    self.record_type_idents_in(catch_ty, &clause);
                }
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
}
