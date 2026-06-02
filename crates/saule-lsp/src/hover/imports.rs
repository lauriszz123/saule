//! Helpers for resolving imports and rendering import-statement blurbs
//! shown on hover. Split out from `hover` so the walker module isn't
//! bloated with import-resolution mechanics.

use saule_ast::{Decl, ImportNames, Module, Stmt};

pub(super) fn aliases_for_file(imported: &Module, names: &ImportNames) -> Vec<(String, String)> {
    match names {
        ImportNames::All => imported
            .stmts
            .iter()
            .filter_map(|s| match &s.value {
                Stmt::Decl(d) => exported_name(&d.value).map(|n| (n.to_string(), n.to_string())),
                _ => None,
            })
            .collect(),
        ImportNames::List(items) => items
            .iter()
            .map(|(orig, alias)| (orig.clone(), alias.clone().unwrap_or_else(|| orig.clone())))
            .collect(),
    }
}

pub(super) fn aliases_for_native(exports: &[&'static str], names: &ImportNames) -> Vec<(String, String)> {
    match names {
        ImportNames::All => exports.iter().map(|n| ((*n).to_string(), (*n).to_string())).collect(),
        ImportNames::List(items) => items
            .iter()
            .map(|(orig, alias)| (orig.clone(), alias.clone().unwrap_or_else(|| orig.clone())))
            .collect(),
    }
}

pub(super) fn exported_name(decl: &Decl) -> Option<&str> {
    match decl {
        Decl::Function { name, .. }
        | Decl::Class { name, .. }
        | Decl::Interface { name, .. }
        | Decl::Enum { name, .. } => Some(name),
        Decl::Import { .. } => None,
    }
}

pub(super) fn render_native_import_blurb(pkg: &str, aliases: &[(String, String)]) -> String {
    let mut s = format!("```saule\n(native package) \"{pkg}\"");
    if !aliases.is_empty() {
        s.push_str("\n\nbrings into scope:\n");
        for (orig, alias) in aliases {
            s.push_str("  ");
            if alias == orig {
                s.push_str(orig);
            } else {
                s.push_str(orig);
                s.push_str(" as ");
                s.push_str(alias);
            }
            s.push('\n');
        }
    }
    s.push_str("```");
    s
}

pub(super) fn render_file_import_blurb(
    path_literal: &str,
    abs_path: &str,
    aliases: &[(String, String)],
) -> String {
    let mut s = format!("```saule\n(import) \"{path_literal}\"\n```\n\n`{abs_path}`");
    if !aliases.is_empty() {
        s.push_str("\n\n```saule\n");
        s.push_str("brings into scope:\n");
        for (orig, alias) in aliases {
            s.push_str("  ");
            if alias == orig {
                s.push_str(orig);
            } else {
                s.push_str(orig);
                s.push_str(" as ");
                s.push_str(alias);
            }
            s.push('\n');
        }
        s.push_str("```");
    }
    s
}

pub(super) fn render_unresolved_import(path: &str) -> String {
    format!("```saule\n(import) \"{path}\"  -- unresolved\n```")
}
