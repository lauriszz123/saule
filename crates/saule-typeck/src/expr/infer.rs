//! Lightweight type inference.
//!
//! Returns `Some(ty)` only when we can prove the type. Anything we can't
//! see through (calls, member reads on unknown classes, indexing) returns
//! `None`, and callers treat that as "don't know, don't complain".

use saule_ast::{BinOp, CallArg, Expr, LambdaBody, Spanned, TableEntry, Type, UnaryOp};

use crate::funcs;
use crate::state::{Scope, current_class, with_classes, with_enums};

use super::*;

pub(crate) fn infer(expr: &Spanned<Expr>, scope: &Scope) -> Option<Type> {
    match &expr.value {
        Expr::Nil => Some(Type::Named("nil".into())),
        Expr::Int(_) => Some(Type::Named("integer".into())),
        Expr::Float(_) => Some(Type::Named("float".into())),
        Expr::Bool(_) => Some(Type::Named("boolean".into())),
        Expr::Str(_) => Some(Type::Named("string".into())),
        Expr::Ident(n) => scope.lookup(n).cloned(),
        Expr::Self_ => current_class().map(Type::Named),
        // `x as T` always produces `T?` — the cast is checked at runtime
        // and yields `nil` when the value isn't a `T`. Making the result
        // nullable is what keeps the escape from `any` sound: the caller
        // has to deal with the failure case via `??`, `!`, or a nil test.
        Expr::Cast { ty, .. } => Some(Type::Nullable(Box::new(ty.clone()))),
        // `obj.field` — when `obj` resolves to a known class, return the
        // declared type of that field (walks parents). Methods aren't fields,
        // so this only fires for stored slots declared with `local x: T`.
        Expr::Member { obj, name } => {
            // `Direction.North` — a variant of a known enum has that enum's
            // type. Checked before the general member path because the
            // receiver here is a type name, not a value.
            if let Expr::Ident(enum_name) = &obj.value
                && with_enums(|reg| {
                    reg.get(enum_name)
                        .is_some_and(|e| e.variants.contains_key(name))
                })
            {
                return Some(Type::Named(enum_name.clone()));
            }
            // `Math.pi`, `Os.sep`, `Io.stdout` — a stdlib member holding a
            // value rather than a callable. Checked before the general
            // path because the receiver is a module name, which has no
            // inferable type of its own: falling through would return
            // `None` and every annotated use would be `UndeterminedType`.
            if let Expr::Ident(module) = &obj.value
                && let Some(t) = crate::sigs::lookup_const(&format!("{module}.{name}"))
            {
                return Some(t);
            }
            // `Cursors.requested` — a static member read off the class
            // itself. Like the module case above, the receiver is a *name*
            // and not a value, so `infer` on it answers `None` and the whole
            // access would come back "type unknown". `Class.method(...)`
            // already resolves this way in the `Call` arm; a static *field*
            // read needs the same. A local of the same name wins, which is
            // why the scope is consulted first.
            if let Expr::Ident(class_name) = &obj.value
                && scope.lookup(class_name).is_none()
                && with_classes(|r| r.contains_key(class_name))
            {
                if let Some(t) = saule_semantic::lookup_field_type(class_name, name) {
                    return Some(t);
                }
                if let Some(sig) = saule_semantic::lookup_method(class_name, name) {
                    return Some(Type::Function {
                        params: sig.params.iter().map(|p| p.ty.clone()).collect(),
                        ret: Box::new(sig.return_ty.clone().unwrap_or(Type::Named("any".into()))),
                    });
                }
            }
            let ty = infer(obj, scope)?;
            let stripped = strip_nullable(ty);
            // `t.foo` on a table is Lua map sugar for `t["foo"]`. The value
            // type of a bare `table` is genuinely unknown, so it is `any` —
            // the checker's explicit "could be anything" — rather than an
            // absence of information that would silently skip checks.
            if let Type::Table { value, .. } = &stripped {
                return Some((**value).clone());
            }
            let Type::Named(class_name) = stripped else {
                return Some(Type::Named("any".into()));
            };
            // A member of an `any` is itself `any` — the chain stays
            // explicitly unknown instead of collapsing to "no information".
            //
            // A bare `table` annotation (no element types, e.g. a scratch
            // slot declared `data: table`) lands here as a plain name
            // rather than as `Type::Table`, so it needs the same answer as
            // the branch above: reading a member off it is `any`, not the
            // absence of a type.
            if is_any(&Type::Named(class_name.clone())) || class_name == "table" {
                return Some(Type::Named("any".into()));
            }
            if let Some(t) = saule_semantic::lookup_field_type(&class_name, name) {
                return Some(t);
            }
            // Not a field — fall back to method-as-value: a bare `obj.method`
            // reference (no parens) yields a function value. We surface this
            // so downstream checks (e.g. `match` against a method ref) can
            // detect a missing call.
            let sig = saule_semantic::lookup_method(&class_name, name)?;
            Some(Type::Function {
                params: sig.params.iter().map(|p| p.ty.clone()).collect(),
                ret: Box::new(sig.return_ty.clone().unwrap_or(Type::Named("any".into()))),
            })
        }
        // `obj?.field` — the same lookup as `obj.field`, wrapped in
        // `Nullable` because the whole chain yields `nil` when the
        // receiver is `nil`.
        //
        // This used to answer a flat `any?` regardless of the receiver.
        // That was invisible while an `any` value could flow into any
        // slot; now that it can't, `b?.label` has to report the field's
        // real type or every safe-chain would demand a cast.
        Expr::SafeMember { obj, name } => {
            let inner = infer(
                &Spanned::new(
                    Expr::Member {
                        obj: obj.clone(),
                        name: name.clone(),
                    },
                    expr.span.clone(),
                ),
                scope,
            )
            .unwrap_or_else(|| Type::Named("any".into()));
            Some(Type::Nullable(Box::new(strip_nullable(inner))))
        }
        // `t[k]` — return the declared element type as-is. If the table
        // was declared `table<V?>` the result is nullable and member
        // access on it will trip the nullable-receiver check; if it was
        // declared `table<V>` we trust the declaration and yield `V`.
        Expr::Index { obj, index: _ } => {
            let ty = infer(obj, scope)?;
            match strip_nullable(ty) {
                Type::Table { value, .. } => Some(*value),
                // Indexing a string or an `any` yields something the
                // declaration cannot pin down. `any` says that explicitly.
                _ => Some(Type::Named("any".into())),
            }
        }
        Expr::ForceUnwrap(inner) => match infer(inner, scope)? {
            Type::Nullable(t) => Some(*t),
            other => Some(other),
        },
        // A table literal infers to a typed `table<…>` so a bare `table`
        // annotation can be refined (`local xs: table = {"a"}` → `table<string>`)
        // and a literal passed straight to a native is checked element-wise.
        Expr::Table(items) => Some(infer_table_literal(items, scope)),
        Expr::Binary { op, lhs, rhs } => match op {
            BinOp::Coalesce => {
                // `lhs ?? rhs`: result is non-nullable iff `rhs` is non-nullable.
                match (infer(lhs, scope), infer(rhs, scope)) {
                    (Some(lt), Some(rt)) => {
                        let base = strip_nullable(lt);
                        if is_nullable(&rt) {
                            Some(Type::Nullable(Box::new(base)))
                        } else {
                            Some(base)
                        }
                    }
                    // One side unknown: the coalesce still produces whatever
                    // the other side says, which is better than knowing
                    // nothing about the whole expression.
                    (Some(lt), None) => Some(strip_nullable(lt)),
                    (None, Some(rt)) => Some(rt),
                    (None, None) => Some(Type::Named("any".into())),
                }
            }
            BinOp::Eq | BinOp::NotEq | BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => {
                Some(Type::Named("boolean".into()))
            }
            // Lua semantics: `and`/`or` evaluate to one of their operands,
            // not to a boolean. Same base on both sides is that base;
            // anything else is genuinely a union, reported as `any`.
            BinOp::And | BinOp::Or => match (infer(lhs, scope), infer(rhs, scope)) {
                (Some(lt), Some(rt)) => {
                    let (lb, rb) = (strip_nullable(lt), strip_nullable(rt));
                    if lb == rb {
                        Some(lb)
                    } else {
                        Some(Type::Named("any".into()))
                    }
                }
                _ => Some(Type::Named("any".into())),
            },
            // `..` yields whatever an `OpConcat` overload returns, and a
            // plain `string` otherwise.
            BinOp::Concat => crate::ops::infer_binary(*op, lhs, scope)
                .or_else(|| Some(Type::Named("string".into()))),
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod | BinOp::Pow => {
                crate::ops::infer_binary(*op, lhs, scope)
                    .or_else(|| infer(lhs, scope))
                    .or_else(|| infer(rhs, scope))
            }
        },
        // `Foo(args)` where `Foo` is a known class → produces a `Foo`.
        // Otherwise, look up the qualified callee name in:
        //   1. the local class registry as a user-defined method
        //      (`Foo.bar(...)` → return type of `Foo.bar`); or
        //   2. the native signature table (`String.byte`, `Math.tointeger`,
        //      `assert`).
        Expr::Call { callee, args } => {
            if let Expr::Ident(n) = &callee.value
                && with_classes(|reg| reg.contains_key(n))
            {
                return Some(Type::Named(n.clone()));
            }
            // `Class.method(args)` — receiver is the class itself.
            if let Expr::Member { obj, name } = &callee.value
                && let Expr::Ident(class_name) = &obj.value
                && let Some(sig) = saule_semantic::lookup_method(class_name, name)
            {
                return semantic_method_return(&sig, args, scope);
            }
            // `instance.method(args)` — receiver inferred to a class.
            if let Expr::Member { obj, name } = &callee.value
                && let Some(ty) = infer(obj, scope)
                && let Type::Named(class_name) = strip_nullable(ty)
                && let Some(sig) = saule_semantic::lookup_method(&class_name, name)
            {
                return semantic_method_return(&sig, args, scope);
            }
            if let Some(qname) = native_callee_name(callee, scope)
                && let Some(sig) = crate::sigs::lookup(&qname)
            {
                // Generic native: bind type params from the actual args, then
                // substitute the return list. Falls back to the raw returns
                // for non-generic sigs.
                let returns = if sig.type_params.is_empty() {
                    sig.returns
                } else {
                    let mut subst: std::collections::HashMap<String, Type> =
                        std::collections::HashMap::new();
                    let positional: Vec<&Spanned<Expr>> = args
                        .iter()
                        .filter_map(|a| match a {
                            CallArg::Positional(e) => Some(e),
                            CallArg::Named { .. } => None,
                        })
                        .collect();
                    for (i, expected) in sig.params.iter().enumerate() {
                        let Some(arg_expr) = positional.get(i) else {
                            break;
                        };
                        if let Some(found_ty) = infer(arg_expr, scope) {
                            unify(expected, &found_ty, &sig.type_params, &mut subst);
                        }
                    }
                    sig.returns
                        .iter()
                        .map(|t| substitute(t, &subst, &sig.type_params))
                        .collect()
                };
                return first_or_tuple(returns);
            }
            // A native whose name is known but whose signature is
            // deliberately unregistered — `Math.abs` and friends, whose
            // return kind follows their input. That is `any`, not "no
            // information": the distinction matters because an unknown
            // type now fails the check rather than skipping it.
            if let Expr::Member { obj, name } = &callee.value
                && let Expr::Ident(module) = &obj.value
                && crate::sigs::has_member(module, name)
            {
                return Some(Type::Named("any".into()));
            }
            // A call through a value of function type — `f(x)` where `f`
            // is a parameter or local declared `fn(A) -> R`. Its result is
            // the declared return type; without this the call inferred as
            // `None` and everything downstream of it went unchecked.
            if let Expr::Ident(name) = &callee.value
                && funcs::lookup(name).is_none()
                && crate::sigs::lookup(name).is_none()
                && let Some(ty) = scope.lookup(name)
                && let Type::Function { ret, .. } = strip_nullable(ty.clone())
            {
                return Some(*ret);
            }
            // A call to a top-level user `fn`. Checked last so every rule
            // above keeps priority — in particular a prelude native of the
            // same name still resolves the way it always has.
            //
            // Without this arm the call inferred as `None`, and because
            // `check_assignment_compat` treats `None` as "nothing to check",
            // `local s: string = returns_an_integer()` was accepted silently.
            if let Expr::Ident(name) = &callee.value
                && let Some(info) = crate::funcs::lookup(name)
            {
                let ret = info.return_ty.as_ref()?;
                if info.type_params.is_empty() {
                    return Some(ret.clone());
                }
                // Generic `fn id<T>(x: T) -> T`: explicit type arguments are
                // discarded by the parser, so bind the type params from the
                // actual argument types, exactly as the native path above does.
                let mut subst: std::collections::HashMap<String, Type> =
                    std::collections::HashMap::new();
                let positional: Vec<&Spanned<Expr>> = args
                    .iter()
                    .filter_map(|a| match a {
                        CallArg::Positional(e) => Some(e),
                        CallArg::Named { .. } => None,
                    })
                    .collect();
                for (i, param) in info.params.iter().enumerate() {
                    let Some(arg_expr) = positional.get(i) else {
                        break;
                    };
                    if let Some(found_ty) = infer(arg_expr, scope) {
                        unify(&param.ty, &found_ty, &info.type_params, &mut subst);
                    }
                }
                let substituted = substitute(ret, &subst, &info.type_params);
                // An unbound type param (nothing in the args pinned it down)
                // stays unknown rather than leaking `T` as a concrete name.
                if mentions_unbound_param(&substituted, &info.type_params) {
                    return None;
                }
                return Some(substituted);
            }
            None
        }
        // `obj:method(args)` — same lookup as the `obj.method(args)` case.
        Expr::MethodCall { obj, method, .. } => {
            if let Some(ty) = infer(obj, scope)
                && let Type::Named(class_name) = strip_nullable(ty)
                && let Some(sig) = saule_semantic::lookup_method(&class_name, method)
            {
                return sig.return_ty;
            }
            None
        }
        // A `match` expression has the type of its unified arm bodies.
        // We collect every arm's inferred type and:
        //   * if any arm yields nil / Nullable → the result is `Nullable(base)`;
        //   * if arm bases agree → that base (wrapped if nullable seen);
        //   * if arm bases disagree → `any`;
        //   * if no arm could be inferred → `None` (give up conservatively).
        // A bare `nil` arm contributes nullability but no base: the common
        // `case _ then nil` fallback must not make the arms "disagree" and
        // widen an otherwise uniform `string` result to `any`.
        // Block-bodied arms aren't inferable today; they're skipped, but
        // their presence still widens the result to `any` rather than
        // silently dropping a possibly-different branch.
        Expr::Match { arms, .. } => {
            let mut any_nullable = false;
            let mut bases: Vec<Type> = Vec::new();
            let mut had_block = false;
            for a in arms {
                match &a.body {
                    saule_ast::MatchBody::Expr(e) => {
                        let Some(t) = infer(e, scope) else { continue };
                        if matches!(&t, Type::Named(n) if n == "nil") {
                            // A bare `nil` arm only tells us the result is
                            // nullable — it carries no base of its own, so it
                            // must not count as a disagreeing shape.
                            any_nullable = true;
                            continue;
                        }
                        if is_nullable(&t) {
                            any_nullable = true;
                        }
                        bases.push(strip_nullable(t));
                    }
                    saule_ast::MatchBody::Block(_) => {
                        had_block = true;
                    }
                }
            }
            if bases.is_empty() {
                return None;
            }
            let first = bases[0].clone();
            let same = bases.iter().all(|t| matches_base(t, &first));
            let base = if same && !had_block {
                first
            } else {
                Type::Named("any".into())
            };
            Some(if any_nullable {
                Type::Nullable(Box::new(base))
            } else {
                base
            })
        }
        // `when(x):a():b():c()` has the return type of the last stage's
        // declared function — instantiated against the value that reaches
        // it, so a generic chain answers `table<integer>` rather than the
        // last stage's own parameter name.
        Expr::Pipe { source, stages } => infer_pipe(source, stages, scope),
        // `-x` keeps the operand's numeric type, `#x` is always an integer
        // count, `not x` is always a boolean — unless the operand is a
        // class overloading `OpNeg` / `OpLen`, which names its own result.
        Expr::Unary { op, rhs } => crate::ops::infer_unary(*op, rhs, scope).or_else(|| match op {
            UnaryOp::Neg => infer(rhs, scope).map(strip_nullable),
            UnaryOp::Len => Some(Type::Named("integer".into())),
            UnaryOp::Not => Some(Type::Named("boolean".into())),
        }),
        // A lambda is a function value. Parameters carry their declared
        // types (`any` when the writer left them off); the return type comes
        // from the annotation when present, and from the body otherwise.
        Expr::Lambda {
            params,
            return_ty,
            body,
        } => {
            let ret = return_ty.clone().or_else(|| match body {
                LambdaBody::Expr(e) => infer(e, scope),
                LambdaBody::Block(_) => None,
            });
            Some(Type::Function {
                params: params.iter().map(|p| p.ty.clone()).collect(),
                ret: Box::new(ret.unwrap_or(Type::Named("any".into()))),
            })
        }
    }
}

