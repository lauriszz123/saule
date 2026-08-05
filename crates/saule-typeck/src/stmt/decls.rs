//! Declaration checking: functions, classes, and the rules that
//! relate a subclass to the class it extends.

use saule_ast::{ClassMember, Decl, Expr, Param, Spanned, Type};

use crate::TypeCheckError;
use crate::expr::{check_assignment_compat, infer, is_any, type_to_string, types_compatible};
use crate::state::{
    Scope, is_interface, pop_generics, push_generics, set_current_class, set_return_ty,
    with_classes,
};
use crate::to_source_span;

use super::*;

pub(crate) fn check_decl(decl: &Decl, errors: &mut Vec<TypeCheckError>) {
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
            if let Some(parent) = extends {
                check_overrides(class_name, parent, members, errors);
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
                reject_nil_in_binding_type(t, name_span.clone(), errors);
            }
            match (ty, value) {
                (Some(t), Some(v)) => {
                    check_expr_expecting(v, Some(t), &scope, errors);
                    check_assignment_compat(t, v, &scope, errors);
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
