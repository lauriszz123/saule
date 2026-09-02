//! Whole-namespace listings: every class, interface, type or value
//! name a bare identifier position could mean.

use super::*;
use crate::server::sighelp::render_type;
use saule_ast::{Decl, Module, Stmt, Type};
use saule_semantic::registry::{
    ClassRegistry, interface_extends, is_subtype_named, lookup_field_type, lookup_method,
    with_classes, with_enums, with_interfaces,
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

/// A fixed set of keywords, in the order they were given — these lists are
/// short and already read in the order they'd be written.
pub(crate) fn keyword_items(keywords: &[&str], detail: &str) -> Vec<CompletionItem> {
    keywords
        .iter()
        .map(|kw| {
            item(
                (*kw).to_string(),
                CompletionItemKind::KEYWORD,
                Some(detail.to_string()),
            )
        })
        .collect()
}

/// What can follow `export`. Only the declaration keywords — the other thing
/// `export` accepts is a variable name, which the author is inventing.
pub(crate) fn export_items() -> Vec<CompletionItem> {
    keyword_items(EXPORT_KEYWORDS, "exported declaration")
}

/// What can begin a class member, in the order it is written: the modifiers
/// not yet given, then `fn`. A field is the remaining possibility, and its
/// name is the author's to invent.
pub(crate) fn class_member_items(is_static: bool, is_private: bool) -> Vec<CompletionItem> {
    let mut keywords = Vec::new();
    if !is_static {
        keywords.push("static");
    }
    if !is_private {
        keywords.push("local");
    }
    keywords.push("fn");
    keyword_items(&keywords, "class member")
}

/// The bare name a type ranks under: `Alignment`, `Alignment?` and
/// `Alignment<T>` all answer `Alignment`. Shapes with no single name — a
/// table, a tuple, a function type — answer `None` and simply don't rank.
pub(crate) fn base_type_name(ty: &Type) -> Option<String> {
    match ty {
        Type::Named(n) => Some(n.clone()),
        Type::Nullable(inner) => base_type_name(inner),
        Type::Generic(g) => Some(g.name.clone()),
        _ => None,
    }
}

/// Orders a value list by whether an item can actually go in the slot the
/// caret is filling.
///
/// Writing an argument, the type the parameter is declared to hold is a much
/// stronger signal than how closely a name matches the prefix: at
/// `alignment: ⟨caret⟩` an `Alignment` is the answer and a same-prefixed
/// method that returns something else is not, however similar it looks. So
/// items that fit the slot are ranked ahead of items that don't, and the
/// existing bucket orders each group within itself.
///
/// Outside an argument — and for a slot declared `any`, which everything
/// fits — there is no signal, and the order is exactly what it was.
pub(crate) struct Slot {
    want: Option<String>,
}

impl Slot {
    pub(crate) fn new(expected: Option<&Type>) -> Slot {
        let want = expected
            .and_then(base_type_name)
            .filter(|n| n != "any" && n != "nil");
        Slot { want }
    }

    /// The sort bucket for an item of type `have`, given the item's own
    /// category bucket. `None` means the item has no type to judge — a
    /// keyword, or a binding with no annotation to read.
    fn rank(&self, have: Option<&str>, bucket: &str) -> String {
        let fits = match (&self.want, have) {
            (Some(want), Some(have)) => is_subtype_named(have, want),
            _ => false,
        };
        format!("{}{bucket}", if fits { "0" } else { "1" })
    }

    /// [`Self::rank`] for an item whose type is a `Type`.
    fn rank_ty(&self, have: Option<&Type>, bucket: &str) -> String {
        self.rank(
            have.and_then(base_type_name).as_deref(),
            bucket,
        )
    }
}

/// Values usable at the caret: bindings in scope first, then the enclosing
/// class's own members, then module-level and imported names.
pub(crate) fn value_items(found: &Found, module: &Module, stmt_start: bool) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    // What the argument slot under the caret is declared to hold, if any.
    let slot = Slot::new(found.expected.as_ref());

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
            &slot.rank_ty(v.ty.as_ref(), "0"),
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
            &slot.rank(Some(class), "0"),
        ));
        for i in class_members(class, Visibility::IncludePrivate, MemberSet::All) {
            // A method ranks on what calling it yields, a field on what it
            // holds — either way, what the slot would end up receiving.
            let ty = lookup_method(class, &i.label)
                .and_then(|s| s.return_ty)
                .or_else(|| lookup_field_type(class, &i.label));
            let bucket = slot.rank_ty(ty.as_ref(), "1");
            items.push(sorted(i, &bucket));
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
                &slot.rank_ty(return_ty.as_ref(), "2"),
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
            // A class name is its constructor, so it yields an instance of
            // itself — and a subclass fits a slot declared as its base.
            items.push(sorted(
                item(n.clone(), CompletionItemKind::CLASS, Some(detail)),
                &slot.rank(Some(n), "3"),
            ));
        }
    });
    with_enums(|r| {
        for n in r.keys() {
            items.push(sorted(
                item(n.clone(), CompletionItemKind::ENUM, Some("enum".into())),
                &slot.rank(Some(n), "3"),
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
                &slot.rank(Some(name), "4"),
            ));
        }
    }

    for name in saule_interpreter::stdlib::all_prelude_names() {
        let detail = sigs::lookup(name)
            .map(|s| render_native_sig(name, &s))
            .unwrap_or_else(|| "prelude".into());
        items.push(sorted(
            item(name.to_string(), CompletionItemKind::FUNCTION, Some(detail)),
            &slot.rank_ty(
                sigs::lookup(name).and_then(|s| s.returns.first().cloned()).as_ref(),
                "4",
            ),
        ));
    }

    // Keywords only where a statement can begin — offering `end` or `then`
    // in the middle of an expression is just noise.
    if stmt_start {
        for kw in STATEMENT_KEYWORDS {
            items.push(sorted(
                item((*kw).to_string(), CompletionItemKind::KEYWORD, None),
                &slot.rank(None, "5"),
            ));
        }
    }

    dedup(items)
}

// ─── rendering ──────────────────────────────────────────────────────────────
