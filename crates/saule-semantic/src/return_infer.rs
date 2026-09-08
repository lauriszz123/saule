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
//! Literals, `self`, parameters, locals and loop/catch bindings, field and
//! index reads off any of those, constructor calls, enum variants, calls to
//! sibling methods and top-level `fn`s, calls to natives (`String.trim`,
//! `Math.floor`, `print`), and every operator the language has. A local
//! without an annotation is typed from its initializer by these same rules,
//! so it can only carry a type this pass would have committed to anyway.
//!
//! Helpers that hand back what other helpers built resolve too — see the
//! rounds in [`infer_missing_returns`], which is what lets `heading()`
//! reach the `Block` that `recordHeading()` returns.
//!
//! The operator rules mirror the checker's own, reached through two tables
//! that had to move before this pass could see them: natives come from
//! [`saule_sigs`], now a crate *below* this one, and operator overloads from
//! [`crate::ops`], moved *up* out of `saule-typeck`. Where the checker must
//! answer with `any`, this declines instead — an inferred `any` is worth
//! nothing and costs the chance to be right later.
//!
//! It still does not re-implement the typechecker. `x as T` declines: the
//! cast-result rule reads the generics in scope during checking, which is
//! checker state and not a table that can move.

use saule_ast::{
    BinOp, CallArg, ClassMember, Decl, Expr, Method, Module, Param, Spanned, Stmt, Type, UnaryOp,
};

use crate::ops::Decls;
use crate::registry::{ClassRegistry, EnumRegistry, FunctionRegistry, InterfaceRegistry};

/// Fill in the return type of every method and top-level `fn` in `module`
/// that declared none, in place.
///
/// `classes` must already hold this module's own declarations plus anything
/// spliced in from imports and builtins — a method body reads its own
/// class's fields through it.
pub fn infer_missing_returns(
    module: &Module,
    classes: &mut ClassRegistry,
    interfaces: &InterfaceRegistry,
    enums: &EnumRegistry,
    funcs: &mut FunctionRegistry,
) {
    let mut methods: Vec<Candidate<'_>> = Vec::new();
    let mut functions: Vec<Candidate<'_>> = Vec::new();
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
                    methods.push(Candidate {
                        owner: name,
                        name: &meth.name,
                        params: &meth.params,
                        body: &meth.body,
                        self_class: (!meth.is_static).then_some(name.as_str()),
                    });
                }
            }
            Decl::Function {
                name,
                params,
                return_ty: None,
                body,
                ..
            } => functions.push(Candidate {
                owner: name,
                name,
                params,
                body,
                self_class: None,
            }),
            _ => {}
        }
    }

    // Settle by repeated rounds rather than in one pass, because a helper
    // very often hands back what another helper built: `heading()` returns
    // `self.recordHeading(…)`, which returns a `Block`. Reading only
    // *declared* return types left every such caller untyped, and reading
    // inferred ones as they land would make the answer depend on the order
    // methods happen to sit in a `HashMap`.
    //
    // Each round reads the registries as they stood when it began and
    // applies its results at the end, so a round can only ever see types
    // settled by *earlier* rounds. The fixed point that reaches is the same
    // whatever order the candidates are visited in. A round that settles
    // nothing ends the loop, so a recursive helper — whose type depends on
    // itself and can never settle — simply keeps no type, as before.
    for _ in 0..MAX_ROUNDS {
        let mut method_results: Vec<(String, String, Type)> = Vec::new();
        let mut fn_results: Vec<(String, Type)> = Vec::new();
        for c in &methods {
            if let Some(ty) = c.infer(classes, interfaces, enums, funcs) {
                method_results.push((c.owner.clone(), c.name.clone(), ty));
            }
        }
        for c in &functions {
            if let Some(ty) = c.infer(classes, interfaces, enums, funcs) {
                fn_results.push((c.name.clone(), ty));
            }
        }
        if method_results.is_empty() && fn_results.is_empty() {
            break;
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
        // Anything that settled is done; the next round retries only what
        // is still undecided, so a chain of depth d costs d passes over a
        // shrinking list rather than d passes over all of it.
        methods.retain(|c| c.unsettled_method(classes));
        functions.retain(|c| c.unsettled_fn(funcs));
        if methods.is_empty() && functions.is_empty() {
            break;
        }
    }
}

/// How many times [`infer_missing_returns`] will re-examine what is still
/// undecided.
///
/// One round settles a helper, the next settles its caller, and so on, so
/// this is the longest chain of unannotated helpers that resolves. Real
/// chains are two or three deep; the cap is what keeps a pathological file
/// from turning this into quadratic work on every keystroke.
const MAX_ROUNDS: usize = 16;

/// One function or method whose return type is still to be decided.
struct Candidate<'a> {
    /// The class a method belongs to; its own name for a free function.
    owner: &'a String,
    name: &'a String,
    params: &'a [Param],
    body: &'a [Spanned<Stmt>],
    self_class: Option<&'a str>,
}

