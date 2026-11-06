//! Helpers for resolving imports and rendering import-statement blurbs
//! shown on hover. Split out from `hover` so the walker module isn't
//! bloated with import-resolution mechanics.

use saule_ast::{Decl, ImportNames, Module, Stmt};

pub(super) fn aliases_for_file(imported: &Module, names: &ImportNames) -> Vec<(String, String)> {
    let exports: Vec<&str> = imported
        .stmts
        .iter()
        .filter_map(|s| match &s.value {
            Stmt::Decl(d) => exported_name(&d.value),
            _ => None,
        })
        .collect();
    names.bind(exports)
}

pub(super) fn aliases_for_native(
    exports: &[&'static str],
    names: &ImportNames,
) -> Vec<(String, String)> {
    names.bind(exports.iter().copied())
}

/// Like [`aliases_for_native`] but for dynamic (manifest-described)
/// packages, whose export names are owned `String`s read from disk rather
/// than `&'static str` baked into the binary.
pub(super) fn aliases_for_dynamic(
    exports: &[String],
    names: &ImportNames,
) -> Vec<(String, String)> {
    names.bind(exports.iter().map(String::as_str))
}

/// The name a declaration contributes to `import *`, or `None` when it
/// stays private to its module.
///
/// This must agree with the module loader's own `exported_name`
/// ([`saule_runtime::module`]) — that function is what actually
/// decides which bindings a wildcard import produces. Matching on the
/// name alone made the hover blurb advertise private declarations that
/// then failed to resolve at the use site.
pub(super) fn exported_name(decl: &Decl) -> Option<&str> {
    match decl {
        Decl::Function {
            exported: true,
            name,
            ..
        }
        | Decl::Class {
            exported: true,
            name,
            ..
        }
        | Decl::Interface {
            exported: true,
            name,
            ..
        }
        | Decl::Enum {
            exported: true,
            name,
            ..
        } => Some(name),
        _ => None,
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
