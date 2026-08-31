//! Declaration checking: functions, classes, and the rules that
//! relate a subclass to the class it extends.

use saule_ast::{ClassMember, Decl, Expr, Param, Spanned, Type};

use crate::TypeCheckError;
use crate::expr::{
    check_assignment_compat, check_assignment_compat_coercing, infer, is_any, type_to_string,
    types_compatible,
};
use crate::state::{
    Scope, is_interface, pop_generics, push_generics, set_current_class, set_return_ty,
    with_classes,
};
use crate::to_source_span;

use super::*;

/// Check the type arguments a class header supplies to its parent and its
/// interfaces — `class C extends Box<integer> implements Repository<Player>`.
///
/// These are the one set of generic applications a class writes that don't
/// pass through [`check_binding_type`], because they are not binding types;
/// they arrive as [`saule_ast::TypeRef`]s in the declaration header. Reusing
/// the same arity rule keeps one answer for what a wrong argument count
/// means, wherever it is written.
fn check_type_arg_arity(
    extends: Option<&saule_ast::TypeRef>,
    implements: &[saule_ast::TypeRef],
    span: std::ops::Range<usize>,
    errors: &mut Vec<TypeCheckError>,
) {
    for r in extends.into_iter().chain(implements.iter()) {
        check_generic_arity(&r.to_type(), span.clone(), errors);
    }
}

pub(crate) fn check_decl(decl: &Decl, errors: &mut Vec<TypeCheckError>) {
    match decl {
        Decl::Class {
            name: class_name,
            type_params,
            extends,
            implements,
            members,
            ..
        } => {
            // Validate the class hierarchy: every name in `extends` /
            // `implements` must refer to a real class / interface that
            // semantic has already collected into the registry.
            if let Some(parent) = extends
                && !with_classes(|r| r.contains_key(&parent.name))
            {
                // Point the diagnostic at the first member span — class
                // span isn't readily available here; tweaking the AST to
                // carry it is left for a follow-up.
                let span = members.first().map(|m| m.span.clone()).unwrap_or(0..0);
                errors.push(TypeCheckError::UnknownParentClass {
                    name: class_name.clone(),
                    parent: parent.name.clone(),
                    span: to_source_span(span),
                });
            }
            for iface in implements {
                if !is_interface(&iface.name) {
                    let span = members.first().map(|m| m.span.clone()).unwrap_or(0..0);
                    errors.push(TypeCheckError::UnknownInterface {
                        name: class_name.clone(),
                        iface: iface.name.clone(),
                        span: to_source_span(span),
                    });
                }
            }
            // The class's own `<T, U>` are in scope for every member: a
            // field typed `T`, a method returning `T`, a body mentioning it.
            // Without this each one reads as an undeclared class name, and
            // `class Box<T> local v: T` fails on its own field.
            let prev_generics = push_generics(type_params);
            let span_for = || members.first().map(|m| m.span.clone()).unwrap_or(0..0);
            check_type_arg_arity(extends.as_ref(), implements, span_for(), errors);
            if let Some(parent) = extends {
                check_overrides(class_name, &parent.name, members, errors);
            }
            check_class(class_name, members, errors);
            pop_generics(prev_generics);
        }
        Decl::Function {
            type_params,
            params,
            return_ty,
            body,
            ..
        } => {
            check_param_types(params, errors);
            // `Decl::Function` carries no span of its own, so a bad return
            // type is pointed at the nearest node that does.
            if let Some(rt) = return_ty {
                let span = params
                    .first()
                    .map(|p| p.span.clone())
                    .or_else(|| body.first().map(|s| s.span.clone()))
                    .unwrap_or(0..0);
                reject_non_types(rt, span, errors);
            }
            let prev_generics = push_generics(type_params);
            let mut scope = Scope::default();
            check_default_params(params, &scope, errors);
            seed_params(&mut scope, params);
            let prev_ret = set_return_ty(return_ty.clone());
            for s in body {
                check_stmt(s, &mut scope, errors);
            }
            set_return_ty(prev_ret);
            pop_generics(prev_generics);
        }
        // `export name: T = value` — the module-scope sibling of a `local`,
        // and checked by exactly the same two rules: the initializer must
        // fit the annotation, and a missing initializer means `nil`, which
        // only a nullable `T` accepts.
        Decl::Variable {
            name,
            name_span,
            ty,
            value,
            ..
        } => {
            // The initializer is checked in an empty scope: it runs at
            // module level, where no local is in scope yet.
            let scope = Scope::default();
            if let Some(t) = ty {
                check_binding_type(t, name_span.clone(), errors);
            }
            match (ty, value) {
                (Some(t), Some(v)) => {
                    check_expr_expecting(v, Some(t), &scope, errors);
                    // Annotated module variable — a coercing site, like the
                    // annotated `local` it mirrors.
                    check_assignment_compat_coercing(t, v, &scope, errors);
                }
                (None, Some(v)) => check_expr(v, &scope, errors),
                (Some(t), None) => {
                    if !is_nullable(t) {
                        errors.push(TypeCheckError::UninitializedVariable {
                            name: name.clone(),
                            ty: type_to_string(t),
                            span: to_source_span(name_span.clone()),
                        });
                    }
                }
                (None, None) => {}
            }
        }
        Decl::Interface {
            type_params,
            methods,
            ..
        } => {
            // `<T>` is in scope for every signature: `fn get() -> T` on
            // `interface Box<T>` must read `T` as the interface's parameter,
            // not as an undeclared class. A method's own `<U>` nests inside.
            let prev_generics = push_generics(type_params);
            for sig in methods {
                let prev_method = push_generics(&sig.type_params);
                check_param_types(&sig.params, errors);
                if let Some(rt) = &sig.return_ty {
                    reject_non_types(rt, sig.span.clone(), errors);
                }
                pop_generics(prev_method);
            }
            pop_generics(prev_generics);
        }
        Decl::Enum {
            type_params,
            variants,
            ..
        } => {
            // `<T>` is in scope for the payload types: `Ok(value: T)` must
            // read `T` as this enum's parameter, not as an undeclared class.
            let prev_generics = push_generics(type_params);
            for v in variants {
                if let saule_ast::EnumVariant::Tuple { fields, .. } = &v.value {
                    check_param_types(fields, errors);
                }
            }
            pop_generics(prev_generics);
        }
        _ => {}
    }
}

