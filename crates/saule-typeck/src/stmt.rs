//! Statement & declaration walker. Threads a [`Scope`](super::state::Scope)
//! so we know the static type of every `local` seen on the current path, and
//! so narrowing in `if`/`else` can override types for the duration of a
//! sub-block.

mod assign;
mod decls;

pub(crate) use assign::*;
pub(crate) use decls::*;

use saule_ast::{Expr, Spanned, Stmt, Type};

use super::TypeCheckError;
use super::expr::{
    check_assignment_compat, check_assignment_compat_coercing, check_boolean_cond,
    check_element_compat, check_expr, check_expr_expecting, check_table_key_compat, infer,
    is_nullable, narrow_falsy, narrow_truthy, type_to_string,
};
use super::state::{Scope, class_implements_iterable, current_return_ty, with_classes};
use super::to_source_span;

/// How many values a lone right-hand side supplies to a parallel binding,
/// when that is knowable.
///
/// `local a, b = f()` spreads a multi-return across the names, so what
/// decides whether every name gets a value is the callee's arity, not the
/// number of expressions written. A resolved tuple supplies one value per
/// component; any other resolved type supplies exactly one, which is the
/// case that used to slip through — `fn one() -> integer` fills the first
/// name and leaves the rest `nil`, in a slot whose type says that cannot
/// happen.
///
/// `None` means the front end could not resolve the expression at all. The
/// names are then left alone rather than reported against a guess, since a
/// multi-return whose type we cannot see still fills them at run time.
fn spread_arity(v: &Spanned<Expr>, scope: &Scope) -> Option<usize> {
    match infer(v, scope) {
        Some(Type::Tuple(ts)) => Some(ts.len()),
        Some(_) => Some(1),
        None => None,
    }
}

/// Report every assignment target past `supplied` whose type forbids `nil`.
///
/// A parallel assignment with more targets than values leaves the extra ones
/// `nil`. Catching it here is what keeps the VM's type-tag guard from being
/// the first thing to notice — by then the only report it can make is an
/// internal one, about a chunk disagreeing with the types it was compiled
/// against.
fn report_unsupplied(
    targets: &[Spanned<Expr>],
    supplied: usize,
    scope: &Scope,
    stmt: &Spanned<Stmt>,
    errors: &mut Vec<TypeCheckError>,
) {
    for target in targets.iter().skip(supplied) {
        if let Expr::Ident(n) = &target.value
            && let Some(ty) = scope.lookup(n)
            && !is_nullable(ty)
        {
            errors.push(TypeCheckError::NilToNonNullable {
                ty: type_to_string(ty),
                span: to_source_span(stmt.span.clone()),
            });
        }
    }
}

