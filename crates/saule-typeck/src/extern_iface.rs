//! The rules for `extern interface`: what a method implemented in another
//! process may take and return.
//!
//! Everything in a call crosses a process boundary, so every parameter and
//! result must be something that can be sent:
//!
//! - `nil`, `boolean`, `integer`, `float`, `string`, `bytes`, `any`;
//! - `T?` and tables of anything that can be sent;
//! - another `extern interface` — sent **by reference**: the receiver gets a
//!   proxy and calls go back to the original;
//! - a plain data class whose fields can all be sent — **copied**, field by
//!   field, into an instance of the same class on the other side.
//!
//! Functions, local interfaces, enums, generic classes and stdlib handles
//! (files, processes, coroutines) cannot, and are reported where they are
//! written, with what to use instead.
//!
//! The declaration itself is kept simple for now: no generics, no `extends`,
//! no parameter defaults or variadics, at least one method, and `async fn`
//! returns nothing (a result to `await` comes with `Task<T>`).

use std::collections::HashSet;

use saule_ast::{MethodSig, Type, TypeRef};

use crate::TypeCheckError;
use crate::to_source_span;

pub(crate) fn check_extern_interface(
    name: &str,
    type_params: &[String],
    extends: &[TypeRef],
    methods: &[MethodSig],
    decl_span: std::ops::Range<usize>,
    errors: &mut Vec<TypeCheckError>,
) {
    let mut err = |message: String, help: &str, span: std::ops::Range<usize>| {
        errors.push(TypeCheckError::ExternInterface {
            message,
            help: help.to_string(),
            span: to_source_span(span),
        });
    };

    if !type_params.is_empty() {
        err(
            format!("`extern interface {name}` cannot be generic"),
            "both sides must agree on every type without seeing each other's code; name the types",
            decl_span.clone(),
        );
    }
    if !extends.is_empty() {
        err(
            format!("`extern interface {name}` cannot extend another interface yet"),
            "declare the methods it needs directly",
            decl_span.clone(),
        );
    }
    if methods.is_empty() {
        err(
            format!("`extern interface {name}` declares no methods"),
            "an extern interface is a set of calls another process answers; declare at least one",
            decl_span,
        );
    }

    for m in methods {
        let what = format!("`{name}.{}`", m.name);
        if !m.type_params.is_empty() {
            err(
                format!("{what} cannot be generic"),
                "a method called from another process must name its types",
                m.span.clone(),
            );
        }
        for p in &m.params {
            if p.variadic {
                err(
                    format!("{what} cannot take a variadic parameter"),
                    "take a table instead",
                    p.span.clone(),
                );
            }
            if p.default.is_some() {
                err(
                    format!("parameter `{}` of {what} cannot have a default", p.name),
                    "a default is evaluated by the implementation, which runs in another process; \
                     make the parameter nullable and handle `nil` there",
                    p.span.clone(),
                );
            }
            if let Err(why) = sendable(&p.ty, &mut HashSet::new()) {
                err(
                    format!("parameter `{}` of {what} cannot be sent to another process: {why}", p.name),
                    SENDABLE_HELP,
                    p.span.clone(),
                );
            }
        }
        match &m.return_ty {
            Some(_) if m.is_async => err(
                format!("{what} is `async`, so it cannot return a value yet"),
                "drop the return type, or make it a plain `fn` that the caller waits for",
                m.span.clone(),
            ),
            Some(Type::Tuple(items)) => {
                for t in items {
                    if let Err(why) = sendable(t, &mut HashSet::new()) {
                        err(
                            format!("{what} returns something that cannot be sent back: {why}"),
                            SENDABLE_HELP,
                            m.span.clone(),
                        );
                    }
                }
            }
            Some(t) => {
                if let Err(why) = sendable(t, &mut HashSet::new()) {
                    err(
                        format!("{what} returns something that cannot be sent back: {why}"),
                        SENDABLE_HELP,
                        m.span.clone(),
                    );
                }
            }
            None => {}
        }
    }
}

