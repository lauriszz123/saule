//! Return-type inference for functions and methods that declare none.
//!
//! A `fn` that says `-> T` is taken at its word everywhere. One that says
//! nothing used to be typed `any` by every consumer — the hover popup, the
//! type of `local x = obj.get()`, the checker at the call site — even when
//! the body says plainly what comes back:
//!
//! ```text
//! fn getBlocks()
//!     return self.blocks      -- table<Block>, and always was
//! end
//! ```
//!
//! This pass reads those bodies and fills the gap in. It runs after the
//! class registry is otherwise complete (so `self.blocks` can be looked up)
//! and before the registries are installed, so everything downstream sees
//! one answer.
//!
//! # Deliberately conservative
//!
//! An inferred type is one the author never wrote, so a wrong guess turns
//! into a compile error on a line they didn't touch. Every rule below
//! therefore fails closed: anything this pass cannot type with certainty
//! leaves the return type absent, which is exactly the behaviour that came
//! before it. Specifically it declines to infer when
//!
//! * the function declares a return type — the annotation always wins;
//! * the body has no `return` carrying a value;
//! * any returned expression is one [`Cx::type_of`] does not type;
//! * two returns disagree on more than nullability;
//! * a `return` hands back more than one value.
//!
//! And it widens to `T?` whenever the body can also produce nil — a bare
//! `return`, a `return nil`, or a path that falls off the end (which yields
//! nil at runtime). Under-reporting a type costs a suggestion; claiming a
//! value is non-nil when it can be nil costs a crash.
//!
//! # What it types
//!
//! Literals, `self`, parameters, annotated locals and loop/catch bindings,
//! field reads off any of those, constructor calls, and calls to methods
//! that *declare* a return type. That covers accessor-shaped bodies, which
//! is where unannotated returns overwhelmingly live. It deliberately does
//! not re-implement the typechecker: operators, un-annotated locals, casts
//! and free calls all decline, and the function keeps the `any` it has now.

use saule_ast::{ClassMember, Decl, Expr, Method, Module, Param, Spanned, Stmt, Type};

use crate::registry::{ClassRegistry, FunctionRegistry};

/// Fill in the return type of every method and top-level `fn` in `module`
/// that declared none, in place.
///
/// `classes` must already hold this module's own declarations plus anything
/// spliced in from imports and builtins — a method body reads its own
/// class's fields through it.
pub fn infer_missing_returns(
    module: &Module,
    classes: &mut ClassRegistry,
    funcs: &mut FunctionRegistry,
) {
    // Every inference is computed against the registry as it stands on
    // entry, then applied. Inferring in place instead would let one
    // method's result feed another's, and since methods are held in a
    // `HashMap` the order that happened in — and so the types the module
    // ended up with — would vary between runs.
    let mut method_results: Vec<(String, String, Type)> = Vec::new();
    let mut fn_results: Vec<(String, Type)> = Vec::new();

    for stmt in &module.stmts {
        let Stmt::Decl(d) = &stmt.value else { continue };
        match &d.value {
            Decl::Class { name, members, .. } => {
                for m in members {
                    let ClassMember::Method(meth) = &m.value else {
                        continue;
                    };
                    if !wants_inference(meth) {
                        continue;
                    }
                    let self_class = (!meth.is_static).then_some(name.as_str());
                    if let Some(ty) = infer_body(&meth.body, &meth.params, self_class, classes) {
                        method_results.push((name.clone(), meth.name.clone(), ty));
                    }
                }
            }
            Decl::Function {
                name,
                params,
                return_ty: None,
                body,
                ..
            } => {
                if let Some(ty) = infer_body(body, params, None, classes) {
                    fn_results.push((name.clone(), ty));
                }
            }
            _ => {}
        }
    }

    for (class, method, ty) in method_results {
        if let Some(sig) = classes
            .get_mut(&class)
            .and_then(|info| info.methods.get_mut(&method))
        {
            sig.return_ty = Some(ty);
        }
    }
    for (name, ty) in fn_results {
        if let Some(sig) = funcs.get_mut(&name) {
            sig.return_ty = Some(ty);
        }
    }
}

/// Whether `meth` is a candidate at all.
///
/// A declared return type is the author's word and is never second-guessed.
/// `init` is skipped because a constructor's result is the instance, decided
/// by the language rather than by whatever its body happens to `return`.
fn wants_inference(meth: &Method) -> bool {
    meth.return_ty.is_none() && meth.name != "init"
}