/// Every method that shadows one from an ancestor must remain usable where
/// the ancestor's version was. Without this a subclass could redeclare
/// `get() -> integer` as `get() -> string`, and a caller holding the parent
/// type would silently receive the wrong thing.
///
/// Parameters are compared invariantly (same count, compatible types) and
/// the return type covariantly (the child may narrow, never widen), which is
/// the rule that keeps parent-typed call sites correct. `init` is exempt:
/// constructors are not dispatched through a parent reference, and
/// `self.super(...)` already checks its own arguments.
pub(crate) fn check_overrides(
    class_name: &str,
    parent: &str,
    members: &[Spanned<ClassMember>],
    errors: &mut Vec<TypeCheckError>,
) {
    for member in members {
        let ClassMember::Method(m) = &member.value else {
            continue;
        };
        if m.name == "init" {
            continue;
        }
        let Some(base) = saule_semantic::lookup_method(parent, &m.name) else {
            continue;
        };

        let detail = override_mismatch(m, &base);
        if let Some(detail) = detail {
            errors.push(TypeCheckError::IncompatibleOverride {
                class: class_name.to_string(),
                parent: parent.to_string(),
                method: m.name.clone(),
                detail,
                span: to_source_span(m.span.clone()),
            });
        }
    }
}

/// Describes how `m` fails to override `base`, or `None` when it is a valid
/// override.
pub(crate) fn override_mismatch(
    m: &saule_ast::Method,
    base: &saule_semantic::MethodSig,
) -> Option<String> {
    if m.is_static != base.is_static {
        return Some(if m.is_static {
            "the parent's is an instance method, this one is `static`".to_string()
        } else {
            "the parent's is a `static` method, this one is an instance method".to_string()
        });
    }

    // `self` is implicit and typed as the enclosing class, so it is never
    // comparable across the two and is left out of the arity check.
    let own: Vec<&Param> = m.params.iter().filter(|p| p.name != "self").collect();
    let base_params: Vec<&Param> = base.params.iter().filter(|p| p.name != "self").collect();

    if own.len() != base_params.len() {
        return Some(format!(
            "the parent takes {} parameter(s), this takes {}",
            base_params.len(),
            own.len()
        ));
    }

    for (i, (p, bp)) in own.iter().zip(base_params.iter()).enumerate() {
        if is_any(&p.ty) || is_any(&bp.ty) {
            continue;
        }
        if !types_compatible(&bp.ty, &p.ty) || !types_compatible(&p.ty, &bp.ty) {
            return Some(format!(
                "parameter {} is `{}` in the parent but `{}` here",
                i + 1,
                type_to_string(&bp.ty),
                type_to_string(&p.ty)
            ));
        }
    }

    // Return type: the child's must satisfy the parent's contract. An
    // omitted return type on either side means "unconstrained", so only
    // compare when both are declared.
    if let (Some(ret), Some(base_ret)) = (&m.return_ty, &base.return_ty)
        && !is_any(ret)
        && !is_any(base_ret)
        && !types_compatible(base_ret, ret)
    {
        return Some(format!(
            "the parent returns `{}`, this returns `{}`",
            type_to_string(base_ret),
            type_to_string(ret)
        ));
    }
    None
}

