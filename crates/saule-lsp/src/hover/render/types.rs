//! Rendering the type-level constructs — classes, interfaces, enums
//! and the types themselves — in both their short (head) and full
//! forms.

use saule_ast::{
    Decl, EnumVariant, MethodSig as AstMethodSig, Module, Param, Pattern, Spanned, Stmt, Type,
    TypeRef,
};
use saule_semantic::{ClassInfo, MethodSig};
use std::collections::HashMap;

use super::*;

/// `<T, U>` as written on a declaration, or nothing when it declares none.
pub(crate) fn render_type_params(type_params: &[String]) -> String {
    if type_params.is_empty() {
        String::new()
    } else {
        format!("<{}>", type_params.join(", "))
    }
}

/// A named type reference from a declaration header, arguments included —
/// `Animal`, or `Repository<Player>`.
pub(crate) fn render_type_ref(r: &TypeRef) -> String {
    if r.args.is_empty() {
        return r.name.clone();
    }
    let args: Vec<String> = r.args.iter().map(render_type).collect();
    format!("{}<{}>", r.name, args.join(", "))
}

pub(crate) fn render_class_head(
    name: &str,
    type_params: &[String],
    extends: Option<&TypeRef>,
    implements: &[TypeRef],
) -> String {
    let mut s = format!("```saule\nclass {name}{}", render_type_params(type_params));
    if let Some(p) = extends {
        s.push_str(" extends ");
        s.push_str(&render_type_ref(p));
    }
    if !implements.is_empty() {
        let names: Vec<String> = implements.iter().map(render_type_ref).collect();
        s.push_str(" implements ");
        s.push_str(&names.join(", "));
    }
    s.push_str("\n```");
    s
}

/// Render a class with its full public surface. This is the same format
/// used for `Ident` hover on a class name and for hover on a `class`
/// declaration head, so the two views agree.
///
/// The layout is a one-line head followed by labelled sections:
///
/// ```text
/// class WindowGroup extends Scene
///
/// -- Params
///   title: string = "Saule"
///   width: integer = 900
///
/// -- Fields
///   background: Color?
///
/// -- Functions
///   fn present()
/// ```
///
/// Sections beat the class-shaped body this replaced for one reason:
/// the reader is asking a specific question — "how do I build one?",
/// "what can I read off it?", "what can I call?" — and a label answers
/// it without them having to infer the answer from punctuation. The old
/// form hoisted the constructor onto the head as `class C(a, b, …)`,
/// which soft-wrapped at whatever column the popup happened to be and
/// stranded `extends Scene` mid-parameter-list, then ran fields and
/// methods together in one alphabetical column.
///
/// **The whole blurb is one `saule` fence, deliberately.** An editor
/// syntax-highlights a fenced block and nothing else — Markdown carries
/// no language on inline code — so rendering the entries as real
/// Markdown bullets costs every entry its colours, which on a wide
/// signature is most of what makes the type scannable. Keeping one
/// fence also puts the blank lines under our control instead of the
/// renderer's paragraph margins, which is what lets each label sit
/// directly on top of its list.
pub(crate) fn render_class_full(name: &str, info: &ClassInfo) -> String {
    let mut head = format!("```saule\nclass {name}");
    if let Some(p) = &info.parent {
        head.push_str(" extends ");
        head.push_str(p);
    }
    if !info.implements.is_empty() {
        head.push_str(" implements ");
        head.push_str(&info.implements.join(", "));
    }

    // Constructor parameters. A private `init` contributes nothing: the
    // class can't be built from outside, so advertising a call shape
    // would lie. Deliberately *not* sorted — these are positional at the
    // call site, and any order but the declared one is a wrong answer to
    // "how do I build one?".
    let params: Vec<String> = info
        .methods
        .get("init")
        .filter(|sig| !sig.is_private)
        .map(|ctor| ctor.params.iter().map(render_param_inline).collect())
        .unwrap_or_default();

    // Public surface — `info.members` is the canonical visibility map,
    // and `info.methods` is what tells a callable member from a field.
    // Sorted so the same class always renders the same way regardless of
    // HashMap iteration order.
    let (mut field_names, mut method_names): (Vec<&String>, Vec<&String>) =
        (Vec::new(), Vec::new());
    for (member, is_private) in &info.members {
        if *is_private || member == "init" {
            continue;
        }
        if info.methods.contains_key(member) {
            method_names.push(member);
        } else {
            field_names.push(member);
        }
    }
    field_names.sort();
    method_names.sort();

    let fields: Vec<String> = field_names
        .iter()
        .map(|f| match info.field_types.get(*f) {
            Some(ty) => format!("{f}: {}", render_type(ty)),
            // No recorded type — inherited or otherwise unsourced. Still
            // surface the name so the hover doesn't lie about the API.
            None => (*f).clone(),
        })
        .collect();

    let methods: Vec<String> = method_names
        .iter()
        .filter_map(|m| info.methods.get(*m).map(|sig| render_member_method(m, sig)))
        .collect();

    let mut s = head;
    push_section(&mut s, "Params", &params);
    push_section(&mut s, "Fields", &fields);
    push_section(&mut s, "Functions", &methods);
    s.push_str("\n```");
    s
}