/// The type a body hands back, or `None` when this pass won't commit to one.
fn infer_body(
    body: &[Spanned<Stmt>],
    params: &[Param],
    self_class: Option<&str>,
    classes: &ClassRegistry,
) -> Option<Type> {
    let mut cx = Cx {
        classes,
        self_class,
        scope: params.iter().map(|p| (p.name.clone(), p.ty.clone())).collect(),
    };
    let mut found = Returns::default();
    cx.block(body, &mut found);

    // A multi-value return is a tuple whose shape this pass doesn't try to
    // unify across branches; leave the whole function alone.
    if found.multi {
        return None;
    }
    // Nothing but bare `return`s (or no returns at all) says nothing about
    // a type. The author asked for inference *from* a returned value.
    let mut unified: Option<Type> = None;
    for value in &found.values {
        let ty = value.clone()?;
        unified = Some(match unified {
            None => ty,
            Some(prev) => unify(prev, ty)?,
        });
    }
    let ty = unified?;

    // Nil reaches the caller from three places: `return nil`, a bare
    // `return`, and simply running past the last statement. Any of them
    // makes the result nullable however confident the other paths are.
    if found.nil || !crate::return_check::block_returns(body) {
        return Some(nullable(ty));
    }
    Some(ty)
}

/// What the `return`s in one body add up to.
#[derive(Default)]
struct Returns {
    /// One entry per `return <expr>`; `None` where the expression is one
    /// this pass does not type, which sinks the whole inference.
    values: Vec<Option<Type>>,
    /// A bare `return` or a `return nil` was reached.
    nil: bool,
    /// A `return a, b` was reached.
    multi: bool,
}

struct Cx<'a> {
    classes: &'a ClassRegistry,
    /// The class whose instance `self` names, absent in a static method or
    /// a free function.
    self_class: Option<&'a str>,
    /// Name -> declared type, innermost last. Only bindings that carry a
    /// written annotation are tracked: inferring the type of `local x = …`
    /// is the typechecker's job, and guessing at it here would be the one
    /// way this pass could produce a type the checker disagrees with.
    scope: Vec<(String, Type)>,
}