pub(super) fn check_stmt(
    stmt: &Spanned<Stmt>,
    scope: &mut Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    match &stmt.value {
        Stmt::Error => {}

        Stmt::Decl(decl) => check_decl(&decl.value, decl.span.clone(), errors),

        Stmt::Local {
            name, ty, value, ..
        } => {
            if let Some(t) = ty {
                check_binding_type(t, stmt.span.clone(), errors);
            }
            // A name the language has no type for cannot constrain this
            // binding. `check_binding_type` has already reported it; going
            // on to check the value against it too would contradict that
            // error — see [`is_non_type`]. Drop the annotation and treat
            // the binding as untyped.
            let ty = ty.as_ref().filter(|t| !is_non_type(t));
            if let (Some(ty), Some(v)) = (ty, value) {
                check_expr_expecting(v, Some(ty), scope, errors);
                // An annotated `local` is one of the sites the interpreter
                // converts at, so `Assignable` may apply here.
                check_assignment_compat_coercing(ty, v, scope, errors);
                // Refine the bare structural annotation `table`
                // to the value's concrete shape — e.g.
                // `local args: table = Os.args()` widens to `table<string>`
                // so `args[i] = 10` then errors. Without this, the bare
                // name passes assignment-compat (everything is a `table`)
                // but loses the element type for downstream checks.
                let bound = refine_bare_binding(ty, v, scope);
                scope.bind(name.clone(), bound);
            } else if let Some(v) = value {
                check_expr(v, scope, errors);
                if let Some(t) = infer(v, scope) {
                    scope.bind(name.clone(), t);
                }
            } else if let Some(ty) = ty {
                // `local x: T` with no initializer is implicitly `nil`.
                // Reject when `T` isn't nullable so the user has to either
                // mark the type `T?` or supply a value up front.
                if !is_nullable(ty) {
                    errors.push(TypeCheckError::NilToNonNullable {
                        ty: type_to_string(ty),
                        span: to_source_span(stmt.span.clone()),
                    });
                }
                scope.bind(name.clone(), ty.clone());
            } else {
                // Annotation dropped above, no initializer: the name still
                // has to exist for the rest of the scope, or the one real
                // error is followed by a pile of "unknown variable".
                scope.bind(name.clone(), Type::Named("any".into()));
            }
        }

        Stmt::LocalMulti { names, values } => {
            for (_, _, ty_opt) in names {
                if let Some(t) = ty_opt {
                    check_binding_type(t, stmt.span.clone(), errors);
                }
            }
            for v in values {
                check_expr(v, scope, errors);
            }

            // Single-RHS tuple destructuring: `local a, b = f()` where `f()`
            // returns `(A, B)`. Distribute the tuple components across the
            // bindings instead of comparing the whole tuple to each one.
            let tuple_spread: Option<(Vec<Type>, std::ops::Range<usize>)> =
                if values.len() == 1 && names.len() > 1 {
                    let v = &values[0];
                    match infer(v, scope) {
                        Some(Type::Tuple(ts)) => Some((ts, v.span.clone())),
                        _ => None,
                    }
                } else {
                    None
                };

            // Decided before the loop binds anything, so a name being
            // introduced cannot change what the right-hand side infers to.
            let unknown_spread = values.len() == 1
                && names.len() > 1
                && matches!(values[0].value, Expr::Call { .. })
                && spread_arity(&values[0], scope).is_none();

            if let Some((ts, vspan)) = tuple_spread {
                for (i, (name, _, ty_opt)) in names.iter().enumerate() {
                    let found = ts.get(i).cloned();
                    if let (Some(ty), Some(found_ty)) = (ty_opt, found.as_ref()) {
                        check_type_assignment_compat(ty, found_ty, vspan.clone(), errors);
                    }
                    // Past the end of the tuple: the call has no value for
                    // this name, so it binds `nil` — reported here rather
                    // than left to the VM's type-tag guard, which can only
                    // call it an internal error by the time it sees it.
                    if found.is_none()
                        && let Some(ty) = ty_opt
                        && !is_nullable(ty)
                    {
                        errors.push(TypeCheckError::NilToNonNullable {
                            ty: type_to_string(ty),
                            span: to_source_span(stmt.span.clone()),
                        });
                    }
                    let bound = match (ty_opt, found) {
                        (Some(ty), _) => ty.clone(),
                        (None, Some(t)) => t,
                        (None, None) => Type::Named("nil".into()),
                    };
                    scope.bind(name.clone(), bound);
                }
                return;
            }

            for (i, (name, _, ty_opt)) in names.iter().enumerate() {
                if let (Some(ty), Some(v)) = (ty_opt, values.get(i)) {
                    check_expr_expecting(v, Some(ty), scope, errors);
                    check_assignment_compat(ty, v, scope, errors);
                }
                if let Some(ty) = ty_opt {
                    let bound = match values.get(i) {
                        Some(v) => refine_bare_binding(ty, v, scope),
                        None => {
                            // Fewer values than names — this one binds `nil`,
                            // same as a `local x: T` with no initializer.
                            // A lone call as the RHS is exempt only while
                            // its arity is genuinely unknown — a native or an
                            // unresolved callee, which still fills these names
                            // at run time. A call we *can* see through is not
                            // exempt: `fn one() -> integer` supplies one
                            // value, and the names after it are as empty as a
                            // literal would leave them. Only a call spreads at
                            // all — a literal or a variable never does.
                            if !is_nullable(ty) && !unknown_spread {
                                errors.push(TypeCheckError::NilToNonNullable {
                                    ty: type_to_string(ty),
                                    span: to_source_span(stmt.span.clone()),
                                });
                            }
                            ty.clone()
                        }
                    };
                    scope.bind(name.clone(), bound);
                } else if let Some(v) = values.get(i)
                    && let Some(t) = infer(v, scope)
                {
                    scope.bind(name.clone(), t);
                }
            }
        }

        Stmt::Assign { target, value } => {
            check_expr(target, scope, errors);
            // Walked against the target's declared type, which is what gives
            // an untyped lambda its parameter types: `x.onChanged = t => ...`
            // takes `t` from the field's own `fn(string) -> nil`, exactly as
            // `local f: fn(string) -> nil = t => ...` already did. Without
            // it the parameter fell back to `any` and the body went
            // unchecked.
            //
            // For a bare name that is the *declared* type rather than the
            // narrowed one, for the same reason `check_write_to_target` uses
            // it: the expectation belongs to the slot, not to the value
            // about to be replaced.
            let expected = match &target.value {
                Expr::Ident(n) => scope.declared(n).cloned(),
                _ => None,
            }
            .or_else(|| infer(target, scope));
            check_expr_expecting(value, expected.as_ref(), scope, errors);
            check_write_to_target(target, value, scope, errors);
            reassigned(target, value, scope);
        }

        Stmt::CompoundAssign { target, op, value } => {
            // `a op= b` is typed as `a = a op b`. Building that binary node
            // is what lets the operator's own rules apply unchanged —
            // numeric-only for `+`, string-or-numeric for `..`, `Op*`
            // overloads for class instances — and gives the target's
            // declared type something with the *result* type to check
            // against, so `local n: integer = 1; n /= 2` is caught the same
            // way `n = n / 2` is.
            //
            // `check_expr` on the synthetic node recurses into both operands,
            // so target and value are checked here too; checking them again
            // separately would double every diagnostic they produce.
            let combined = Spanned::new(
                Expr::Binary {
                    op: *op,
                    lhs: Box::new(target.clone()),
                    rhs: Box::new(value.clone()),
                },
                target.span.start..value.span.end,
            );
            check_expr(&combined, scope, errors);
            check_write_to_target(target, &combined, scope, errors);
            reassigned(target, &combined, scope);
        }

        Stmt::AssignMulti { targets, values } => {
            for t in targets {
                check_expr(t, scope, errors);
            }
            for v in values {
                check_expr(v, scope, errors);
            }

            // How many values a lone right-hand side supplies, which is
            // what decides whether every target gets one. Same question as
            // `local a, b = f()` — see [`spread_arity`] — and the same
            // answer, since the two forms spread identically.
            let supplied = if values.len() == 1 && targets.len() > 1 {
                spread_arity(&values[0], scope)
            } else {
                Some(values.len())
            };

            // Single-RHS tuple destructuring on the assignment form.
            if values.len() == 1
                && targets.len() > 1
                && let Some(Type::Tuple(ts)) = infer(&values[0], scope)
            {
                let vspan = values[0].span.clone();
                for (i, target) in targets.iter().enumerate() {
                    if let Expr::Ident(n) = &target.value
                        && let (Some(ty), Some(found_ty)) = (scope.lookup(n).cloned(), ts.get(i))
                    {
                        check_type_assignment_compat(&ty, found_ty, vspan.clone(), errors);
                    }
                }
                report_unsupplied(targets, ts.len(), scope, stmt, errors);
                return;
            }

            for (i, target) in targets.iter().enumerate() {
                if let Expr::Ident(n) = &target.value
                    && let (Some(ty), Some(v)) = (scope.lookup(n).cloned(), values.get(i))
                {
                    check_assignment_compat(&ty, v, scope, errors);
                }
            }
            // A target with no value left to take is assigned `nil`, which
            // its declared type may well forbid. Skipped where the arity is
            // unknowable, for the reason `spread_arity` gives.
            if let Some(n) = supplied {
                report_unsupplied(targets, n, scope, stmt, errors);
            }
        }

        Stmt::Expr(e) => check_expr(e, scope, errors),

        Stmt::If {
            cond,
            then_block,
            elseifs,
            else_block,
        } => {
            check_expr(cond, scope, errors);
            check_boolean_cond("if", cond, scope, errors);

            // Branch the scope so narrowing in the then-block doesn't leak.
            let mut then_scope = scope.clone();
            narrow_truthy(cond, &mut then_scope);
            for s in then_block {
                check_stmt(s, &mut then_scope, errors);
            }

            for (econd, ebody) in elseifs {
                check_expr(econd, scope, errors);
                check_boolean_cond("elseif", econd, scope, errors);
                let mut ei_scope = scope.clone();
                narrow_truthy(econd, &mut ei_scope);
                for s in ebody {
                    check_stmt(s, &mut ei_scope, errors);
                }
            }

            if let Some(block) = else_block {
                let mut else_scope = scope.clone();
                narrow_falsy(cond, &mut else_scope);
                for s in block {
                    check_stmt(s, &mut else_scope, errors);
                }
            }

            // Early-exit narrowing: when a branch always diverges (every
            // path ends in return/throw/break/continue), the opposite
            // assumption holds in code that follows the `if`. This makes
            // the common guard idiom work:
            //
            //   if x == nil then return end
            //   -- x is non-nil from here on
            //
            // Only handles the cases without elseifs to keep the analysis
            // small and obviously correct.
            if elseifs.is_empty() {
                let then_diverges = block_diverges(then_block);
                match else_block {
                    None if then_diverges => narrow_falsy(cond, scope),
                    Some(block) if block_diverges(block) && !then_diverges => {
                        narrow_truthy(cond, scope);
                    }
                    _ => {}
                }
            }
        }

        Stmt::While { cond, body } => {
            check_expr(cond, scope, errors);
            check_boolean_cond("while", cond, scope, errors);
            // Reaching the body means the condition held, so the body sees
            // whatever that proves.
            let mut body_scope = scope.clone();
            narrow_truthy(cond, &mut body_scope);
            for s in body {
                check_stmt(s, &mut body_scope, errors);
            }
        }

        Stmt::Repeat { body, cond } => {
            check_expr(cond, scope, errors);
            check_boolean_cond("until", cond, scope, errors);
            // **No narrowing here, unlike `while`.** A `repeat` body runs
            // before the condition is ever evaluated, and the condition
            // holding is what *ends* the loop rather than what enters it —
            // so `repeat x = f() until x != nil` would otherwise narrow `x`
            // to non-nil inside the very body that is trying to fill it, and
            // refuse the assignment that makes the loop terminate.
            let mut body_scope = scope.clone();
            for s in body {
                check_stmt(s, &mut body_scope, errors);
            }
        }

        Stmt::ForNumeric {
            var,
            var_ty,
            from,
            to,
            step,
            body,
        } => {
            if let Some(t) = var_ty {
                check_binding_type(t, stmt.span.clone(), errors);
            }
            check_expr(from, scope, errors);
            check_expr(to, scope, errors);
            if let Some(s) = step {
                check_expr(s, scope, errors);
            }
            // Record the bounds' types.
            //
            // `check_expr` walks them for diagnostics but never asks what
            // they *are*, so without this they are absent from the type
            // table — and a bytecode compiler that cannot tell an integer
            // loop from a float one has to refuse the loop entirely
            // (`VM_DESIGN.md` §11.1: `FORPREP_I` and `FORPREP_F` are
            // separate opcodes precisely so the check happens once, here,
            // rather than per iteration).
            //
            // Also improves hover and inlay hints on loop bounds.
            let _ = crate::expr::infer(from, scope);
            let _ = crate::expr::infer(to, scope);
            if let Some(s) = step {
                let _ = crate::expr::infer(s, scope);
            }
            let mut body_scope = scope.clone();
            let ty = var_ty.clone().unwrap_or(Type::Named("integer".into()));
            body_scope.bind(var.clone(), ty);
            for s in body {
                check_stmt(s, &mut body_scope, errors);
            }
        }

        Stmt::ForIn { vars, iter, body } => {
            for (_, ty_opt) in vars {
                if let Some(t) = ty_opt {
                    check_binding_type(t, stmt.span.clone(), errors);
                }
            }
            check_expr(iter, scope, errors);
            // A nullable iterable is a real bug — the loop blows up the
            // moment the value is nil. Report it, then carry on with the
            // unwrapped type so the body still gets typed bindings and
            // its own mistakes surface in the same pass.
            let iter_ty = match infer(iter, scope) {
                Some(Type::Nullable(inner)) => {
                    errors.push(TypeCheckError::NullableIteration {
                        ty: crate::expr::type_to_string(&Type::Nullable(inner.clone())),
                        span: to_source_span(iter.span.clone()),
                    });
                    Some(*inner)
                }
                other => other,
            };
            crate::receivers::check_iterable(iter, scope, errors);
            // If the iter expression is a known class instance, it must
            // implement `Iterable` or `Iterable2` (walking the parent chain).
            if let Some((class_name, _)) = iter_ty.as_ref().and_then(crate::expr::named_head)
                && with_classes(|reg| reg.contains_key(&class_name))
                && !class_implements_iterable(&class_name)
            {
                errors.push(TypeCheckError::NotIterable {
                    class: class_name,
                    span: to_source_span(iter.span.clone()),
                });
            }
            // When the iter is a `table<V>` / `table<K, V>` we know exactly
            // what each binding receives. Reject mismatched annotations so
            // e.g. `for k: string, v: string in table<Entry>` flags both
            // bindings rather than letting them silently lie.
            if let Some(Type::Table { key, value }) = iter_ty.clone() {
                let yielded: Vec<Type> = match vars.len() {
                    1 => vec![(*value).clone()],
                    2 => {
                        let k_ty = key
                            .as_deref()
                            .cloned()
                            .unwrap_or_else(|| Type::Named("integer".into()));
                        vec![k_ty, (*value).clone()]
                    }
                    _ => Vec::new(),
                };
                // An empty `{}` has no element type to contradict the
                // annotation, and the body never runs — nothing can be
                // bound, so nothing can be bound wrongly. Without this the
                // literal's placeholder `any` element would be read as a
                // downcast and `for v: integer in {} do` would be rejected.
                let empty_literal =
                    matches!(&iter.value, saule_ast::Expr::Table(items) if items.is_empty());

                for ((name, ty_opt), actual) in vars.iter().zip(yielded.iter()) {
                    if let Some(declared) = ty_opt
                        && !empty_literal
                        && !crate::expr::types_compatible(declared, actual)
                    {
                        errors.push(TypeCheckError::ForBindingTypeMismatch {
                            name: name.clone(),
                            declared: crate::expr::type_to_string(declared),
                            actual: crate::expr::type_to_string(actual),
                            span: to_source_span(iter.span.clone()),
                        });
                    }
                }
            }
            let mut body_scope = scope.clone();
            // Bind each loop var: prefer the user's annotation; otherwise
            // fall back to the element/key type inferred from `iter` so
            // unannotated `for i, task in table<Entry>` still gets
            // `task: Entry`. Without this, downstream method calls and
            // exhaustiveness checks (e.g. `match task.isDone()` over a
            // `boolean`) lose their receiver type and bail.
            let yielded_from_iter: Vec<Type> = match iter_ty {
                Some(Type::Table { key, value }) => match vars.len() {
                    1 => vec![(*value).clone()],
                    2 => {
                        let k_ty = key
                            .as_deref()
                            .cloned()
                            .unwrap_or_else(|| Type::Named("integer".into()));
                        vec![k_ty, (*value).clone()]
                    }
                    _ => Vec::new(),
                },
                Some(ty) => {
                    let yielded = iterable_element_types(&ty, vars.len());
                    // The same lie a table loop rejects: a binding annotated
                    // with a type the iterable does not produce.
                    for ((name, ty_opt), actual) in vars.iter().zip(yielded.iter()) {
                        if let Some(declared) = ty_opt
                            && !crate::expr::types_compatible(declared, actual)
                        {
                            errors.push(TypeCheckError::ForBindingTypeMismatch {
                                name: name.clone(),
                                declared: crate::expr::type_to_string(declared),
                                actual: crate::expr::type_to_string(actual),
                                span: to_source_span(iter.span.clone()),
                            });
                        }
                    }
                    yielded
                }
                None => Vec::new(),
            };
            for (i, (name, ty_opt)) in vars.iter().enumerate() {
                if let Some(ty) = ty_opt {
                    body_scope.bind(name.clone(), ty.clone());
                } else if let Some(inferred) = yielded_from_iter.get(i) {
                    body_scope.bind(name.clone(), inferred.clone());
                }
            }
            for s in body {
                check_stmt(s, &mut body_scope, errors);
            }
        }

        Stmt::Return(values) => {
            // A single value is checked against the declared return type, as
            // an annotated `local` is against its annotation: that is what
            // lets `return Coroutine.create(fn(co) … end)` in a function
            // declared `-> Coroutine<integer>` type `co` — the factory
            // shape every generator takes.
            let single_ret = current_return_ty().filter(|t| !matches!(t, Type::Tuple(_)));
            match (values.as_slice(), single_ret.as_ref()) {
                ([v], Some(ret)) => check_expr_expecting(v, Some(ret), scope, errors),
                _ => {
                    for v in values {
                        check_expr(v, scope, errors);
                    }
                }
            }
            // Checked here, against the scope as it stands at this exact
            // statement, rather than in a second pass over the body — see
            // `RETURN_TY`. `scope` already carries every binding in lexical
            // scope, including ones made inside the `if` or `while` this
            // `return` may be nested in.
            if let Some(return_ty) = current_return_ty() {
                check_return_values(values, &return_ty, scope, errors);
            }
        }

        Stmt::Throw(e) => check_expr(e, scope, errors),

        Stmt::Try {
            body,
            catches,
            finally,
        } => {
            let mut body_scope = scope.clone();
            for s in body {
                check_stmt(s, &mut body_scope, errors);
            }
            for (i, c) in catches.iter().enumerate() {
                check_binding_type(&c.ty, c.span.clone(), errors);
                // A clause the ones before it have already swallowed can
                // never run. Reported because the shadowing is invisible
                // otherwise: both clauses look live, and the second one's
                // body is dead code that still type-checks.
                if let Some(prev) = catches[..i].iter().find(|p| catch_shadows(&p.ty, &c.ty)) {
                    errors.push(TypeCheckError::UnreachableCatch {
                        ty: type_to_string(&c.ty),
                        earlier: type_to_string(&prev.ty),
                        span: to_source_span(c.span.clone()),
                    });
                }
                let mut catch_scope = scope.clone();
                catch_scope.bind(c.var.clone(), c.ty.clone());
                for s in &c.body {
                    check_stmt(s, &mut catch_scope, errors);
                }
            }
            if let Some(f) = finally {
                let mut f_scope = scope.clone();
                for s in f {
                    check_stmt(s, &mut f_scope, errors);
                }
            }
        }

        Stmt::Break | Stmt::Continue => {}
    }
}

