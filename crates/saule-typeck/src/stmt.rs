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

pub(super) fn check_stmt(
    stmt: &Spanned<Stmt>,
    scope: &mut Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    match &stmt.value {
        Stmt::Error => {}

        Stmt::Decl(decl) => check_decl(&decl.value, errors),

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

            if let Some((ts, vspan)) = tuple_spread {
                for (i, (name, _, ty_opt)) in names.iter().enumerate() {
                    let found = ts.get(i).cloned();
                    if let (Some(ty), Some(found_ty)) = (ty_opt, found.as_ref()) {
                        check_type_assignment_compat(ty, found_ty, vspan.clone(), errors);
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
                            // A lone call as the RHS is exempt: the tuple
                            // spread above only fires when we could infer the
                            // callee's return tuple, and a multi-return whose
                            // type we couldn't resolve still fills these names
                            // at runtime. Only a call can do that — a literal
                            // or a variable never spreads.
                            let maybe_spread = values.len() == 1
                                && names.len() > 1
                                && matches!(values[0].value, Expr::Call { .. });
                            if !is_nullable(ty) && !maybe_spread {
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
            check_expr(value, scope, errors);
            check_write_to_target(target, value, scope, errors);
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
        }

        Stmt::AssignMulti { targets, values } => {
            for t in targets {
                check_expr(t, scope, errors);
            }
            for v in values {
                check_expr(v, scope, errors);
            }

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
                return;
            }

            for (i, target) in targets.iter().enumerate() {
                if let Expr::Ident(n) = &target.value
                    && let (Some(ty), Some(v)) = (scope.lookup(n).cloned(), values.get(i))
                {
                    check_assignment_compat(&ty, v, scope, errors);
                }
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

        Stmt::While { cond, body } | Stmt::Repeat { body, cond } => {
            check_expr(cond, scope, errors);
            check_boolean_cond(
                if matches!(stmt.value, Stmt::While { .. }) {
                    "while"
                } else {
                    "until"
                },
                cond,
                scope,
                errors,
            );
            let mut body_scope = scope.clone();
            narrow_truthy(cond, &mut body_scope);
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
            // If the iter expression is a known class instance, it must
            // implement `Iterable` or `Iterable2` (walking the parent chain).
            if let Some(Type::Named(class_name)) = iter_ty.clone()
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
            let yielded_from_iter: Vec<Type> = if let Some(Type::Table { key, value }) = iter_ty {
                match vars.len() {
                    1 => vec![(*value).clone()],
                    2 => {
                        let k_ty = key
                            .as_deref()
                            .cloned()
                            .unwrap_or_else(|| Type::Named("integer".into()));
                        vec![k_ty, (*value).clone()]
                    }
                    _ => Vec::new(),
                }
            } else {
                Vec::new()
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
            for v in values {
                check_expr(v, scope, errors);
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
            catch_var,
            catch_ty,
            catch_body,
        } => {
            check_binding_type(catch_ty, stmt.span.clone(), errors);
            let mut body_scope = scope.clone();
            for s in body {
                check_stmt(s, &mut body_scope, errors);
            }
            let mut catch_scope = scope.clone();
            catch_scope.bind(catch_var.clone(), catch_ty.clone());
            for s in catch_body {
                check_stmt(s, &mut catch_scope, errors);
            }
        }

        Stmt::Break | Stmt::Continue => {}
    }
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
        && let Some(ty) = scope.lookup(n).cloned().or_else(|| crate::vars::lookup(n))
    {
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

// ──────────────────────────────────────────────────────────────────────────────
// Declaration walker.
// ──────────────────────────────────────────────────────────────────────────────
