//! Binding and assignment checking: the declared-type / value-type
//! agreement rules, `nil` annotation rejection, and the receiver
//! checks for `obj.field = value`.

use saule_ast::{Expr, Param, Spanned, Stmt, Type};

use crate::TypeCheckError;
use crate::expr::{infer, is_any, is_nullable, strip_nullable, type_to_string, types_compatible};
use crate::state::{Scope, with_classes};
use crate::to_source_span;

/// Reject `nil` used as a binding type. `nil` is a value (the inhabitant
/// of the unit type), and nullability is expressed with `T?` — so any
/// occurrence of `nil` inside a binding/parameter/field type is a
/// foot-gun the typechecker should call out. Return types are *not*
/// validated here: `fn foo() -> nil` is the conventional "returns
/// nothing" signature.
pub(crate) fn reject_nil_in_binding_type(
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
            Type::Function { params, ret } => params.iter().any(walk) || walk_return(ret),
        }
    }
    /// The return slot of a function *type* is a return position, so a bare
    /// `nil` there is the same unit return a declaration writes — the rule
    /// that spares `fn foo() -> nil` has to spare `body: fn() -> nil` too.
    /// Nested occurrences (`fn() -> table<nil>`) are still binding types.
    fn walk_return(ty: &Type) -> bool {
        match ty {
            Type::Named(n) => n != "nil" && walk(ty),
            other => walk(other),
        }
    }
    if walk(ty) {
        errors.push(TypeCheckError::NilTypeAnnotation {
            span: to_source_span(span),
        });
    }
}

/// Run [`reject_nil_in_binding_type`] over every parameter's declared type.
pub(crate) fn reject_nil_in_params(params: &[Param], errors: &mut Vec<TypeCheckError>) {
    for p in params {
        reject_nil_in_binding_type(&p.ty, p.span.clone(), errors);
    }
}

/// Type-vs-type assignment compatibility check, used when the value side
/// is a tuple component (e.g. `local a, b = f()` where `f()` returns
/// `(A, B)`) and we don't have a per-element expression to feed through
/// [`check_assignment_compat`].
pub(crate) fn check_type_assignment_compat(
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
    // Widening into an `any` slot is always fine. An `any` value flowing
    // into a concrete slot is a downcast and must be written `x as T`.
    if is_any(decl_ty) {
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
pub(crate) fn refine_bare_binding(decl_ty: &Type, value: &Spanned<Expr>, scope: &Scope) -> Type {
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
pub(crate) fn is_assignment_compatible(
    decl_ty: &Type,
    value: &Spanned<Expr>,
    scope: &Scope,
) -> bool {
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

/// Verify that `obj.name = ...` is being written to a receiver that
/// actually supports field assignment. Class instances and class statics
/// do; plain tables, primitives, and functions don't.
/// The class whose field is being written by `obj.field = …`, if the
/// receiver resolves to one. `Some` for `self`, for an instance-typed
/// binding, and for a bare class name used as a static receiver; `None` for
/// tables and anything else, where there is no declared field type to check
/// against.
pub(crate) fn member_assign_class(obj: &Spanned<Expr>, scope: &Scope) -> Option<String> {
    // `Class.staticField = v`. A local of the same name shadows the class,
    // so only treat a bare identifier as a class when nothing is bound.
    if let Expr::Ident(n) = &obj.value
        && scope.lookup(n).is_none()
        && with_classes(|reg| reg.contains_key(n))
    {
        return Some(n.clone());
    }
    // `self.field = v` and `instance.field = v`.
    match strip_nullable(infer(obj, scope)?) {
        Type::Named(n) => Some(n),
        _ => None,
    }
}

pub(crate) fn check_member_assign_receiver(
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
            span: crate::to_source_span(span),
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
pub(crate) fn block_diverges(block: &[Spanned<Stmt>]) -> bool {
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