pub(crate) fn strip_nullable(ty: Type) -> Type {
    match ty {
        Type::Nullable(t) => *t,
        other => other,
    }
}

/// Infer the static type of a table literal.
///
/// * All-positional (`{a, b, c}`) → `table<T>` when every element shares the
///   same inferred base type, otherwise `table<any>`.
/// * All-field (`{x: a, y: b}`) → `table<string, V>` with `V` unified the same
///   way.
/// * Empty (`{}`) or mixed positional+field → `table<any>` (element type
///   can't be pinned down without false positives).
///
/// Any element whose type can't be inferred widens the result to `any`, so the
/// outcome is always at least as permissive as the old `None` behaviour.
pub(crate) fn infer_table_literal(items: &[TableEntry], scope: &Scope) -> Type {
    let mut has_positional = false;
    let mut has_field = false;
    let mut elem: Option<Type> = None;
    let mut unknown = false;

    for item in items {
        let value_expr = match item {
            TableEntry::Positional(e) => {
                has_positional = true;
                e
            }
            TableEntry::Field { value, .. } => {
                has_field = true;
                value
            }
        };
        match infer(value_expr, scope) {
            Some(t) => {
                let base = strip_nullable(t);
                elem = match elem {
                    None => Some(base),
                    Some(prev) if matches_base(&prev, &base) => Some(prev),
                    Some(_) => Some(Type::Named("any".into())),
                };
            }
            None => unknown = true,
        }
    }

    let value_ty = match elem {
        Some(t) if !unknown => t,
        _ => Type::Named("any".into()),
    };

    if has_field && !has_positional {
        Type::Table {
            key: Some(Box::new(Type::Named("string".into()))),
            value: Box::new(value_ty),
        }
    } else if has_positional && !has_field {
        Type::Table {
            key: None,
            value: Box::new(value_ty),
        }
    } else {
        // Empty or mixed — known to be a table, element type left open.
        Type::Table {
            key: None,
            value: Box::new(Type::Named("any".into())),
        }
    }
}