const SENDABLE_HELP: &str = "values that cross: nil, boolean, integer, float, string, bytes, any, \
     T?, tables of those, an `extern interface` (by reference), or a class whose fields all cross (copied)";

/// Whether a value of type `ty` can be sent to another process; why not if
/// it cannot. `seen` stops a data class that contains itself from looping.
pub(crate) fn sendable(ty: &Type, seen: &mut HashSet<String>) -> Result<(), String> {
    match ty {
        Type::Named(n) => sendable_named(n, seen),
        Type::Nullable(inner) => sendable(inner, seen),
        Type::Table { key, value } => {
            if let Some(k) = key {
                sendable(k, seen)?;
            }
            sendable(value, seen)
        }
        Type::Tuple(_) => Err("a tuple can only be a method's whole result".into()),
        Type::Function { .. } => Err(
            "a function cannot cross processes — for a callback, pass an object implementing an `extern interface`"
                .into(),
        ),
        Type::Generic(g) => Err(format!(
            "`{}` is generic, and generic classes cannot cross yet",
            g.name
        )),
    }
}

fn sendable_named(name: &str, seen: &mut HashSet<String>) -> Result<(), String> {
    match name {
        "nil" | "boolean" | "integer" | "float" | "string" | "bytes" | "byte" | "any" => {
            return Ok(());
        }
        _ => {}
    }
    if saule_semantic::is_extern_interface(name) {
        return Ok(());
    }
    if saule_semantic::is_interface(name) {
        return Err(format!(
            "`{name}` is a local interface — make it an `extern interface` to pass it by reference"
        ));
    }
    if saule_semantic::with_enums(|r| r.contains_key(name)) {
        return Err(format!(
            "`{name}` is an enum, and enums cannot cross yet — send the variant's name as a string"
        ));
    }
    if crate::sigs::is_value_type(name) {
        return Err(format!(
            "a `{name}` belongs to this process and cannot be handed to another"
        ));
    }
    let fields = saule_semantic::with_classes(|r| {
        r.get(name).map(|c| {
            c.field_types
                .iter()
                .filter(|(f, _)| !c.static_fields.contains(*f))
                .map(|(f, t)| (f.clone(), t.clone()))
                .collect::<Vec<_>>()
        })
    });
    let Some(fields) = fields else {
        return Err(format!("`{name}` is not a type that can be sent"));
    };
    if !seen.insert(name.to_string()) {
        return Ok(());
    }
    for (field, t) in &fields {
        sendable(t, seen).map_err(|why| format!("field `{field}` of `{name}`: {why}"))?;
    }
    Ok(())
}

/// The statics every `extern interface X` has, though it declares none:
///
/// - `X.connect(name: string) -> X` — a proxy for the service published as
///   `name`, whose methods are calls to it;
/// - `X.publish(name: string, service: X)` — offer `service`, an object of
///   this process implementing `X`, to other processes under `name`.
///
/// `None` for anything else, including a plain interface.
pub(crate) fn extern_static(iface: &str, method: &str) -> Option<saule_semantic::MethodSig> {
    if !saule_semantic::is_extern_interface(iface) {
        return None;
    }
    let param = |name: &str, ty: Type| saule_ast::Param {
        name: name.into(),
        ty,
        default: None,
        variadic: false,
        span: 0..0,
    };
    let me = Type::Named(iface.to_string());
    let string = Type::Named("string".into());
    let (params, return_ty) = match method {
        "connect" => (vec![param("name", string)], Some(me)),
        "publish" => (vec![param("name", string), param("service", me)], None),
        _ => return None,
    };
    Some(saule_semantic::MethodSig {
        is_static: true,
        is_private: false,
        remote: saule_semantic::Remote::No,
        type_params: Vec::new(),
        type_bounds: Vec::new(),
        params,
        return_ty,
    })
}
