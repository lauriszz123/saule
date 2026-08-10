//! Turning a resolved name into a `CompletionItem`: signature
//! strings, doc extraction, and the sort / filter / dedup passes
//! applied to the finished list.

use crate::server::sighelp::render_type;
use saule_ast::{Param, Type};
use saule_semantic::MethodSig;
use saule_typeck::sigs::NativeSig;
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, Documentation, MarkupContent, MarkupKind,
};

pub(crate) fn render_native_sig(name: &str, sig: &NativeSig) -> String {
    let mut params: Vec<String> = sig.params.iter().map(render_type).collect();
    if let Some(v) = &sig.variadic {
        params.push(format!("...{}", render_type(v)));
    }
    let ret = if sig.returns.is_empty() {
        "nil".to_string()
    } else {
        sig.returns
            .iter()
            .map(render_type)
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!("fn {name}({}) -> {ret}", params.join(", "))
}

pub(crate) fn render_method_sig(name: &str, sig: &MethodSig) -> String {
    let params: Vec<String> = sig
        .params
        .iter()
        .map(|p| format!("{}: {}", p.name, render_type(&p.ty)))
        .collect();
    let ret = sig
        .return_ty
        .as_ref()
        .map(render_type)
        .unwrap_or_else(|| "nil".into());
    let kw = if sig.is_static { "static fn" } else { "fn" };
    format!("{kw} {name}({}) -> {ret}", params.join(", "))
}

pub(crate) fn render_fn_sig(name: &str, params: &[Param], ret: Option<&Type>) -> String {
    let ps: Vec<String> = params
        .iter()
        .map(|p| format!("{}: {}", p.name, render_type(&p.ty)))
        .collect();
    let r = ret.map(render_type).unwrap_or_else(|| "nil".into());
    format!("fn {name}({}) -> {r}", ps.join(", "))
}

// ─── item plumbing ──────────────────────────────────────────────────────────

pub(crate) fn item(
    label: String,
    kind: CompletionItemKind,
    detail: Option<String>,
) -> CompletionItem {
    CompletionItem {
        sort_text: Some(format!("1{label}")),
        label,
        kind: Some(kind),
        detail,
        ..Default::default()
    }
}

pub(crate) fn sorted(mut i: CompletionItem, bucket: &str) -> CompletionItem {
    i.sort_text = Some(format!("{bucket}{}", i.label));
    i
}

pub(crate) fn doc_of(mut i: CompletionItem, doc: Option<String>) -> CompletionItem {
    if let Some(d) = doc {
        i.documentation = Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: d,
        }));
    }
    i
}

/// The client filters too, but trimming here keeps the payload small.
pub(crate) fn filter(items: Vec<CompletionItem>, prefix: &str) -> Vec<CompletionItem> {
    if prefix.is_empty() {
        return items;
    }
    let lower = prefix.to_lowercase();
    items
        .into_iter()
        .filter(|i| i.label.to_lowercase().starts_with(&lower))
        .collect()
}

pub(crate) fn dedup(mut items: Vec<CompletionItem>) -> Vec<CompletionItem> {
    let mut seen: Vec<String> = Vec::new();
    items.retain(|i| {
        if seen.contains(&i.label) {
            false
        } else {
            seen.push(i.label.clone());
            true
        }
    });
    items
}

/// The type names that stand on their own. `function` is deliberately absent:
/// a function's type is its signature, written `fn(...) -> T`.
pub(crate) const PRIMITIVES: &[&str] = &[
    "integer", "float", "string", "boolean", "table", "userdata", "thread", "any", "nil",
];

/// Keywords that can begin a statement.
pub(crate) const STATEMENT_KEYWORDS: &[&str] = &[
    "local",
    "if",
    "for",
    "while",
    "repeat",
    "return",
    "break",
    "continue",
    "try",
    "throw",
    "fn",
    "class",
    "interface",
    "enum",
    "export",
    "import",
    "match",
    "do",
];
