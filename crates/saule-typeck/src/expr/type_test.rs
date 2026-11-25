//! `v is T` — the runtime type test.
//!
//! The test itself is the one the checked reading of `as` makes, and the
//! runtime answers both from one function. What the checker adds here is
//! the two things a test gives that a cast does not:
//!
//! * a refusal when the answer is already known to be no — `count is
//!   string` with `count: integer` is a mistake, not a question;
//! * the *narrowing*: inside `if v is T then`, `v` is a `T`
//!   (see [`super::narrow`]).

use saule_ast::{Expr, Spanned, Type, UnaryOp};
use saule_semantic::{is_interface, is_subtype_named};

use super::generics::is_any;
use super::infer::{infer, strip_nullable};
use super::{type_to_string, types_compatible};
use crate::TypeCheckError;
use crate::state::{Scope, is_type_param, with_classes, with_enums};
use crate::to_source_span;

/// Check `value is ty`, whose whole expression is `expr`.
pub(crate) fn check(
    expr: &Spanned<Expr>,
    value: &Spanned<Expr>,
    ty: &Type,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    // `nil` is a value. The annotation rules would refuse it too, but with
    // advice about `?` that does not answer what was being asked here.
    if matches!(ty, Type::Named(n) if n == "nil") {
        errors.push(TypeCheckError::IsNil {
            span: to_source_span(expr.span.clone()),
        });
        return;
    }
    crate::stmt::check_binding_type(ty, expr.span.clone(), errors);

    // A type parameter is gone by the time the program runs: there is
    // nothing to test a value against.
    if let Some(name) = type_param_in(ty) {
        errors.push(TypeCheckError::IsTypeParameter {
            name,
            span: to_source_span(expr.span.clone()),
        });
        return;
    }

    let Some(source) = infer(value, scope) else {
        return;
    };
    if never(&source, ty) {
        // `not x is T` is `(not x) is T`: a boolean, tested against `T`.
        let negation = matches!(
            &value.value,
            Expr::Unary {
                op: UnaryOp::Not,
                ..
            }
        );
        errors.push(TypeCheckError::ImpossibleTypeTest {
            from: type_to_string(&source),
            to: type_to_string(ty),
            help: if negation {
                "`not` binds tighter than `is`, so this tests the result of the `not`; \
                 write `v is not T` to ask whether `v` is something else"
                    .to_string()
            } else {
                "the value's type is already known, and nothing of that type is one of \
                 these; remove the test, or check the type the value was declared with"
                    .to_string()
            },
            span: to_source_span(expr.span.clone()),
        });
    }
}

/// The first type parameter in scope that `ty` mentions.
fn type_param_in(ty: &Type) -> Option<String> {
    match ty {
        Type::Named(n) => is_type_param(n).then(|| n.clone()),
        Type::Nullable(inner) => type_param_in(inner),
        Type::Table { key, value } => key
            .as_deref()
            .and_then(type_param_in)
            .or_else(|| type_param_in(value)),
        Type::Tuple(items) => items.iter().find_map(type_param_in),
        Type::Function { params, ret } => params
            .iter()
            .find_map(type_param_in)
            .or_else(|| type_param_in(ret)),
        Type::Generic(g) => g.args.iter().find_map(type_param_in),
    }
}

/// What kind of thing a type is, as far as telling two apart goes.
#[derive(PartialEq)]
enum Kind<'a> {
    /// `integer`, `float`, `string`, `boolean`, `bytes` — values that are
    /// exactly one of these and nothing else.
    Primitive(&'a str),
    Table,
    Function,
    Class(&'a str),
    Enum(&'a str),
    Interface,
    /// Anything this cannot classify. Never the basis of a refusal.
    Other,
}

fn kind(ty: &Type) -> Kind<'_> {
    let name = match ty {
        Type::Table { .. } => return Kind::Table,
        Type::Function { .. } => return Kind::Function,
        Type::Named(n) => n.as_str(),
        Type::Generic(g) => g.name.as_str(),
        Type::Nullable(inner) => return kind(inner),
        Type::Tuple(_) => return Kind::Other,
    };
    match name {
        "integer" | "float" | "string" | "boolean" | "bytes" => Kind::Primitive(name),
        "table" => Kind::Table,
        _ if is_type_param(name) => Kind::Other,
        _ if with_classes(|r| r.contains_key(name)) => Kind::Class(name),
        _ if with_enums(|r| r.contains_key(name)) => Kind::Enum(name),
        _ if is_interface(name) => Kind::Interface,
        // `any`, `byte` (an integer in a range), `userdata`, a stdlib
        // value type, a name nothing declares.
        _ => Kind::Other,
    }
}

/// Whether a value of type `source` can be shown, from the types alone,
/// never to be a `target`.
///
/// Conservative: it says yes only for pairs with no value in common. A
/// class against an interface is never one of them — a subclass may
/// implement what its parent does not.
fn never(source: &Type, target: &Type) -> bool {
    // `nil` is in both.
    if matches!(source, Type::Nullable(_)) && matches!(target, Type::Nullable(_)) {
        return false;
    }
    let bare = strip_nullable(source.clone());
    if is_any(&bare) {
        return false;
    }
    match (kind(&bare), kind(target)) {
        (Kind::Other, _) | (_, Kind::Other) => false,
        (Kind::Primitive(a), Kind::Primitive(b)) => a != b,
        (Kind::Enum(a), Kind::Enum(b)) => a != b,
        // One line of descent: two classes share an instance only when one
        // is an ancestor of the other.
        (Kind::Class(a), Kind::Class(b)) => !is_subtype_named(a, b) && !is_subtype_named(b, a),
        (Kind::Table, Kind::Table) | (Kind::Function, Kind::Function) => false,
        (Kind::Interface, Kind::Interface | Kind::Class(_)) | (Kind::Class(_), Kind::Interface) => {
            false
        }
        // Different kinds of thing altogether.
        _ => true,
    }
}

/// What `name` is known to be where `name is ty` held.
///
/// The more specific of the two: a `Shape` that `is Circle` is a `Circle`,
/// while a `Circle` that `is Shape` is still a `Circle` — narrowing it to
/// the interface would take its fields away.
pub(crate) fn narrowed(current: &Type, ty: &Type) -> Type {
    let bare = strip_nullable(current.clone());
    let tested = strip_nullable(ty.clone());
    // Nothing is known about an `any`, and a type the value does not
    // already fit is news: either way the tested type is the answer.
    if is_any(&bare) || !types_compatible(&tested, &bare) {
        return ty.clone();
    }
    // The value already was one of these. All the test can have added is
    // that it is not nil — unless the tested type allows nil too.
    match ty {
        Type::Nullable(_) => current.clone(),
        _ => bare,
    }
}