/// Does a `catch` for `earlier` already handle everything `later` would?
///
/// Only the cases that are certain: `any` catches everything, and the same
/// type twice is the same clause twice. A subclass relationship counts too —
/// a clause for a parent class handles its children.
fn catch_shadows(earlier: &Type, later: &Type) -> bool {
    if matches!(earlier, Type::Named(n) if n == "any") {
        return true;
    }
    match (earlier, later) {
        (Type::Named(a), Type::Named(b)) => a == b || super::state::is_subtype_named(b, a),
        _ => earlier == later,
    }
}

/// Re-set the flow type of a bare-name target after a write to it.
///
/// Narrowing describes the value that was in the slot; writing a new one
/// invalidates it. What replaces it is settled by whether the written value
/// may be nil — see [`Scope::reassign`].
fn reassigned(target: &Spanned<Expr>, value: &Spanned<Expr>, scope: &mut Scope) {
    let Expr::Ident(n) = &target.value else {
        return;
    };
    // An un-inferable value is treated as possibly nil: it falls back to
    // the declared type, which is always sound.
    let may_be_nil = !matches!(
        infer(value, scope),
        Some(t) if !matches!(&t, Type::Nullable(_))
            && !matches!(&t, Type::Named(n) if n == "nil")
    );
    scope.reassign(n, may_be_nil);
}