impl Candidate<'_> {
    fn infer(
        &self,
        classes: &ClassRegistry,
        interfaces: &InterfaceRegistry,
        enums: &EnumRegistry,
        funcs: &FunctionRegistry,
    ) -> Option<Type> {
        infer_body(
            self.body,
            self.params,
            self.self_class,
            classes,
            interfaces,
            enums,
            funcs,
        )
    }

    fn unsettled_method(&self, classes: &ClassRegistry) -> bool {
        classes
            .get(self.owner)
            .and_then(|info| info.methods.get(self.name))
            .is_some_and(|sig| sig.return_ty.is_none())
    }

    fn unsettled_fn(&self, funcs: &FunctionRegistry) -> bool {
        funcs
            .get(self.name)
            .is_some_and(|sig| sig.return_ty.is_none())
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
    interfaces: &InterfaceRegistry,
    enums: &EnumRegistry,
    funcs: &FunctionRegistry,
) -> Option<Type> {
    let mut cx = Cx {
        classes,
        decls: Decls::Pending {
            classes,
            interfaces,
        },
        enums,
        funcs,
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
    /// The same declarations, in the shape operator-overload lookup wants.
    /// It must be told to read *these* rather than the installed
    /// thread-locals: this pass runs while the registries are still being
    /// built, so the installed ones still describe the previous module.
    decls: Decls<'a>,
    /// Enums are a separate registry from classes, and a variant reaches
    /// this pass as an ordinary member access on the enum's name.
    enums: &'a EnumRegistry,
    /// Top-level `fn`s, so a call to a sibling helper resolves. These are
    /// the file's own functions; the *native* signature table is in
    /// `saule-typeck` and out of reach, so `String.trim(…)` still declines.
    funcs: &'a FunctionRegistry,
    /// The class whose instance `self` names, absent in a static method or
    /// a free function.
    self_class: Option<&'a str>,
    /// Name -> type, innermost last. A binding's written annotation when it
    /// has one, and otherwise whatever [`Cx::type_of`] makes of its
    /// initializer — the same rules that type a `return`, so a local never
    /// carries a type this pass would have declined to infer directly.
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
                name, ty, value, ..
            } => {
                // An annotation is the author's word. Failing that, take
                // what the initializer produces — by the same rules that
                // type a `return`, so a local can only carry a type this
                // pass would have committed to anyway. `local counter = 0`
                // followed by `return counter` is the ordinary shape of a
                // function that counts something, and tracking only
                // annotated locals left every one of them untyped.
                let resolved = match ty {
                    Some(t) => Some(t.clone()),
                    None => value.as_ref().and_then(|v| self.type_of(&v.value)),
                };
                if let Some(t) = resolved {
                    self.scope.push((name.clone(), t));
                }
            }
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
            Expr::Call { callee, args, .. } => self.call(&callee.value, args),
            // `t[i]` on a table is its element type. Indexing anything else
            // is a question for the checker.
            Expr::Index { obj, .. } => match strip_nullable(self.type_of(&obj.value)?) {
                Type::Table { value, .. } => Some(*value),
                _ => None,
            },
            Expr::Unary { op, rhs } => self.unary(*op, &rhs.value),
            // A comparison is a boolean whatever it compares — the checker
            // types these unconditionally, and no overload redirects them.
            // The other operators follow their operands, or an overload the
            // contract table in the typechecker resolves, and that table is
            // downstream of this crate. They decline.
            Expr::Binary { op, lhs, rhs } => match op {
                BinOp::Eq | BinOp::NotEq | BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => {
                    Some(named("boolean"))
                }
                // `a ?? b` drops the left side's nullability — that is what
                // the operator is for — and is nullable again only when the
                // fallback is. No contract overloads it, so the rule holds
                // whatever the operands are.
                BinOp::Coalesce => {
                    let base = strip_nullable(self.type_of(&lhs.value)?);
                    Some(match &rhs.value {
                        Expr::Nil => nullable(base),
                        other => match self.type_of(other) {
                            Some(Type::Nullable(_)) => nullable(base),
                            _ => base,
                        },
                    })
                }
                // Lua semantics: `and` / `or` evaluate to one of their
                // *operands*, not to a boolean. Operands that disagree are a
                // genuine union — decline rather than pick a side.
                BinOp::And | BinOp::Or => {
                    let (l, r) = (self.type_of(&lhs.value)?, self.type_of(&rhs.value)?);
                    let (lb, rb) = (strip_nullable(l), strip_nullable(r));
                    (lb == rb).then_some(lb)
                }
                // `..` yields whatever an `OpConcat` overload returns, and a
                // plain `string` otherwise.
                BinOp::Concat => self
                    .binary_overload(*op, &lhs.value)
                    .or_else(|| Some(named("string"))),
                // Arithmetic dispatches on the left operand's overload, else
                // takes whichever side can be typed — `integer + integer` is
                // an `integer`, `float + integer` a `float`.
                BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod | BinOp::Pow => self
                    .binary_overload(*op, &lhs.value)
                    .or_else(|| self.type_of(&lhs.value))
                    .or_else(|| self.type_of(&rhs.value)),
                // Bitwise operands and results are both `integer`, so unlike
                // arithmetic there is no operand kind to follow.
                BinOp::BAnd | BinOp::BOr | BinOp::BXor | BinOp::Shl | BinOp::Shr => self
                    .binary_overload(*op, &lhs.value)
                    .or_else(|| Some(named("integer"))),
            },
            _ => None,
        }
    }

    /// Result of `lhs op …` when `lhs` is a class that overloads `op`.
    fn binary_overload(&self, op: BinOp, lhs: &Expr) -> Option<Type> {
        crate::ops::overload_binary_result_in(self.decls, op, &self.type_of(lhs)?)
    }

    /// Type of `op rhs`, mirroring the checker.
    ///
    /// An `OpNeg` / `OpLen` / `OpBNot` overload names its own result and
    /// wins; otherwise `-x` keeps the operand's type with any nullability
    /// stripped, `#x` and `~x` count, and `not x` is a boolean.
    fn unary(&self, op: UnaryOp, rhs: &Expr) -> Option<Type> {
        if let Some(ty) = self
            .type_of(rhs)
            .and_then(|t| crate::ops::overload_unary_result_in(self.decls, op, &t))
        {
            return Some(ty);
        }
        match op {
            UnaryOp::Not => Some(named("boolean")),
            UnaryOp::Len | UnaryOp::BNot => Some(named("integer")),
            UnaryOp::Neg => self.type_of(rhs).map(strip_nullable),
        }
    }

    fn field(&self, obj: &Expr, name: &str) -> Option<Type> {
        // `Colour.Red` — a payload-free variant used as a value is the enum
        // it belongs to, not a field of anything.
        if let Some(en) = self.variant_owner(obj, name) {
            return Some(named(&en));
        }
        let class = self.class_of(obj)?;
        self.classes.get(&class)?.field_types.get(name).cloned()
    }

    fn call(&self, callee: &Expr, args: &[CallArg]) -> Option<Type> {
        match callee {
            // `Player()` — a constructor call.
            Expr::Ident(n) if self.classes.contains_key(n) => Some(named(n)),
            Expr::Ident(n) if self.lookup(n).is_none() => {
                // `runLength(t, '#')` — a sibling top-level `fn`, whose own
                // return type may itself have been inferred in an earlier
                // round. A local of the same name shadows it, hence the
                // scope check above.
                if let Some(sig) = self.funcs.get(n) {
                    return sig.return_ty.clone();
                }
                // `print(x)`, `tostring(v)` — a prelude native.
                self.native(n, args)
            }
            Expr::Member { obj, name } => {
                // `Block.Heading(level, slug, …)` — constructing a tuple
                // variant produces the *enum*. Enums live in their own
                // registry, so the class lookup below misses them entirely,
                // and a body whose whole job is to build one variant —
                // which is most of what a parser's helpers do — inferred
                // nothing at all.
                if let Some(en) = self.variant_owner(&obj.value, name) {
                    return Some(named(&en));
                }
                // `String.trim(s)`, `Math.floor(n)` — a stdlib module's
                // native. Tried before the class path because these modules
                // are not classes and would miss it entirely; a user class
                // or a local of the same name takes precedence.
                if let Expr::Ident(m) = &obj.value
                    && self.lookup(m).is_none()
                    && !self.classes.contains_key(m)
                    && let Some(ty) = self.native(&format!("{m}.{name}"), args)
                {
                    return Some(ty);
                }
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

    /// The return type of the native `qname`, with its generics bound from
    /// whatever the arguments turn out to be.
    ///
    /// A type parameter the arguments never pinned down comes back as its
    /// own bare name — `table<U>` rather than a type anyone wrote — so that
    /// is treated as unknown and declines, the same guard the checker and
    /// the LSP apply to the same table.
    fn native(&self, qname: &str, args: &[CallArg]) -> Option<Type> {
        let sig = saule_sigs::lookup(qname)?;
        let arg_types: Vec<Option<Type>> = args
            .iter()
            .filter_map(|a| match a {
                CallArg::Positional(e) => Some(self.type_of(&e.value)),
                // Named arguments bind no generics here, mirroring the
                // checker. A non-generic native is unaffected, and a generic
                // one simply goes unpinned and declines below.
                CallArg::Named { .. } => None,
            })
            .collect();
        let ret = saule_sigs::instantiate_returns(&sig, &arg_types)
            .into_iter()
            .next()?;
        (!saule_sigs::mentions_unbound_param(&ret, &sig.type_params)).then_some(ret)
    }

    /// The enum `obj.name` names a variant of, when `obj` is a bare enum
    /// name that nothing in scope shadows and `name` is one of its variants.
    fn variant_owner(&self, obj: &Expr, name: &str) -> Option<String> {
        let Expr::Ident(n) = obj else { return None };
        if self.lookup(n).is_some() {
            return None;
        }
        self.enums
            .get(n)
            .filter(|info| info.variants.contains_key(name))
            .map(|_| n.clone())
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
