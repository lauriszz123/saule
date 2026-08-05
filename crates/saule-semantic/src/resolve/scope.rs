//! Building the module-level scope an identifier resolves against.

use saule_ast::{Decl, ImportNames, Module, Stmt};
use std::collections::HashSet;

pub(crate) fn module_has_wildcard_import(module: &Module) -> bool {
    module.stmts.iter().any(|s| {
        matches!(
            &s.value,
            Stmt::Decl(d) if matches!(&d.value, Decl::Import { names: ImportNames::All, .. })
        )
    })
}

/// Pre-collect every name visible at module scope so forward references
/// (e.g. `fn a() b() end` then `fn b() end`) resolve cleanly.
pub(crate) fn collect_module_scope(module: &Module) -> HashSet<String> {
    let mut scope: HashSet<String> = HashSet::new();
    for s in &module.stmts {
        match &s.value {
            Stmt::Local { name, .. } => {
                scope.insert(name.clone());
            }
            Stmt::LocalMulti { names, .. } => {
                for (n, _, _) in names {
                    scope.insert(n.clone());
                }
            }
            Stmt::Decl(d) => match &d.value {
                Decl::Function { name, .. }
                | Decl::Class { name, .. }
                | Decl::Interface { name, .. }
                | Decl::Enum { name, .. }
                | Decl::Variable { name, .. } => {
                    scope.insert(name.clone());
                }
                Decl::Import { names, .. } => match names {
                    ImportNames::All => {}
                    ImportNames::List(items) => {
                        for (orig, alias) in items {
                            scope.insert(alias.clone().unwrap_or_else(|| orig.clone()));
                        }
                    }
                },
            },
            _ => {}
        }
    }
    scope
}
