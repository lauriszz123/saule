//! Statement & declaration walker. Threads a [`Scope`](super::state::Scope)
//! so we know the static type of every `local` seen on the current path, and
//! so narrowing in `if`/`else` can override types for the duration of a
//! sub-block.

use saule_ast::{ClassMember, Decl, Expr, Param, Spanned, Stmt, Type};

use super::TypeCheckError;
use super::expr::{
    check_assignment_compat, check_boolean_cond, check_element_compat, check_expr,
    check_table_key_compat, infer, is_nullable, narrow_falsy, narrow_truthy, strip_nullable,
    type_to_string,
};
use super::state::{
    Scope, class_implements_iterable, is_interface, is_type_param, pop_generics, push_generics,
    set_current_class, with_classes,
};
use super::to_source_span;

pub(super) fn check_stmt(
    stmt: &Spanned<Stmt>,
    scope: &mut Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    match &stmt.value {
        Stmt::Decl(decl) => check_decl(&decl.value, errors),

        Stmt::Local { name, ty, value } => {
            if let (Some(ty), Some(v)) = (ty, value) {
                check_expr(v, scope, errors);
                check_assignment_compat(ty, v, scope, errors);
                scope.bind(name.clone(), ty.clone());
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
            for v in values {
                check_expr(v, scope, errors);
            }
            for (i, (name, ty_opt)) in names.iter().enumerate() {
                if let (Some(ty), Some(v)) = (ty_opt, values.get(i)) {
                    check_assignment_compat(ty, v, scope, errors);
                }
                if let Some(ty) = ty_opt {
                    scope.bind(name.clone(), ty.clone());
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
                && let Some(Type::Table { key, value: elem_ty }) = infer(obj, scope)
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
            let mut body_scope = scope.clone();
            for (name, ty_opt) in vars {
                if let Some(ty) = ty_opt {
                    body_scope.bind(name.clone(), ty.clone());
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
                let span = members
                    .first()
                    .map(|m| m.span.clone())
                    .unwrap_or(0..0);
                errors.push(TypeCheckError::UnknownParentClass {
                    name: class_name.clone(),
                    parent: parent.clone(),
                    span: to_source_span(span),
                });
            }
            for iface in implements {
                if !is_interface(iface) {
                    let span = members
                        .first()
                        .map(|m| m.span.clone())
                        .unwrap_or(0..0);
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

fn walk_returns(
    stmt: &Stmt,
    return_ty: &Type,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    match stmt {
        Stmt::Return(values) => {
            if let Some(v) = values.first()
                && !is_assignment_compatible(return_ty, v, scope)
            {
                errors.push(TypeCheckError::WrongReturnType {
                    ty: type_to_string(return_ty),
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

/// True when we can *prove* the value is incompatible-free with the target
/// type. Returns true when we can't decide (conservative: don't false-positive).
fn is_assignment_compatible(decl_ty: &Type, value: &Spanned<Expr>, scope: &Scope) -> bool {
    if is_nullable(decl_ty) {
        // Nullable target accepts anything we can express.
        return true;
    }
    if matches!(value.value, Expr::Nil) {
        return false;
    }
    let Some(value_ty) = infer(value, scope) else {
        return true;
    };
    if is_nullable(&value_ty) {
        return false;
    }
    match (decl_ty, &value_ty) {
        (Type::Named(a), Type::Named(b)) => {
            if a == b || a == "any" || b == "any" {
                true
            } else if is_type_param(a) || is_type_param(b) {
                // Generic type parameters in scope match anything.
                true
            } else {
                // Allow numeric literals in either direction only when same name.
                false
            }
        }
        _ => true,
    }
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
        if let ClassMember::Field {
            ty,
            default: Some(default_expr),
            ..
        } = &m.value
        {
            let scope = Scope::default();
            check_assignment_compat(ty, default_expr, &scope, errors);
        }
    }


    // Walk every method body with a scope seeded from its parameters,
    // and within `CURRENT_CLASS` set so private-member checks know we're
    // *inside* `class_name`. Also validate default parameters and return types.
    let prev = set_current_class(Some(class_name.to_string()));
    for m in members {
        if let ClassMember::Method(meth) = &m.value {
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