/// Append a labelled section to a class blurb, or nothing at all when
/// the list is empty — an empty "Fields" heading is a line spent saying
/// there is nothing to say.
///
/// A blank line separates one section from the last, but never the label
/// from its own entries: the gap belongs *between* the groups, and the
/// label reads as belonging to the list it sits on.
///
/// The label is written as a `--` comment so it renders muted instead of
/// coloured. Everything in the blurb is inside one `saule` fence, so the
/// editor's lexer has an opinion about every word in it — and the
/// highlighters map PascalCase to a type reference, which painted
/// `Params` / `Fields` / `Functions` the same colour as `Scene` and
/// `Color`. A label that reads as a class name is worse than no colour
/// at all; comment is the one token class that recedes.
fn push_section(s: &mut String, label: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    s.push_str("\n\n-- ");
    s.push_str(label);
    for item in items {
        s.push_str("\n  ");
        s.push_str(item);
    }
}

/// One entry in a class's `Functions:` list.
///
/// `-> nil` is omitted: the README defines it as the conventional way to
/// spell "this function returns nothing", so printing it adds a column
/// of noise to exactly the methods that have the least to say. Every
/// other return type is shown.
fn render_member_method(name: &str, sig: &MethodSig) -> String {
    let mut prefix = String::new();
    if sig.is_static {
        prefix.push_str("static ");
    }
    prefix.push_str("fn ");
    prefix.push_str(name);
    if !sig.type_params.is_empty() {
        prefix.push('<');
        prefix.push_str(&sig.type_params.join(", "));
        prefix.push('>');
    }

    let suffix = match &sig.return_ty {
        Some(rt) if !matches!(rt, Type::Named(n) if n == "nil") => {
            format!(" -> {}", render_type(rt))
        }
        _ => String::new(),
    };

    let params: Vec<String> = sig.params.iter().map(render_param_inline).collect();
    // Members sit two columns in, which is where the wrapped form has to
    // hang its continuation lines from.
    render_call_shape(&prefix, &params, &suffix, 2)
}