/// Structural equality on stripped bases — used by `Match` inference to
/// decide whether all arms produce the same shape (in which case we keep
/// that type) or whether to widen to `any`.
pub(crate) fn matches_base(a: &Type, b: &Type) -> bool {
    match (a, b) {
        (Type::Named(x), Type::Named(y)) => x == y,
        (Type::Nullable(x), Type::Nullable(y)) => matches_base(x, y),
        (Type::Tuple(xs), Type::Tuple(ys)) => {
            xs.len() == ys.len() && xs.iter().zip(ys).all(|(a, b)| matches_base(a, b))
        }
        (Type::Table { key: kx, value: vx }, Type::Table { key: ky, value: vy }) => {
            let key_ok = match (kx, ky) {
                (None, None) => true,
                (Some(a), Some(b)) => matches_base(a, b),
                _ => false,
            };
            key_ok && matches_base(vx, vy)
        }
        (
            Type::Function {
                params: px,
                ret: rx,
            },
            Type::Function {
                params: py,
                ret: ry,
            },
        ) => {
            px.len() == py.len()
                && px.iter().zip(py).all(|(a, b)| matches_base(a, b))
                && matches_base(rx, ry)
        }
        _ => false,
    }
}

/// Build a qualified callee name suitable for `stdlib::sigs::lookup`:
/// `assert`, `String.byte`, or — for instance calls on stdlib value types
/// — `File.read` (resolved by looking at the receiver's inferred type).
pub(crate) fn native_callee_name(callee: &Spanned<Expr>, scope: &Scope) -> Option<String> {
    match &callee.value {
        Expr::Ident(n) => Some(n.clone()),
        Expr::Member { obj, name } => {
            // Prefer the receiver Ident as a module name only when it
            // actually denotes a known module / value-type. Otherwise
            // (e.g. `file.read` where `file` is a local of type `File`)
            // fall through to the inferred-type path below so we build
            // `File.read`, not `file.read`.
            if let Expr::Ident(class) = &obj.value
                && (crate::sigs::is_module(class) || crate::sigs::lookup(class).is_some())
            {
                return Some(format!("{class}.{name}"));
            }
            // `instance.method(...)` where `instance` has a stdlib
            // value-type (e.g. `File`). Build the qname so the existing
            // sig-based arg / return checks apply to instance methods
            // the same way they do for static `Class.method` calls.
            let ty = infer(obj, scope)?;
            if let Type::Named(n) = strip_nullable(ty)
                && crate::sigs::is_value_type(&n)
            {
                return Some(format!("{n}.{name}"));
            }
            None
        }
        _ => None,
    }
}

/// Collapse a returns-list into a single inferred type: single returns stay
/// as-is, multi-returns become a `Type::Tuple`.
pub(crate) fn first_or_tuple(returns: Vec<Type>) -> Option<Type> {
    match returns.len() {
        0 => None,
        1 => returns.into_iter().next(),
        _ => Some(Type::Tuple(returns)),
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Flow narrowing.
//
// `narrow_truthy(cond, scope)` — apply the assumptions that hold when `cond`
// is true. Today: `x != nil` and `nil != x` strip `Nullable` off `x`. `and`
// chains compose.
//
// `narrow_falsy(cond, scope)` — apply the assumptions when `cond` is false.
// Used by the else-branch: `if x == nil then ... else ... end` narrows `x`
// in the else.
// ──────────────────────────────────────────────────────────────────────────────
