//! Whole-namespace listings: every class, interface, type or value
//! name a bare identifier position could mean.

use super::*;
use crate::server::sighelp::render_type;
use saule_ast::{Decl, Module, Stmt};
use saule_semantic::registry::{
    ClassRegistry, interface_extends, lookup_method, with_classes, with_enums, with_interfaces,
};
use saule_typeck::sigs::{self};
use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind};

/// Classes usable as a base class. A class can't extend itself, and can't
/// extend one of its own descendants either — that would close a cycle in the
/// chain — so both are left out.
pub(crate) fn class_items(exclude: &[String]) -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = with_classes(|r| {
        r.iter()
            .filter(|(name, _)| !exclude.iter().any(|e| descends_from(r, name, e)))
            .map(|(name, info)| {
                let detail = match &info.parent {
                    Some(p) => format!("class extends {p}"),
                    None => "class".to_string(),
                };
                item(name.clone(), CompletionItemKind::CLASS, Some(detail))
            })
            .collect()
    });
    items.sort_by(|a, b| a.label.cmp(&b.label));
    items
}

/// Is `class` `ancestor`, or one of its descendants?
pub(crate) fn descends_from(reg: &ClassRegistry, class: &str, ancestor: &str) -> bool {
    let mut cur = Some(class.to_string());
    let mut hops = 0;
    while let Some(name) = cur {
        if name == ancestor {
            return true;
        }
        // A pre-existing cycle in the registry would otherwise spin forever.
        hops += 1;
        if hops > MAX_INHERITANCE_DEPTH {
            return false;
        }
        cur = reg.get(&name).and_then(|i| i.parent.clone());
    }
    false
}

pub(crate) const MAX_INHERITANCE_DEPTH: usize = 64;

/// Interfaces, minus the ones already named in the header — and, for
/// `interface X extends`, minus anything that already extends `X`.
pub(crate) fn interface_items(exclude: &[String]) -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = with_interfaces(|r| {
        r.iter()
            .filter(|(name, _)| !exclude.iter().any(|e| interface_extends(name, e)))
            .map(|(name, extends)| {
                let detail = if extends.is_empty() {
                    "interface".to_string()
                } else {
                    format!("interface extends {}", extends.join(", "))
                };
                item(name.clone(), CompletionItemKind::INTERFACE, Some(detail))
            })
            .collect()
    });
    items.sort_by(|a, b| a.label.cmp(&b.label));
    items
}

/// Only type names — never values or keywords.
pub(crate) fn type_items() -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = PRIMITIVES
        .iter()
        .map(|t| {
            item(
                (*t).to_string(),
                CompletionItemKind::KEYWORD,
                Some("primitive".into()),
            )
        })
        .collect();

    with_classes(|r| {
        for n in r.keys() {
            items.push(item(
                n.clone(),
                CompletionItemKind::CLASS,
                Some("class".into()),
            ));
        }
    });
    with_interfaces(|r| {
        for n in r.keys() {
            items.push(item(
                n.clone(),
                CompletionItemKind::INTERFACE,
                Some("interface".into()),
            ));
        }
    });
    with_enums(|r| {
        for n in r.keys() {
            items.push(item(
                n.clone(),
                CompletionItemKind::ENUM,
                Some("enum".into()),
            ));
        }
    });
    items
}

/// Values usable at the caret: bindings in scope first, then the enclosing
/// class's own members, then module-level and imported names.
pub(crate) fn value_items(found: &Found, module: &Module, stmt_start: bool) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    // Argument keywords for the call the caret is inside, ahead of
    // everything else — at `Widget(back…)` the author is far more likely
    // to be naming the `background:` parameter than reaching for a local
    // that happens to start the same way.
    for p in &found.named_params {
        items.push(sorted(
            item(
                format!("{}:", p.name),
                CompletionItemKind::FIELD,
                Some(format!("parameter: {}", render_type(&p.ty))),
            ),
            "!",
        ));
    }

    // Innermost bindings win, so walk the stack in reverse and skip shadowed
    // names.
    let mut seen: Vec<String> = Vec::new();
    for v in found.scope.iter().rev() {
        if seen.contains(&v.name) {
            continue;
        }
        seen.push(v.name.clone());
        let detail =
            v.ty.as_ref()
                .map(|t| format!("{}: {}", v.kind, render_type(t)))
                .unwrap_or_else(|| v.kind.to_string());
        items.push(sorted(
            item(v.name.clone(), CompletionItemKind::VARIABLE, Some(detail)),
            "0",
        ));
    }

    // Inside a method every class member is reachable by bare name.
    if let Some(class) = &found.class {
        items.push(sorted(
            item(
                "self".into(),
                CompletionItemKind::KEYWORD,
                Some(format!("the current {class}")),
            ),
            "0",
        ));
        for i in class_members(class, Visibility::IncludePrivate, MemberSet::All) {
            items.push(sorted(i, "1"));
        }
    }

    // Module-level functions declared in this file.
    for stmt in &module.stmts {
        if let Stmt::Decl(d) = &stmt.value
            && let Decl::Function {
                name,
                params,
                return_ty,
                ..
            } = &d.value
            && name != SENTINEL
        {
            items.push(sorted(
                item(
                    name.clone(),
                    CompletionItemKind::FUNCTION,
                    Some(render_fn_sig(name, params, return_ty.as_ref())),
                ),
                "2",
            ));
        }
    }

    // Classes and enums in scope (this file plus imports, courtesy of the
    // seeded registries). A class name doubles as its constructor.
    with_classes(|r| {
        for n in r.keys() {
            let detail = lookup_method(n, "init")
                .map(|s| render_method_sig(n, &s))
                .unwrap_or_else(|| "class".into());
            items.push(sorted(
                item(n.clone(), CompletionItemKind::CLASS, Some(detail)),
                "3",
            ));
        }
    });
    with_enums(|r| {
        for n in r.keys() {
            items.push(sorted(
                item(n.clone(), CompletionItemKind::ENUM, Some("enum".into())),
                "3",
            ));
        }
    });

    // Names the auto-prelude packages contribute (`Math`, `Io`, `IoMode`, …).
    // Taken from the packages themselves rather than a hardcoded list, so this
    // can't drift as the stdlib grows.
    for pkg in saule_interpreter::native_packages::all() {
        if !pkg.auto_prelude {
            continue;
        }
        for name in pkg.exports {
            items.push(sorted(
                item(
                    (*name).to_string(),
                    CompletionItemKind::MODULE,
                    Some("stdlib".into()),
                ),
                "4",
            ));
        }
    }

    for name in saule_interpreter::stdlib::all_prelude_names() {
        let detail = sigs::lookup(name)
            .map(|s| render_native_sig(name, &s))
            .unwrap_or_else(|| "prelude".into());
        items.push(sorted(
            item(name.to_string(), CompletionItemKind::FUNCTION, Some(detail)),
            "4",
        ));
    }

    // Keywords only where a statement can begin — offering `end` or `then`
    // in the middle of an expression is just noise.
    if stmt_start {
        for kw in STATEMENT_KEYWORDS {
            items.push(sorted(
                item((*kw).to_string(), CompletionItemKind::KEYWORD, None),
                "5",
            ));
        }
    }

    dedup(items)
}

// ─── rendering ──────────────────────────────────────────────────────────────
