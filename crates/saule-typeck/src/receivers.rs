//! What a value of a known type cannot do at all.
//!
//! `#n`, `n()`, `n[1]`, `n.field` and `for x in n` on an `integer` are errors
//! the moment they run. Nothing about them depends on the value, so they are
//! reported here rather than left for the program to reach: a line that is
//! only reached on some paths would otherwise stay wrong until it was.
//!
//! The table of what is refused mirrors the runtime's, and each message uses
//! its wording. Only types the checker is certain of take part. `any`, a type
//! parameter and anything it could not infer all pass, so dynamic code is
//! unaffected.

use saule_ast::{Expr, Spanned, Type};

use crate::expr::{infer, strip_nullable, type_to_string};
use crate::state::{Scope, with_enums};
use crate::{TypeCheckError, to_source_span};

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// `integer`, `float`, `boolean`.
    Scalar,
    /// `string`, `bytes`: a length, but nothing else.
    Text,
    Table,
    Function,
    Enum,
}

fn kind_of(ty: &Type) -> Option<Kind> {
    match ty {
        Type::Named(name) => match name.as_str() {
            "integer" | "float" | "boolean" => Some(Kind::Scalar),
            "string" | "bytes" => Some(Kind::Text),
            // A class of the same name as an enum cannot exist, so this is
            // the enum's values rather than something shadowing it.
            _ if with_enums(|r| r.contains_key(name)) => Some(Kind::Enum),
            _ => None,
        },
        Type::Table { .. } => Some(Kind::Table),
        Type::Function { .. } => Some(Kind::Function),
        _ => None,
    }
}

/// The receiver's kind and how to name its type. A bare name that means a
/// class, enum or stdlib module is not a value of that type, and `infer` is
/// not asked about it.
fn known(expr: &Spanned<Expr>, scope: &Scope) -> Option<(Kind, String)> {
    let ty = strip_nullable(infer(expr, scope)?);
    Some((kind_of(&ty)?, type_to_string(&ty)))
}

fn refuse(
    message: String,
    label: &'static str,
    span: std::ops::Range<usize>,
    errors: &mut Vec<TypeCheckError>,
) {
    errors.push(TypeCheckError::InvalidOperation {
        message,
        label,
        span: to_source_span(span),
    });
}

/// `#value`
pub(crate) fn check_length(value: &Spanned<Expr>, scope: &Scope, errors: &mut Vec<TypeCheckError>) {
    let Some((kind, ty)) = known(value, scope) else {
        return;
    };
    if matches!(kind, Kind::Scalar | Kind::Function | Kind::Enum) {
        refuse(
            format!(
                "cannot take length of a `{ty}` — only strings, bytes, tables, and classes \
                 implementing `OpLen` have a length"
            ),
            "has no length",
            value.span.clone(),
            errors,
        );
    }
}

/// `value[k]`, read or written.
pub(crate) fn check_index(obj: &Spanned<Expr>, scope: &Scope, errors: &mut Vec<TypeCheckError>) {
    let Some((kind, ty)) = known(obj, scope) else {
        return;
    };
    if kind != Kind::Table {
        refuse(
            format!(
                "cannot index a value of type `{ty}` — only tables, and classes implementing \
                 `OpIndex`, can be indexed"
            ),
            "cannot be indexed",
            obj.span.clone(),
            errors,
        );
    }
}

/// `value.name`. An enum value can have methods this pass does not track, and
/// a table's fields are its keys, so neither is refused. Not asked of `value?.name`,
/// which the language defines on a `nil` whatever type it might otherwise hold.
pub(crate) fn check_member(
    obj: &Spanned<Expr>,
    name: &str,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    let Some((kind, ty)) = known(obj, scope) else {
        return;
    };
    if matches!(kind, Kind::Scalar | Kind::Text | Kind::Function) {
        let module = match ty.as_str() {
            "string" => " — string functions live on `String`, as in `String.upper(s)`",
            "bytes" => " — bytes functions live on `Bytes`",
            _ => "",
        };
        refuse(
            format!("cannot read field `{name}` on value of type `{ty}`{module}"),
            "has no fields",
            obj.span.end..obj.span.end + name.len() + 1,
            errors,
        );
    }
}

/// `value(...)`, where `value` is not a member call. A bare name only counts
/// when it is a variable: `K()` constructs and `f()` calls a declaration, and
/// both are checked elsewhere.
pub(crate) fn check_call(callee: &Spanned<Expr>, scope: &Scope, errors: &mut Vec<TypeCheckError>) {
    if let Expr::Ident(name) = &callee.value
        && scope.lookup(name).is_none()
        && crate::vars::lookup(name).is_none()
    {
        return;
    }
    let Some((kind, ty)) = known(callee, scope) else {
        return;
    };
    if matches!(kind, Kind::Scalar | Kind::Text | Kind::Table) {
        refuse(
            format!(
                "value of type `{ty}` is not callable — only functions, classes, and enum \
                 variants can be called"
            ),
            "not callable",
            callee.span.clone(),
            errors,
        );
    }
}

/// `for ... in value`
pub(crate) fn check_iterable(
    iter: &Spanned<Expr>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    let Some((kind, ty)) = known(iter, scope) else {
        return;
    };
    if matches!(kind, Kind::Scalar | Kind::Text | Kind::Enum) {
        refuse(
            format!(
                "cannot iterate over a `{ty}` with `for ... in` — use a table, a function, or a \
                 class that implements `Iterable`"
            ),
            "not iterable",
            iter.span.clone(),
            errors,
        );
    }
}