/// The rules that govern writing `value` into `target`, independent of how
/// the written value was spelled.
///
/// Shared by `a = v` and `a op= v`; for the latter, `value` is the synthetic
/// `a op v` binary, since that — not the bare RHS — is what actually lands in
/// the target. Both operands have already been walked by the caller, so this
/// only reasons about types.
fn check_write_to_target(
    target: &Spanned<Expr>,
    value: &Spanned<Expr>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    // A module variable is assignable from anywhere in the file, so
    // its declared type has to constrain the write the same way a
    // local's does — hence the same scope-then-module lookup order
    // `infer` uses for reads.
    if let Expr::Ident(n) = &target.value
        && let Some(ty) = scope
            .declared(n)
            .cloned()
            .or_else(|| crate::vars::lookup(n))
    {
        // `declared`, not `lookup`: a write is bound by what the name was
        // declared as, not by what flow narrowing proved about the value
        // being overwritten. `while x != nil do x = next() end` is the case
        // — the narrowed `x` is non-nil, and checking the write against that
        // refused the assignment the loop exists to make.
        check_assignment_compat(&ty, value, scope, errors);
    }
    // `t[k] = v` — enforce the table's static key/value types.
    if let Expr::Index { obj, index } = &target.value
        && let Some(Type::Table {
            key,
            value: elem_ty,
        }) = infer(obj, scope)
    {
        let key_ty = key
            .as_deref()
            .cloned()
            .unwrap_or_else(|| Type::Named("integer".into()));
        check_table_key_compat(&key_ty, index, scope, errors);
        check_element_compat(&elem_ty, value, scope, errors);
    }
    // `obj.field = v` — only class instances and class statics support
    // dotted-field assignment. Catches `tbl.foo = ...` on plain
    // tables, where `tbl["foo"] = ...` is the intended form, before
    // it blows up at runtime.
    if let Expr::Member { obj, name } = &target.value {
        check_member_assign_receiver(obj, name, target.span.clone(), scope, errors);
        // …and enforce the field's *declared type*. Only the receiver
        // was validated before, so `self.label = 42` on a
        // `label: string` field went through unchecked.
        if let Some(class_name) = member_assign_class(obj, scope)
            && let Some(field_ty) = saule_semantic::lookup_field_type(&class_name, name)
        {
            check_assignment_compat(&field_ty, value, scope, errors);
        }
    }
}

