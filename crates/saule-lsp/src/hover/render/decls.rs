//! Rendering declarations: functions, methods, parameters and
//! fields, as they appear at the top of a hover popup.

use saule_ast::{Expr, Method, Param, Type, UnaryOp};
use saule_semantic::MethodSig;
use saule_typeck::sigs::NativeSig;

use super::*;

pub(crate) fn render_function_sig(
    name: &str,
    type_params: &[String],
    params: &[Param],
    return_ty: Option<&Type>,
) -> String {
    let mut prefix = String::from("fn ");
    prefix.push_str(name);
    if !type_params.is_empty() {
        prefix.push('<');
        prefix.push_str(&type_params.join(", "));
        prefix.push('>');
    }
    let suffix = match return_ty {
        Some(rt) => format!(" -> {}", render_type(rt)),
        None => String::new(),
    };
    let params: Vec<String> = params.iter().map(render_param_inline).collect();
    format!(
        "```saule\n{}\n```",
        render_call_shape(&prefix, &params, &suffix, 0)
    )
}

/// `return_ty` is passed in rather than read off `m` so the caller can
/// supply the type the semantic pass inferred for a method that declared
/// none. Pass `m.return_ty.as_ref()` for the written one.
pub(crate) fn render_method_head(owner: &str, m: &Method, return_ty: Option<&Type>) -> String {
    let sig = MethodSig {
        is_static: m.is_static,
        is_private: m.is_private,
        type_params: m.type_params.clone(),
        params: m.params.clone(),
        return_ty: return_ty.cloned(),
    };
    render_method_sig(owner, &m.name, &sig)
}

pub(crate) fn render_method_sig(owner: &str, name: &str, sig: &MethodSig) -> String {
    let mut prefix = String::new();
    if sig.is_private {
        prefix.push_str("private ");
    }
    if sig.is_static {
        prefix.push_str("static ");
    }
    prefix.push_str("fn ");
    if !owner.is_empty() {
        prefix.push_str(owner);
        prefix.push('.');
    }
    prefix.push_str(name);
    if !sig.type_params.is_empty() {
        prefix.push('<');
        prefix.push_str(&sig.type_params.join(", "));
        prefix.push('>');
    }
    let suffix = match &sig.return_ty {
        Some(rt) => format!(" -> {}", render_type(rt)),
        None => String::new(),
    };
    let params: Vec<String> = sig.params.iter().map(render_param_inline).collect();
    format!(
        "```saule\n{}\n```",
        render_call_shape(&prefix, &params, &suffix, 0)
    )
}

pub(crate) fn render_param(p: &Param) -> String {
    format!("```saule\n(parameter) {}\n```", render_param_inline(p))
}

pub(crate) fn render_param_inline(p: &Param) -> String {
    let mut s = String::new();
    if p.variadic {
        s.push_str("...");
    }
    s.push_str(&p.name);
    s.push_str(": ");
    s.push_str(&render_type(&p.ty));
    s.push_str(&render_default_suffix(p));
    s
}

/// The ` = <default>` tail of a parameter, or the empty string when the
/// parameter is required.
pub(crate) fn render_default_suffix(p: &Param) -> String {
    match &p.default {
        Some(d) => format!(" = {}", render_default(&d.value)),
        None => String::new(),
    }
}

/// Render a parameter's default value for display.
///
/// Simple constants print as written: `width: integer = 900` answers
/// "what do I get if I leave this out?", which the bare `…` placeholder
/// never did — and on a class like `WindowGroup`, where every parameter
/// is defaulted, seven repeats of `= …` were pure noise.
///
/// Anything with sub-expressions (a call, a table constructor,
/// arithmetic) falls back to `…`. Those have no length bound, and a
/// hover line that wraps costs the reader more than the value is worth.
pub(crate) fn render_default(e: &Expr) -> String {
    const ELIDED: &str = "…";
    match e {
        Expr::Int(i) => i.to_string(),
        // `Display` drops the fraction on a whole float, so a `float`
        // parameter defaulting to `1.0` would advertise `= 1` and read
        // as an integer. `Debug` keeps the point.
        Expr::Float(f) => format!("{f:?}"),
        Expr::Bool(b) => b.to_string(),
        Expr::Nil => "nil".to_string(),
        // Long strings are the one literal that can blow the line
        // budget on its own, so they elide on length.
        Expr::Str(s) if s.chars().count() <= 24 => format!("{s:?}"),
        Expr::Ident(n) => n.clone(),
        // `Color.black`, `Align.center` — reads as one token to the
        // user even though the AST nests it.
        Expr::Member { obj, name } => match &obj.value {
            Expr::Ident(o) => format!("{o}.{name}"),
            _ => ELIDED.to_string(),
        },
        // Negative number literals only; `-foo()` stays elided.
        Expr::Unary {
            op: UnaryOp::Neg,
            rhs,
        } => match &rhs.value {
            Expr::Int(_) | Expr::Float(_) => format!("-{}", render_default(&rhs.value)),
            _ => ELIDED.to_string(),
        },
        _ => ELIDED.to_string(),
    }
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