impl Cx<'_> {
    fn block(&mut self, body: &[Spanned<Stmt>], out: &mut Returns) {
        let mark = self.scope.len();
        for s in body {
            self.stmt(&s.value, out);
        }
        self.scope.truncate(mark);
    }

    fn stmt(&mut self, s: &Stmt, out: &mut Returns) {
        match s {
            Stmt::Return(values) => match values.len() {
                0 => out.nil = true,
                1 => match &values[0].value {
                    Expr::Nil => out.nil = true,
                    e => out.values.push(self.type_of(e)),
                },
                _ => out.multi = true,
            },
            Stmt::Local {
                name,
                ty: Some(t),
                ..
            } => self.scope.push((name.clone(), t.clone())),
            Stmt::LocalMulti { names, .. } => {
                for (name, _, ty) in names {
                    if let Some(t) = ty {
                        self.scope.push((name.clone(), t.clone()));
                    }
                }
            }
            Stmt::If {
                then_block,
                elseifs,
                else_block,
                ..
            } => {
                self.block(then_block, out);
                for (_, b) in elseifs {
                    self.block(b, out);
                }
                if let Some(b) = else_block {
                    self.block(b, out);
                }
            }
            Stmt::While { body, .. } | Stmt::Repeat { body, .. } => self.block(body, out),
            Stmt::ForNumeric {
                var, var_ty, body, ..
            } => {
                let mark = self.scope.len();
                let ty = var_ty.clone().unwrap_or_else(|| named("integer"));
                self.scope.push((var.clone(), ty));
                self.block(body, out);
                self.scope.truncate(mark);
            }
            Stmt::ForIn { vars, body, .. } => {
                let mark = self.scope.len();
                for (name, ty) in vars {
                    if let Some(t) = ty {
                        self.scope.push((name.clone(), t.clone()));
                    }
                }
                self.block(body, out);
                self.scope.truncate(mark);
            }
            Stmt::Try {
                body,
                catch_var,
                catch_ty,
                catch_body,
                ..
            } => {
                self.block(body, out);
                let mark = self.scope.len();
                self.scope.push((catch_var.clone(), catch_ty.clone()));
                self.block(catch_body, out);
                self.scope.truncate(mark);
            }
            // A `match` used as a statement can hold `return`s in its arms.
            Stmt::Expr(e) => {
                if let Expr::Match { arms, .. } = &e.value {
                    for arm in arms {
                        if let saule_ast::MatchBody::Block(b) = &arm.body {
                            self.block(b, out);
                        }
                    }
                }
            }
            // A nested `fn` or class owns its own `return`s, and a lambda
            // in an expression owns its own too — neither says anything
            // about the function being inferred, so nothing here descends
            // into an expression looking for one.
            _ => {}
        }
    }

    /// The type of a returned expression, or `None` to decline.
    fn type_of(&self, e: &Expr) -> Option<Type> {
        match e {
            Expr::Str(_) => Some(named("string")),
            Expr::Int(_) => Some(named("integer")),
            Expr::Float(_) => Some(named("float")),
            Expr::Bool(_) => Some(named("boolean")),
            Expr::Self_ => self.self_class.map(named),
            Expr::Ident(n) => self.lookup(n),
            Expr::Member { obj, name } => self.field(&obj.value, name),
            // `obj?.field` yields nil when the receiver is nil, whatever
            // the field's own type says.
            Expr::SafeMember { obj, name } => Some(nullable(self.field(&obj.value, name)?)),
            Expr::ForceUnwrap(inner) => Some(strip_nullable(self.type_of(&inner.value)?)),
            Expr::Call { callee, .. } => self.call(&callee.value),
            _ => None,
        }
    }

    fn field(&self, obj: &Expr, name: &str) -> Option<Type> {
        let class = self.class_of(obj)?;
        self.classes.get(&class)?.field_types.get(name).cloned()
    }

    fn call(&self, callee: &Expr) -> Option<Type> {
        match callee {
            // `Player()` — a constructor call is the one free call whose
            // result is knowable without a signature table.
            Expr::Ident(n) if self.classes.contains_key(n) => Some(named(n)),
            Expr::Member { obj, name } => {
                let class = self.class_of(&obj.value)?;
                // Only a *declared* return type. A method whose own return
                // type this pass is inferring is not consulted: results are
                // applied together precisely so one cannot depend on
                // another's.
                self.classes.get(&class)?.methods.get(name)?.return_ty.clone()
            }
            _ => None,
        }
    }

    /// The class an expression is an instance of, for a field or method
    /// lookup on it.
    fn class_of(&self, e: &Expr) -> Option<String> {
        match e {
            Expr::Self_ => self.self_class.map(str::to_string),
            // `Player.make()` — a bare class name as the receiver is the
            // class itself, which is how a static method is reached.
            Expr::Ident(n) if self.classes.contains_key(n) && self.lookup(n).is_none() => {
                Some(n.clone())
            }
            other => named_head(&self.type_of(other)?),
        }
    }

    fn lookup(&self, name: &str) -> Option<Type> {
        self.scope
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, t)| t.clone())
    }
}

/// The single type covering both branches, or `None` when they disagree.
///
/// Nullability is the one difference worth reconciling: a body that returns
/// `self.cached` on one path and `nil` on another really does produce `T?`,
/// and that is the shape accessors are written in. Anything else — a
/// `string` here and an `integer` there — is a program this pass has no
/// business putting a type on.
fn unify(a: Type, b: Type) -> Option<Type> {
    if a == b {
        return Some(a);
    }
    if strip_nullable(a.clone()) == strip_nullable(b.clone()) {
        return Some(nullable(strip_nullable(a)));
    }
    None
}

fn named(n: &str) -> Type {
    Type::Named(n.to_string())
}

fn nullable(ty: Type) -> Type {
    match ty {
        already @ Type::Nullable(_) => already,
        other => Type::Nullable(Box::new(other)),
    }
}

fn strip_nullable(ty: Type) -> Type {
    match ty {
        Type::Nullable(inner) => *inner,
        other => other,
    }
}

/// The name a type heads, for looking its declaration up.
fn named_head(ty: &Type) -> Option<String> {
    match ty {
        Type::Named(n) => Some(n.clone()),
        Type::Generic(g) => Some(g.name.clone()),
        Type::Nullable(inner) => named_head(inner),
        _ => None,
    }
}
