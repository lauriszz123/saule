//! Statement & declaration walker. Threads a [`Scope`](super::state::Scope)
//! so we know the static type of every `local` seen on the current path, and
//! so narrowing in `if`/`else` can override types for the duration of a
//! sub-block.

use saule_ast::{ClassMember, Decl, Expr, Param, Spanned, Stmt, Type};

use super::TypeCheckError;
use super::expr::{
    check_assignment_compat, check_boolean_cond, check_element_compat, check_expr,
    check_table_key_compat, infer, is_any, is_nullable, narrow_falsy, narrow_truthy,
    strip_nullable, type_to_string, types_compatible,
};
use super::state::{
    Scope, class_implements_iterable, is_interface, pop_generics, push_generics,
    set_current_class, with_classes,
};
use super::to_source_span;

/// Reject `nil` used as a binding type. `nil` is a value (the inhabitant
/// of the unit type), and nullability is expressed with `T?` — so any
/// occurrence of `nil` inside a binding/parameter/field type is a
/// foot-gun the typechecker should call out. Return types are *not*
/// validated here: `fn foo() -> nil` is the conventional "returns
/// nothing" signature.
pub(super) fn reject_nil_in_binding_type(
    ty: &Type,
    span: std::ops::Range<usize>,
    errors: &mut Vec<TypeCheckError>,
) {
    fn walk(ty: &Type) -> bool {
        match ty {
            Type::Named(n) => n == "nil",
            Type::Nullable(inner) => walk(inner),
            Type::Table { key, value } => key.as_deref().map(walk).unwrap_or(false) || walk(value),
            Type::Tuple(items) => items.iter().any(walk),
            Type::Function { params, ret } => params.iter().any(walk) || walk(ret),
        }
    }
    if walk(ty) {
        errors.push(TypeCheckError::NilTypeAnnotation {
            span: to_source_span(span),
        });
    }
}

/// Run [`reject_nil_in_binding_type`] over every parameter's declared type.
fn reject_nil_in_params(params: &[Param], errors: &mut Vec<TypeCheckError>) {
    for p in params {
        reject_nil_in_binding_type(&p.ty, p.span.clone(), errors);
    }
}

/// Type-vs-type assignment compatibility check, used when the value side
/// is a tuple component (e.g. `local a, b = f()` where `f()` returns
/// `(A, B)`) and we don't have a per-element expression to feed through
/// [`check_assignment_compat`].
fn check_type_assignment_compat(
    decl_ty: &Type,
    found_ty: &Type,
    span: std::ops::Range<usize>,
    errors: &mut Vec<TypeCheckError>,
) {
    let is_nil_val = matches!(found_ty, Type::Named(n) if n == "nil");
    if is_nil_val {
        if !is_nullable(decl_ty) {
            errors.push(TypeCheckError::NilToNonNullable {
                ty: type_to_string(decl_ty),
                span: to_source_span(span),
            });
        }
        return;
    }
    if is_any(found_ty) || is_any(decl_ty) {
        return;
    }
    if is_nullable(found_ty) && !is_nullable(decl_ty) {
        errors.push(TypeCheckError::NullableToNonNullable {
            from: type_to_string(found_ty),
            to: type_to_string(decl_ty),
            span: to_source_span(span),
        });
        return;
    }
    if !types_compatible(decl_ty, found_ty) {
        errors.push(TypeCheckError::AssignmentTypeMismatch {
            expected: type_to_string(decl_ty),
            found: type_to_string(found_ty),
            span: to_source_span(span),
        });
    }
}