pub(crate) fn seed_params(scope: &mut Scope, params: &[Param]) {
    for p in params {
        scope.bind(p.name.clone(), p.ty.clone());
    }
}

/// Check each parameter default against the declared parameter type.
pub(crate) fn check_default_params(
    params: &[Param],
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
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

/// Check the values of one `return` against the enclosing signature.
///
/// Called from `check_stmt`'s `Stmt::Return` arm, so `scope` is the live
/// lexical environment at the return itself. That is the whole point of
/// the arrangement: the previous design walked the body a second time with
/// the scope the first walk finished with, which held the parameters and
/// the top-level locals but nothing bound inside a nested block. A value
/// laundered through `local x = ...` inside an `if` was therefore invisible
/// — `infer` answered `None`, and the check quietly passed.
pub(crate) fn check_return_values(
    values: &[Spanned<Expr>],
    return_ty: &Type,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
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

pub(crate) fn check_class(
    class_name: &str,
    members: &[Spanned<ClassMember>],
    errors: &mut Vec<TypeCheckError>,
) {
    // Validate field defaults: `local x: string = nil` is an error regardless
    // of whether a constructor exists or the field is static. (Definite
    // assignment of constructor-set fields lives in `saule-semantic`.)
    for m in members {
        if let ClassMember::Field { ty, default, .. } = &m.value {
            check_binding_type(ty, m.span.clone(), errors);
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
            check_param_types(&meth.params, errors);
            if let Some(rt) = &meth.return_ty {
                reject_non_types(rt, meth.span.clone(), errors);
            }
            let prev_generics = push_generics(&meth.type_params);
            let mut scope = Scope::default();
            // `self` resolves to the class itself in `static fn` and to an
            // instance otherwise. Seed it as the class name so member-existence
            // checks on `self.foo` consult the class registry.
            scope.bind("self".to_string(), Type::Named(class_name.to_string()));
            check_default_params(&meth.params, &scope, errors);
            seed_params(&mut scope, &meth.params);
            let prev_ret = set_return_ty(meth.return_ty.clone());
            for s in &meth.body {
                check_stmt(s, &mut scope, errors);
            }
            set_return_ty(prev_ret);
            pop_generics(prev_generics);
        }
    }
    set_current_class(prev);
}
