//! Rendering declarations: functions, methods, parameters and
//! fields, as they appear at the top of a hover popup.

use saule_ast::{Method, Param, Type};
use saule_semantic::MethodSig;
use saule_typeck::sigs::NativeSig;

use super::*;

pub(crate) fn render_function_sig(
    name: &str,
    type_params: &[String],
    params: &[Param],
    return_ty: Option<&Type>,
) -> String {
    let mut s = String::from("```saule\nfn ");
    s.push_str(name);
    if !type_params.is_empty() {
        s.push('<');
        s.push_str(&type_params.join(", "));
        s.push('>');
    }
    s.push('(');
    s.push_str(
        &params
            .iter()
            .map(render_param_inline)
            .collect::<Vec<_>>()
            .join(", "),
    );
    s.push(')');
    if let Some(rt) = return_ty {
        s.push_str(" -> ");
        s.push_str(&render_type(rt));
    }
    s.push_str("\n```");
    s
}

pub(crate) fn render_method_head(owner: &str, m: &Method) -> String {
    let sig = MethodSig {
        is_static: m.is_static,
        is_private: m.is_private,
        type_params: m.type_params.clone(),
        params: m.params.clone(),
        return_ty: m.return_ty.clone(),
    };
    render_method_sig(owner, &m.name, &sig)
}

pub(crate) fn render_method_sig(owner: &str, name: &str, sig: &MethodSig) -> String {
    let mut s = String::from("```saule\n");
    if sig.is_private {
        s.push_str("private ");
    }
    if sig.is_static {
        s.push_str("static ");
    }
    s.push_str("fn ");
    if !owner.is_empty() {
        s.push_str(owner);
        s.push('.');
    }
    s.push_str(name);
    if !sig.type_params.is_empty() {
        s.push('<');
        s.push_str(&sig.type_params.join(", "));
        s.push('>');
    }
    s.push('(');
    s.push_str(
        &sig.params
            .iter()
            .map(render_param_inline)
            .collect::<Vec<_>>()
            .join(", "),
    );
    s.push(')');
    if let Some(rt) = &sig.return_ty {
        s.push_str(" -> ");
        s.push_str(&render_type(rt));
    }
    s.push_str("\n```");
    s
}

pub(crate) fn render_param(p: &Param) -> String {
    let mut s = String::from("```saule\n(parameter) ");
    if p.variadic {
        s.push_str("...");
    }
    s.push_str(&p.name);
    s.push_str(": ");
    s.push_str(&render_type(&p.ty));
    if p.default.is_some() {
        s.push_str(" = …");
    }
    s.push_str("\n```");
    s
}

pub(crate) fn render_param_inline(p: &Param) -> String {
    let mut s = String::new();
    if p.variadic {
        s.push_str("...");
    }
    s.push_str(&p.name);
    s.push_str(": ");
    s.push_str(&render_type(&p.ty));
    if p.default.is_some() {
        s.push_str(" = …");
    }
    s
}

pub(crate) fn render_field(
    owner: &str,
    is_static: bool,
    is_private: bool,
    name: &str,
    ty: &Type,
) -> String {
    let mut s = String::from("```saule\n(field) ");
    if is_private {
        s.push_str("private ");
    }
    if is_static {
        s.push_str("static ");
    }
    if !owner.is_empty() {
        s.push_str(owner);
        s.push('.');
    }
    s.push_str(name);
    s.push_str(": ");
    s.push_str(&render_type(ty));
    s.push_str("\n```");
    s
}

/// Render a `NativeSig` as `[<T, U>](Type1, Type2, ...Variadic) -> Ret`.
/// Native signatures don't carry parameter names, so we print types
/// only — the user gets arity, types, and return shape, which is what
/// most stdlib calls actually need.
pub(crate) fn render_native_sig_full(sig: &NativeSig) -> String {
    let mut s = String::new();
    if !sig.type_params.is_empty() {
        s.push('<');
        s.push_str(&sig.type_params.join(", "));
        s.push('>');
    }
    s.push('(');
    let mut parts: Vec<String> = sig.params.iter().map(render_type).collect();
    if let Some(v) = &sig.variadic {
        parts.push(format!("...{}", render_type(v)));
    }
    s.push_str(&parts.join(", "));
    s.push(')');
    if !sig.returns.is_empty() {
        s.push_str(" -> ");
        if sig.returns.len() == 1 {
            s.push_str(&render_type(&sig.returns[0]));
        } else {
            // Multi-return: surface as a tuple.
            s.push('(');
            s.push_str(
                &sig.returns
                    .iter()
                    .map(render_type)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            s.push(')');
        }
    }
    s
}