pub(super) fn check_stmt(
    stmt: &Spanned<Stmt>,
    scope: &mut Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    match &stmt.value {
        Stmt::Decl(decl) => check_decl(&decl.value, errors),

        Stmt::Local { name, ty, value } => {
            if let Some(t) = ty {
                reject_nil_in_binding_type(t, stmt.span.clone(), errors);
            }
            if let (Some(ty), Some(v)) = (ty, value) {
                check_expr(v, scope, errors);
                check_assignment_compat(ty, v, scope, errors);
                // Refine a bare structural annotation (`table`, `function`)
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
            }
        }

        Stmt::LocalMulti { names, values } => {
            for (_, ty_opt) in names {
                if let Some(t) = ty_opt {
                    reject_nil_in_binding_type(t, stmt.span.clone(), errors);
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
                for (i, (name, ty_opt)) in names.iter().enumerate() {
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

            for (i, (name, ty_opt)) in names.iter().enumerate() {
                if let (Some(ty), Some(v)) = (ty_opt, values.get(i)) {
                    check_assignment_compat(ty, v, scope, errors);
                }
                if let Some(ty) = ty_opt {
                    let bound = match values.get(i) {
                        Some(v) => refine_bare_binding(ty, v, scope),
                        None => ty.clone(),
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
            if let Expr::Ident(n) = &target.value
                && let Some(ty) = scope.lookup(n).cloned()
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
            }
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
                        && let (Some(ty), Some(found_ty)) =
                            (scope.lookup(n).cloned(), ts.get(i))
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
                reject_nil_in_binding_type(t, stmt.span.clone(), errors);
            }
            check_expr(from, scope, errors);
            check_expr(to, scope, errors);
            if let Some(s) = step {
                check_expr(s, scope, errors);
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
                    reject_nil_in_binding_type(t, stmt.span.clone(), errors);
                }
            }
            check_expr(iter, scope, errors);
            // If the iter expression is a known class instance, it must
            // implement `Iterable` or `Iterable2` (walking the parent chain).
            if let Some(Type::Named(class_name)) = infer(iter, scope)
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
            if let Some(Type::Table { key, value }) = infer(iter, scope) {
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
            }
            let mut body_scope = scope.clone();
            // Bind each loop var: prefer the user's annotation; otherwise
            // fall back to the element/key type inferred from `iter` so
            // unannotated `for i, task in table<Entry>` still gets
            // `task: Entry`. Without this, downstream method calls and
            // exhaustiveness checks (e.g. `match task.isDone()` over a
            // `boolean`) lose their receiver type and bail.
            let yielded_from_iter: Vec<Type> =
                if let Some(Type::Table { key, value }) = infer(iter, scope) {
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
        }

        Stmt::Throw(e) => check_expr(e, scope, errors),

        Stmt::Try {
            body,
            catch_var,
            catch_ty,
            catch_body,
        } => {
            reject_nil_in_binding_type(catch_ty, stmt.span.clone(), errors);
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

// ──────────────────────────────────────────────────────────────────────────────
// Declaration walker.
// ──────────────────────────────────────────────────────────────────────────────

fn check_decl(decl: &Decl, errors: &mut Vec<TypeCheckError>) {
    match decl {
        Decl::Class {
            name: class_name,
            extends,
            implements,
            members,
            ..
        } => {
            // Validate the class hierarchy: every name in `extends` /
            // `implements` must refer to a real class / interface that
            // semantic has already collected into the registry.
            if let Some(parent) = extends
                && !with_classes(|r| r.contains_key(parent))
            {
                // Point the diagnostic at the first member span — class
                // span isn't readily available here; tweaking the AST to
                // carry it is left for a follow-up.
                let span = members.first().map(|m| m.span.clone()).unwrap_or(0..0);
                errors.push(TypeCheckError::UnknownParentClass {
                    name: class_name.clone(),
                    parent: parent.clone(),
                    span: to_source_span(span),
                });
            }
            for iface in implements {
                if !is_interface(iface) {
                    let span = members.first().map(|m| m.span.clone()).unwrap_or(0..0);
                    errors.push(TypeCheckError::UnknownInterface {
                        name: class_name.clone(),
                        iface: iface.clone(),
                        span: to_source_span(span),
                    });
                }
            }
            check_class(class_name, members, errors)
        }
        Decl::Function {
            type_params,
            params,
            return_ty,
            body,
            ..
        } => {
            reject_nil_in_params(params, errors);
            let prev_generics = push_generics(type_params);
            let mut scope = Scope::default();
            check_default_params(params, &scope, errors);
            seed_params(&mut scope, params);
            for s in body {
                check_stmt(s, &mut scope, errors);
            }
            if let Some(rt) = return_ty {
                check_returns(body, rt, &scope, errors);
            }
            pop_generics(prev_generics);
        }
        Decl::Interface { methods, .. } => {
            for sig in methods {
                reject_nil_in_params(&sig.params, errors);
            }
        }
        Decl::Enum { variants, .. } => {
            for v in variants {
                if let saule_ast::EnumVariant::Tuple { fields, .. } = &v.value {
                    reject_nil_in_params(fields, errors);
                }
            }
        }
        _ => {}
    }
}

pub(super) fn seed_params(scope: &mut Scope, params: &[Param]) {
    for p in params {
        scope.bind(p.name.clone(), p.ty.clone());
    }
}

/// Check each parameter default against the declared parameter type.
fn check_default_params(params: &[Param], scope: &Scope, errors: &mut Vec<TypeCheckError>) {
    for p in params {
        if let Some(d) = &p.default
            && !is_assignment_compatible(&p.ty, d, scope)
        {
            errors.push(TypeCheckError::DefaultParamTypeMismatch {
                param: p.name.clone(),
                ty: type_to_string(&p.ty),
                span: to_source_span(d.span.clone()),
            });
        }
    }
}

/// Walk a function/method body looking for `return v` statements whose first
/// value can be proved incompatible with `return_ty`.
fn check_returns(
    body: &[Spanned<Stmt>],
    return_ty: &Type,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    for s in body {
        walk_returns(&s.value, return_ty, scope, errors);
    }
}

fn walk_returns(stmt: &Stmt, return_ty: &Type, scope: &Scope, errors: &mut Vec<TypeCheckError>) {
    match stmt {
        Stmt::Return(values) => {
            // Multi-return: `-> (A, B, C)` paired against `return a, b, c`.
            // When there's exactly one return value but the function returns
            // a tuple, the value may be a call that yields the tuple — leave
            // that case to the per-value path (it'll see Tuple vs Tuple and
            // accept).
            if let Type::Tuple(elems) = return_ty
                && values.len() == elems.len()
            {
                for (elem_ty, v) in elems.iter().zip(values.iter()) {
                    if !is_assignment_compatible(elem_ty, v, scope) {
                        let found = infer(v, scope)
                            .map(|t| type_to_string(&t))
                            .unwrap_or_else(|| "<unknown>".to_string());
                        errors.push(TypeCheckError::WrongReturnType {
                            ty: type_to_string(elem_ty),
                            found,
                            span: to_source_span(v.span.clone()),
                        });
                    }
                }
                return;
            }
            if let Some(v) = values.first()
                && !is_assignment_compatible(return_ty, v, scope)
            {
                let found = infer(v, scope)
                    .map(|t| type_to_string(&t))
                    .unwrap_or_else(|| "<unknown>".to_string());
                errors.push(TypeCheckError::WrongReturnType {
                    ty: type_to_string(return_ty),
                    found,
                    span: to_source_span(v.span.clone()),
                });
            }
        }
        Stmt::If {
            then_block,
            elseifs,
            else_block,
            ..
        } => {
            for s in then_block {
                walk_returns(&s.value, return_ty, scope, errors);
            }
            for (_, b) in elseifs {
                for s in b {
                    walk_returns(&s.value, return_ty, scope, errors);
                }
            }
            if let Some(b) = else_block {
                for s in b {
                    walk_returns(&s.value, return_ty, scope, errors);
                }
            }
        }
        Stmt::While { body, .. }
        | Stmt::Repeat { body, .. }
        | Stmt::ForNumeric { body, .. }
        | Stmt::ForIn { body, .. } => {
            for s in body {
                walk_returns(&s.value, return_ty, scope, errors);
            }
        }
        Stmt::Try {
            body, catch_body, ..
        } => {
            for s in body {
                walk_returns(&s.value, return_ty, scope, errors);
            }
            for s in catch_body {
                walk_returns(&s.value, return_ty, scope, errors);
            }
        }
        _ => {}
    }
}

/// Refine a bare structural annotation against the value's inferred shape.
///
/// `local x: table = expr` and `local x: function = expr` only carry the
/// kind tag — no element type, no parameter / return types. If `expr`
/// infers to a concrete `Type::Table { .. }` / `Type::Function { .. }`
/// of the matching kind, use that richer type for the binding so later
/// reads and writes get the full generic check. Otherwise fall back to
/// the declared bare type.
///
/// Nullable wrappers are unwrapped on the declaration side and re-wrapped
/// around the refined type — `local x: table? = maybe()` widens to
/// `table<...>?` when `maybe()` returns one.
fn refine_bare_binding(decl_ty: &Type, value: &Spanned<Expr>, scope: &Scope) -> Type {
    let (inner_decl, was_nullable) = match decl_ty {
        Type::Nullable(inner) => (inner.as_ref(), true),
        other => (other, false),
    };
    let Type::Named(name) = inner_decl else {
        return decl_ty.clone();
    };
    let Some(value_ty) = infer(value, scope) else {
        return decl_ty.clone();
    };
    // Look through a nullable on the value side too — the declared
    // nullability is what wraps the binding, not the value's.
    let value_inner = match &value_ty {
        Type::Nullable(inner) => inner.as_ref().clone(),
        other => other.clone(),
    };
    let matches_kind = matches!(
        (name.as_str(), &value_inner),
        ("table", Type::Table { .. }) | ("function", Type::Function { .. })
    );
    if !matches_kind {
        return decl_ty.clone();
    }
    if was_nullable {
        Type::Nullable(Box::new(value_inner))
    } else {
        value_inner
    }
}

/// True when we can *prove* the value is incompatible-free with the target
/// type. Returns true when we can't decide (conservative: don't false-positive).
fn is_assignment_compatible(decl_ty: &Type, value: &Spanned<Expr>, scope: &Scope) -> bool {
    // `nil` literal is fine only when the target accepts nil.
    if matches!(value.value, Expr::Nil) {
        return is_nullable(decl_ty);
    }
    let Some(value_ty) = infer(value, scope) else {
        // Unknown value type — stay conservative.
        return true;
    };
    // Nullable value into non-nullable slot is always wrong.
    if is_nullable(&value_ty) && !is_nullable(decl_ty) {
        return false;
    }
    crate::expr::types_compatible(decl_ty, &value_ty)
}

fn check_class(
    class_name: &str,
    members: &[Spanned<ClassMember>],
    errors: &mut Vec<TypeCheckError>,
) {
    // Validate field defaults: `local x: string = nil` is an error regardless
    // of whether a constructor exists or the field is static. (Definite
    // assignment of constructor-set fields lives in `saule-semantic`.)
    for m in members {
        if let ClassMember::Field { ty, default, .. } = &m.value {
            reject_nil_in_binding_type(ty, m.span.clone(), errors);
            if let Some(default_expr) = default {
                let scope = Scope::default();
                check_assignment_compat(ty, default_expr, &scope, errors);
            }
        }
    }

    // Walk every method body with a scope seeded from its parameters,
    // and within `CURRENT_CLASS` set so private-member checks know we're
    // *inside* `class_name`. Also validate default parameters and return types.
    let prev = set_current_class(Some(class_name.to_string()));
    for m in members {
        if let ClassMember::Method(meth) = &m.value {
            reject_nil_in_params(&meth.params, errors);
            let prev_generics = push_generics(&meth.type_params);
            let mut scope = Scope::default();
            // `self` resolves to the class itself in `static fn` and to an
            // instance otherwise. Seed it as the class name so member-existence
            // checks on `self.foo` consult the class registry.
            scope.bind("self".to_string(), Type::Named(class_name.to_string()));
            check_default_params(&meth.params, &scope, errors);
            seed_params(&mut scope, &meth.params);
            for s in &meth.body {
                check_stmt(s, &mut scope, errors);
            }
            if let Some(rt) = &meth.return_ty {
                check_returns(&meth.body, rt, &scope, errors);
            }
            pop_generics(prev_generics);
        }
    }
    set_current_class(prev);
}

/// Verify that `obj.name = ...` is being written to a receiver that
/// actually supports field assignment. Class instances and class statics
/// do; plain tables, primitives, and functions don't.
fn check_member_assign_receiver(
    obj: &Spanned<Expr>,
    name: &str,
    span: std::ops::Range<usize>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    // `self.foo = ...` and `super.foo = ...` are always fine inside a method;
    // the resolver / runtime own that path.
    if matches!(obj.value, Expr::Self_) {
        return;
    }
    // `Class.foo = ...` (static-field assignment) — receiver is a known class.
    if let Expr::Ident(n) = &obj.value
        && with_classes(|reg| reg.contains_key(n))
    {
        return;
    }
    // Otherwise infer the receiver's type. We can only complain when we
    // actually know it; an unknown type silently passes so dynamic `any`
    // code still works.
    let Some(ty) = infer(obj, scope) else { return };
    let stripped = strip_nullable(ty.clone());
    // Lua-style tables let `t.foo = v` mean `t["foo"] = v`, so anything
    // shaped like a table is allowed through. Reject only the truly
    // field-less primitives / functions.
    let bad = match &stripped {
        Type::Named(n) => matches!(
            n.as_str(),
            "integer" | "float" | "number" | "boolean" | "string" | "function"
        ),
        Type::Tuple(_) | Type::Function { .. } => true,
        _ => false,
    };
    if bad {
        errors.push(TypeCheckError::InvalidFieldAssign {
            receiver: type_to_string(&stripped),
            member: name.to_string(),
            span: super::to_source_span(span),
        });
    }
}

/// Returns true if every execution path through `block` exits the
/// surrounding scope — i.e. ends in `return`, `throw`, `break`, or
/// `continue`. Used to drive early-exit narrowing for guard idioms
/// like `if x == nil then return end`.
///
/// Conservative: only inspects the *last* statement of the block, plus
/// a shallow recursion into nested `If` arms. Anything else (loops,
/// try/catch, etc.) is treated as non-diverging.
fn block_diverges(block: &[Spanned<Stmt>]) -> bool {
    let Some(last) = block.last() else {
        return false;
    };
    match &last.value {
        Stmt::Return(_) | Stmt::Throw(_) | Stmt::Break | Stmt::Continue => true,
        Stmt::If {
            then_block,
            elseifs,
            else_block,
            ..
        } => {
            else_block.as_ref().is_some_and(|b| block_diverges(b))
                && block_diverges(then_block)
                && elseifs.iter().all(|(_, body)| block_diverges(body))
        }
        _ => false,
    }
}