/// What each binding of `for … in x` receives, when `x` is an instance of a
/// class with an `iter()` method — an `Iterable`, user-declared or builtin.
///
/// Read off `iter`'s own return type, `fn() -> T?`: the loop stops at `nil`,
/// so each binding gets `T` with the `?` taken off. A two-binding loop wants
/// `iter` to return two values. The receiver's type arguments are
/// substituted first, so `for v in gen` over a `Coroutine<integer>` binds
/// `v: integer`, and over a user `Bag<Player>` binds a `Player`.
///
/// Empty when that cannot be worked out, which leaves the bindings untyped
/// exactly as they always were: a generic class named without arguments has
/// no element type to offer, only its own parameter's name.
fn iterable_element_types(ty: &Type, bindings: usize) -> Vec<Type> {
    let Some((name, args)) = crate::expr::named_head(ty) else {
        return Vec::new();
    };
    let (name, args) = crate::state::through_bound(name, args);
    let Some(sig) = saule_semantic::lookup_method(&name, "iter") else {
        return Vec::new();
    };
    let sig = crate::expr::substitute_receiver_args(&sig, &name, &args);
    let Some(Type::Function { ret, .. }) = sig.return_ty else {
        return Vec::new();
    };
    let class_params = with_classes(|r| r.get(&name).map(|c| c.type_params.clone()))
        .unwrap_or_default();
    if crate::expr::mentions_unbound_param(&ret, &class_params) {
        return Vec::new();
    }
    let present = |t: &Type| match t {
        Type::Nullable(inner) => (**inner).clone(),
        other => other.clone(),
    };
    match (*ret, bindings) {
        (Type::Tuple(items), n) if items.len() == n => items.iter().map(present).collect(),
        (Type::Tuple(_), _) => Vec::new(),
        (single, 1) => vec![present(&single)],
        _ => Vec::new(),
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Declaration walker.
// ──────────────────────────────────────────────────────────────────────────────
