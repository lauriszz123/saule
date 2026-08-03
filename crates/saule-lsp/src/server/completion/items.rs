//! Building the completion items themselves: members, classes,
//! interfaces, types, values, enum variants and import names, plus
//! the signature rendering and the sort/filter/dedup passes.

mod catalog;
mod render;

pub(crate) use catalog::*;
pub(crate) use render::*;

use crate::server::sighelp::render_type;
use saule_ast::{Decl, Expr, Spanned, Stmt};
use saule_semantic::registry::{lookup_field_type, lookup_method, with_classes, with_enums};
use saule_typeck::sigs::{self};
use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind};

use super::*;

pub(crate) fn member_items(recv: &Spanned<Expr>, found: &Found) -> Vec<CompletionItem> {
    match infer(&recv.value, found) {
        Some(Recv::SelfClass(c)) => class_members(&c, Visibility::IncludePrivate, MemberSet::All),
        Some(Recv::Instance(c)) => class_members(&c, Visibility::PublicOnly, MemberSet::Instance),
        Some(Recv::Static(c)) => class_members(&c, Visibility::PublicOnly, MemberSet::Static),
        Some(Recv::Module(m)) => module_members(&m),
        Some(Recv::Enum(e)) => enum_variants(&e),
        None => Vec::new(),
    }
}

#[derive(PartialEq)]
pub(crate) enum Visibility {
    PublicOnly,
    IncludePrivate,
}

#[derive(PartialEq)]
pub(crate) enum MemberSet {
    All,
    Instance,
    Static,
}

/// Members of `class` and its ancestors, filtered by visibility and by
/// whether the access was through an instance or the class itself.
pub(crate) fn class_members(class: &str, vis: Visibility, set: MemberSet) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut current = Some(class.to_string());

    while let Some(cls) = current {
        let (members, parent) = with_classes(|r| {
            r.get(&cls)
                .map(|i| (i.members.clone(), i.parent.clone()))
                .unwrap_or_default()
        });

        for (name, is_private) in members {
            if is_private && vis == Visibility::PublicOnly {
                continue;
            }
            if seen.contains(&name) {
                continue;
            }

            let method = lookup_method(&cls, &name);
            // `init` is called as `Player(...)`, never as a member.
            if name == "init" {
                continue;
            }
            if let Some(sig) = &method {
                let wanted = match set {
                    MemberSet::All => true,
                    MemberSet::Instance => !sig.is_static,
                    MemberSet::Static => sig.is_static,
                };
                if !wanted {
                    continue;
                }
            }
            seen.push(name.clone());

            let detail = match &method {
                Some(sig) => render_method_sig(&name, sig),
                None => lookup_field_type(&cls, &name)
                    .as_ref()
                    .map(render_type)
                    .unwrap_or_else(|| "field".into()),
            };
            let kind = if method.is_some() {
                CompletionItemKind::METHOD
            } else {
                CompletionItemKind::FIELD
            };
            let doc = (cls != class).then(|| format!("inherited from `{cls}`"));
            items.push(doc_of(item(name, kind, Some(detail)), doc));
        }
        current = parent;
    }

    items.sort_by(|a, b| a.label.cmp(&b.label));
    items
}

pub(crate) fn module_members(module: &str) -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = sigs::module_members(module)
        .into_iter()
        .map(|m| {
            let sig = sigs::lookup(&format!("{module}.{m}"));
            let kind = if sig.is_some() {
                CompletionItemKind::METHOD
            } else {
                CompletionItemKind::FIELD
            };
            let detail = sig.map(|s| render_native_sig(&m, &s));
            item(m, kind, detail)
        })
        .collect();
    items.sort_by(|a, b| a.label.cmp(&b.label));
    items
}

pub(crate) fn enum_variants(name: &str) -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = with_enums(|r| {
        r.get(name)
            .map(|e| {
                e.variants
                    .keys()
                    .map(|v| {
                        item(
                            v.clone(),
                            CompletionItemKind::ENUM_MEMBER,
                            Some(format!("{name}.{v}")),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    });
    items.sort_by(|a, b| a.label.cmp(&b.label));
    items
}

/// The names a given module actually exports — so `import <caret> from x`
/// offers exactly what `x` provides, and nothing else.
pub(crate) fn import_name_items(path: &str, dir: Option<&std::path::Path>) -> Vec<CompletionItem> {
    // Native packages describe their surface directly.
    if let Some(pkg) = saule_interpreter::native_packages::lookup(path) {
        return pkg
            .exports
            .iter()
            .map(|n| {
                item(
                    (*n).to_string(),
                    CompletionItemKind::CLASS,
                    Some("native package export".into()),
                )
            })
            .collect();
    }
    if saule_interpreter::dynamic_packages::is_dynamic_package(path) {
        return saule_interpreter::dynamic_packages::export_names(path)
            .into_iter()
            .map(|n| {
                item(
                    n,
                    CompletionItemKind::CLASS,
                    Some("native package class".into()),
                )
            })
            .collect();
    }

    let Some(dir) = dir else { return Vec::new() };
    let Some(abs) = saule_interpreter::module::resolve_import_path(dir, path) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    let mut seen = Vec::new();
    collect_exports(&abs, 0, &mut out, &mut seen);
    out.sort_by(|a, b| a.label.cmp(&b.label));
    out
}

/// Exported declarations of the module at `abs`. An `init.sau` barrel
/// re-exports whatever it imports, so those targets are followed too.
pub(crate) fn collect_exports(
    abs: &std::path::Path,
    depth: usize,
    out: &mut Vec<CompletionItem>,
    seen: &mut Vec<String>,
) {
    if depth > 4 || seen.iter().any(|s| s == &abs.display().to_string()) {
        return;
    }
    seen.push(abs.display().to_string());

    let Ok(src) = std::fs::read_to_string(abs) else {
        return;
    };
    let Some(module) = parse(&src) else { return };

    for stmt in &module.stmts {
        let Stmt::Decl(d) = &stmt.value else { continue };
        let (name, kind) = match &d.value {
            Decl::Function {
                exported: true,
                name,
                params,
                return_ty,
                ..
            } => {
                out.push(item(
                    name.clone(),
                    CompletionItemKind::FUNCTION,
                    Some(render_fn_sig(name, params, return_ty.as_ref())),
                ));
                continue;
            }
            Decl::Class {
                exported: true,
                name,
                ..
            } => (name, CompletionItemKind::CLASS),
            Decl::Interface {
                exported: true,
                name,
                ..
            } => (name, CompletionItemKind::INTERFACE),
            Decl::Enum {
                exported: true,
                name,
                ..
            } => (name, CompletionItemKind::ENUM),
            // A barrel publishes what it imports; follow those.
            Decl::Import { path, .. } => {
                if abs.file_stem().and_then(|s| s.to_str()) == Some("init")
                    && let Some(parent) = abs.parent()
                    && let Some(next) = saule_interpreter::module::resolve_import_path(parent, path)
                {
                    collect_exports(&next, depth + 1, out, seen);
                }
                continue;
            }
            _ => continue,
        };
        out.push(item(
            name.clone(),
            kind,
            Some(format!("{kind:?}").to_lowercase()),
        ));
    }
}
