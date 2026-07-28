//! Markdown rendering helpers — every render_* function that turns
//! AST / registry entries into the fenced-code-block blurbs editors
//! display in the hover popup.

use std::collections::HashMap;

use saule_ast::{
    Decl, EnumVariant, Method, MethodSig as AstMethodSig, Module, Param, Pattern, Spanned, Stmt,
    Type,
};
use saule_docs::DocBlock;
use saule_semantic::{ClassInfo, MethodSig};
use saule_typeck::sigs::NativeSig;

pub(super) fn render_function_sig(
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

pub(super) fn render_method_head(owner: &str, m: &Method) -> String {
    let sig = MethodSig {
        is_static: m.is_static,
        is_private: m.is_private,
        type_params: m.type_params.clone(),
        params: m.params.clone(),
        return_ty: m.return_ty.clone(),
    };
    render_method_sig(owner, &m.name, &sig)
}

pub(super) fn render_method_sig(owner: &str, name: &str, sig: &MethodSig) -> String {
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

pub(super) fn render_param(p: &Param) -> String {
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

pub(super) fn render_param_inline(p: &Param) -> String {
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

pub(super) fn render_field(
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

pub(super) fn render_class_head(
    name: &str,
    extends: Option<&str>,
    implements: &[String],
) -> String {
    let mut s = format!("```saule\nclass {name}");
    if let Some(p) = extends {
        s.push_str(" extends ");
        s.push_str(p);
    }
    if !implements.is_empty() {
        s.push_str(" implements ");
        s.push_str(&implements.join(", "));
    }
    s.push_str("\n```");
    s
}

/// Render a class with its full public surface — heading, then a body
/// listing every non-private field and method in alphabetical order.
/// This is the same format used for `Ident` hover on a class name and
/// for hover on a `class` declaration head, so the two views agree.
pub(super) fn render_class_full(name: &str, info: &ClassInfo) -> String {
    let mut s = format!("```saule\nclass {name}");
    if let Some(p) = &info.parent {
        s.push_str(" extends ");
        s.push_str(p);
    }
    if !info.implements.is_empty() {
        s.push_str(" implements ");
        s.push_str(&info.implements.join(", "));
    }

    // Public surface — `info.members` is the canonical visibility map.
    // Sort lexicographically so the same class always renders the same
    // way regardless of HashMap iteration order.
    let mut public: Vec<&String> = info
        .members
        .iter()
        .filter_map(|(n, priv_)| if *priv_ { None } else { Some(n) })
        .collect();
    public.sort();

    if !public.is_empty() {
        s.push_str(" {\n");
        for member in public {
            if let Some(sig) = info.methods.get(member) {
                s.push_str("  ");
                if sig.is_static {
                    s.push_str("static ");
                }
                s.push_str("fn ");
                s.push_str(member);
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
            } else if let Some(ty) = info.field_types.get(member) {
                s.push_str("  ");
                s.push_str(member);
                s.push_str(": ");
                s.push_str(&render_type(ty));
            } else {
                // Inherited or otherwise unsourced — still surface the
                // name so the hover doesn't lie about the API.
                s.push_str("  ");
                s.push_str(member);
            }
            s.push('\n');
        }
        s.push('}');
    }
    s.push_str("\n```");
    s
}

/// Hover for a stdlib static-class identifier (`Math`, `String`, …) or
/// a value type (`File`). Lists every member known to
/// `saule_typeck::sigs`, looking up signatures where available so the
/// reader sees `fn sqrt(number) -> float` instead of just `sqrt`.
pub(super) fn render_stdlib_module(name: &str, kind: &str) -> String {
    let mut members = saule_typeck::sigs::module_members(name);
    members.sort();
    let mut s = format!("```saule\n{kind} {name}");
    if members.is_empty() {
        s.push_str("\n```");
        return s;
    }
    s.push_str(" {\n");
    for m in &members {
        let qname = format!("{name}.{m}");
        if let Some(sig) = saule_typeck::sigs::lookup(&qname) {
            s.push_str("  fn ");
            s.push_str(m);
            s.push_str(&render_native_sig_full(&sig));
        } else {
            // Value field with no registered call signature
            // (e.g. `Math.pi`, `Os.sep`).
            s.push_str("  ");
            s.push_str(m);
        }
        s.push('\n');
    }
    s.push_str("}\n```");
    s
}

/// Render a `NativeSig` as `[<T, U>](Type1, Type2, ...Variadic) -> Ret`.
/// Native signatures don't carry parameter names, so we print types
/// only — the user gets arity, types, and return shape, which is what
/// most stdlib calls actually need.
pub(super) fn render_native_sig_full(sig: &NativeSig) -> String {
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

pub(super) fn render_interface_head(
    name: &str,
    extends: &[String],
    methods: &[AstMethodSig],
) -> String {
    let mut s = format!("```saule\ninterface {name}");
    if !extends.is_empty() {
        s.push_str(" extends ");
        s.push_str(&extends.join(", "));
    }
    if !methods.is_empty() {
        s.push_str(" {\n");
        for m in methods {
            s.push_str("  fn ");
            s.push_str(&m.name);
            s.push('(');
            s.push_str(
                &m.params
                    .iter()
                    .map(render_param_inline)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            s.push(')');
            if let Some(rt) = &m.return_ty {
                s.push_str(" -> ");
                s.push_str(&render_type(rt));
            }
            s.push('\n');
        }
        s.push('}');
    }
    s.push_str("\n```");
    s
}

pub(super) fn render_interface_from_registry(name: &str, extends: &[String]) -> String {
    render_interface_head(name, extends, &[])
}

pub(super) fn render_enum_head(name: &str, variants: &[Spanned<EnumVariant>]) -> String {
    let mut s = format!("```saule\nenum {name} {{\n");
    for v in variants {
        s.push_str("  ");
        match &v.value {
            EnumVariant::Bare(n) => s.push_str(n),
            EnumVariant::Valued(n, _) => {
                s.push_str(n);
                s.push_str(" = …");
            }
            EnumVariant::Tuple { name, fields } => {
                s.push_str(name);
                s.push('(');
                s.push_str(
                    &fields
                        .iter()
                        .map(render_param_inline)
                        .collect::<Vec<_>>()
                        .join(", "),
                );
                s.push(')');
            }
        }
        s.push('\n');
    }
    s.push_str("}\n```");
    s
}

pub(super) fn render_enum_from_registry(name: &str, variants: &[(String, usize)]) -> String {
    let mut s = format!("```saule\nenum {name} {{\n");
    for (vn, arity) in variants {
        s.push_str("  ");
        s.push_str(vn);
        if *arity > 0 {
            s.push('(');
            s.push_str(&"_, ".repeat(*arity));
            // Trim trailing ", "
            s.truncate(s.len() - 2);
            s.push(')');
        }
        s.push('\n');
    }
    s.push_str("}\n```");
    s
}

/// Local copy of the type pretty-printer. Kept here (rather than reused
/// from `saule-semantic::return_check`) to avoid widening that crate's
/// public surface for what amounts to one display helper.
pub(super) fn render_type(ty: &Type) -> String {
    match ty {
        Type::Named(n) => n.clone(),
        Type::Nullable(inner) => format!("{}?", render_type(inner)),
        Type::Table { key: None, value } => format!("table<{}>", render_type(value)),
        Type::Table {
            key: Some(k),
            value,
        } => format!("table<{}, {}>", render_type(k), render_type(value)),
        Type::Tuple(parts) => {
            let inner: Vec<_> = parts.iter().map(render_type).collect();
            format!("({})", inner.join(", "))
        }
        Type::Function { params, ret } => {
            let p: Vec<_> = params.iter().map(render_type).collect();
            format!("fn({}) -> {}", p.join(", "), render_type(ret))
        }
    }
}

/// Pre-pass: walk every `enum` declaration in `module` and record the
/// payload-field shape of each tuple variant. Used by `bind_pattern` to
/// type the names introduced by `case Enum.Variant(a, b, ...)` patterns
/// without having to re-find the decl per arm.
pub(super) fn collect_enum_variant_fields(
    module: &Module,
) -> HashMap<(String, String), Vec<Param>> {
    let mut out: HashMap<(String, String), Vec<Param>> = HashMap::new();
    for s in &module.stmts {
        if let Stmt::Decl(d) = &s.value
            && let Decl::Enum { name, variants, .. } = &d.value
        {
            for v in variants {
                if let EnumVariant::Tuple { name: vn, fields } = &v.value {
                    out.insert((name.clone(), vn.clone()), fields.clone());
                }
            }
        }
    }
    out
}

/// Render hover info for a `Variant` pattern, e.g.
/// `(variant) Event.Click(x: integer, y: integer)`. Falls back to a
/// bare `Enum.Variant` when the enum isn't in our pre-collected map
/// (likely a bare or valued variant that carries no payload).
pub(super) fn render_variant_pattern(
    enum_name: &str,
    variant: &str,
    fields: &[Spanned<Pattern>],
    enum_fields: &HashMap<(String, String), Vec<Param>>,
) -> String {
    let mut s = format!("```saule\n(variant) {enum_name}.{variant}");
    if let Some(params) = enum_fields.get(&(enum_name.to_string(), variant.to_string())) {
        s.push('(');
        s.push_str(
            &params
                .iter()
                .map(render_param_inline)
                .collect::<Vec<_>>()
                .join(", "),
        );
        s.push(')');
    } else if !fields.is_empty() {
        // Unknown enum but pattern carries sub-patterns — surface
        // arity at least.
        s.push('(');
        s.push_str(&"_, ".repeat(fields.len()));
        s.truncate(s.len() - 2);
        s.push(')');
    }
    s.push_str("\n```");
    s
}

// ─── doc comments ───────────────────────────────────────────────────────────

/// Append a declaration's `---` doc comment below its rendered
/// signature, separated by a horizontal rule so the popup reads as
/// "here's the shape, here's what it means".
///
/// A missing or empty block leaves `md` untouched, which is what keeps
/// undocumented code hovering exactly as it did before.
pub(super) fn with_doc(md: String, doc: Option<&DocBlock>) -> String {
    match doc {
        Some(d) if !d.is_empty() => format!("{md}\n\n---\n\n{}", d.to_markdown()),
        _ => md,
    }
}

/// Append the `@param <name>` description to a parameter's hover.
///
/// Only the prose for this one parameter is shown — the enclosing
/// function's summary belongs on the function, not on every parameter
/// inside it.
pub(super) fn with_param_doc(md: String, doc: Option<&DocBlock>, name: &str) -> String {
    match doc.and_then(|d| d.param(name)) {
        Some(desc) if !desc.trim().is_empty() => format!("{md}\n\n---\n\n{desc}"),
        _ => md,
    }
}

/// Signature blurb for one `interface` method. Interface methods are
/// bodiless `MethodSig`s rather than `Method`s, so they need their own
/// small renderer to become a hover target in their own right.
pub(super) fn render_interface_method(owner: &str, m: &AstMethodSig) -> String {
    let sig = MethodSig {
        is_static: false,
        is_private: false,
        type_params: Vec::new(),
        params: m.params.clone(),
        return_ty: m.return_ty.clone(),
    };
    render_method_sig(owner, &m.name, &sig)
}

/// Declaration blurb for one enum variant, so a `---` comment written
/// above `North` has somewhere to surface.
pub(super) fn render_enum_variant_decl(owner: &str, v: &EnumVariant) -> String {
    let mut s = String::from("```saule\n");
    s.push_str(owner);
    s.push('.');
    match v {
        EnumVariant::Bare(n) => s.push_str(n),
        EnumVariant::Valued(n, _) => {
            s.push_str(n);
            s.push_str(" = …");
        }
        EnumVariant::Tuple { name, fields } => {
            s.push_str(name);
            s.push('(');
            s.push_str(
                &fields
                    .iter()
                    .map(render_param_inline)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            s.push(')');
        }
    }
    s.push_str("\n```");
    s
}