/// Hover for a stdlib static-class identifier (`Math`, `String`, …) or
/// a value type (`File`). Lists every member known to
/// `saule_typeck::sigs`, looking up signatures where available so the
/// reader sees `fn sqrt(number) -> float` instead of just `sqrt`.
pub(crate) fn render_stdlib_module(name: &str, kind: &str) -> String {
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

pub(crate) fn render_interface_head(
    name: &str,
    type_params: &[String],
    extends: &[TypeRef],
    methods: &[AstMethodSig],
) -> String {
    let mut s = format!(
        "```saule\ninterface {name}{}",
        render_type_params(type_params)
    );
    if !extends.is_empty() {
        let names: Vec<String> = extends.iter().map(render_type_ref).collect();
        s.push_str(" extends ");
        s.push_str(&names.join(", "));
    }
    if !methods.is_empty() {
        s.push_str(" {\n");
        for m in methods {
            let suffix = match &m.return_ty {
                Some(rt) => format!(" -> {}", render_type(rt)),
                None => String::new(),
            };
            let params: Vec<String> = m.params.iter().map(render_param_inline).collect();
            s.push_str("  ");
            s.push_str(&render_call_shape(
                &format!("fn {}", m.name),
                &params,
                &suffix,
                2,
            ));
            s.push('\n');
        }
        s.push('}');
    }
    s.push_str("\n```");
    s
}

pub(crate) fn render_interface_from_registry(name: &str, extends: &[String]) -> String {
    // The registry keeps only head names — type arguments are erased by the
    // time it is built — so each parent renders bare.
    let refs: Vec<TypeRef> = extends.iter().map(TypeRef::plain).collect();
    render_interface_head(
        name,
        &saule_semantic::interface_type_params(name),
        &refs,
        &[],
    )
}

/// Signature blurb for one `interface` method. Interface methods are
/// bodiless `MethodSig`s rather than `Method`s, so they need their own
/// small renderer to become a hover target in their own right.
pub(crate) fn render_interface_method(owner: &str, m: &AstMethodSig) -> String {
    let sig = MethodSig {
        is_static: false,
        is_private: false,
        type_params: Vec::new(),
        params: m.params.clone(),
        return_ty: m.return_ty.clone(),
    };
    render_method_sig(owner, &m.name, &sig)
}

pub(crate) fn render_enum_head(name: &str, variants: &[Spanned<EnumVariant>]) -> String {
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

pub(crate) fn render_enum_from_registry(name: &str, variants: &[(String, usize)]) -> String {
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

/// Declaration blurb for one enum variant, so a `---` comment written
/// above `North` has somewhere to surface.
pub(crate) fn render_enum_variant_decl(owner: &str, v: &EnumVariant) -> String {
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
/// Local copy of the type pretty-printer. Kept here (rather than reused
/// from `saule-semantic::return_check`) to avoid widening that crate's
/// public surface for what amounts to one display helper.
pub(crate) fn render_type(ty: &Type) -> String {
    match ty {
        Type::Named(n) => n.clone(),
        // A function under `?` needs parens: `fn() -> nil?` reads as a
        // function returning `nil?`, which is a different type from the
        // nullable function the annotation actually declares.
        Type::Nullable(inner) => match &**inner {
            Type::Function { .. } => format!("({})?", render_type(inner)),
            _ => format!("{}?", render_type(inner)),
        },
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
        Type::Generic(g) => {
            let a: Vec<_> = g.args.iter().map(render_type).collect();
            format!("{}<{}>", g.name, a.join(", "))
        }
    }
}

/// Pre-pass: walk every `enum` declaration in `module` and record the
/// payload-field shape of each tuple variant. Used by `bind_pattern` to
/// type the names introduced by `case Enum.Variant(a, b, ...)` patterns
/// without having to re-find the decl per arm.
pub(crate) fn collect_enum_variant_fields(
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
/// bare `Enum.Variant` when nothing declares a payload for it (a bare
/// or valued variant), and to bare arity when the enum is unknown.
pub(crate) fn render_variant_pattern(
    enum_name: &str,
    variant: &str,
    fields: &[Spanned<Pattern>],
    enum_fields: &HashMap<(String, String), Vec<Param>>,
) -> String {
    let mut s = format!("```saule\n(variant) {enum_name}.{variant}");
    if let Some(params) = declared_variant_fields(enum_name, variant, enum_fields) {
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

/// The declared payload of `Enum.Variant`, preferring the map collected
/// from this module and falling back to the semantic registry.
///
/// The map only holds enums declared in the file being hovered, so an
/// *imported* enum — the common case in a project of more than one
/// module — missed it and rendered as bare arity, `(_, _, _)`. The
/// registry is seeded with the imports and keeps whole `Param`s per
/// variant, so the names and types are already there. Reading them off
/// the seed also keeps this clear of the import graph, which a hover
/// must not re-walk.
pub(crate) fn declared_variant_fields(
    enum_name: &str,
    variant: &str,
    enum_fields: &HashMap<(String, String), Vec<Param>>,
) -> Option<Vec<Param>> {
    if let Some(params) = enum_fields.get(&(enum_name.to_string(), variant.to_string())) {
        return Some(params.clone());
    }
    let info = saule_semantic::with_enums(|r| r.get(enum_name).cloned())?;
    let shape = info.variants.get(variant)?;
    (shape.arity() > 0).then(|| shape.fields.clone())
}

// ─── doc comments ───────────────────────────────────────────────────────────
