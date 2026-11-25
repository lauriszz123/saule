//! Building the module-level scope an identifier resolves against.

use saule_ast::{Decl, ImportNames, Module, Stmt};
use std::collections::HashSet;
use std::rc::Rc;

pub(crate) fn module_has_wildcard_import(module: &Module) -> bool {
    module.stmts.iter().any(|s| {
        matches!(
            &s.value,
            Stmt::Decl(d) if matches!(
                &d.value,
                Decl::Import { names: ImportNames::All | ImportNames::Namespace(_), .. }
            )
        )
    })
}

/// Pre-collect every name visible at module scope so forward references
/// (e.g. `fn a() b() end` then `fn b() end`) resolve cleanly.
///
/// Returns them **in declaration order**, because the index is the module
/// slot the compiler will emit `GETMOD`/`SETMOD` against. A `HashSet` would
/// have done for the "is this name defined?" question this used to answer
/// alone, but its iteration order varies with the hash seed, and slot
/// numbers that move between runs would break a bytecode cache and make
/// every disassembly diff.
pub(crate) fn collect_module_scope(module: &Module) -> Vec<Rc<str>> {
    let mut order: Vec<Rc<str>> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut push = |name: &str, order: &mut Vec<Rc<str>>| {
        if seen.insert(name.to_string()) {
            order.push(Rc::from(name));
        }
    };
    for s in &module.stmts {
        match &s.value {
            Stmt::Local { name, .. } => push(name, &mut order),
            Stmt::LocalMulti { names, .. } => {
                for (n, _, _) in names {
                    push(n, &mut order);
                }
            }
            Stmt::Decl(d) => match &d.value {
                Decl::Function { name, .. }
                | Decl::Class { name, .. }
                | Decl::Interface { name, .. }
                | Decl::Enum { name, .. }
                | Decl::Variable { name, .. } => push(name, &mut order),
                // No slot: a `type` alias is not a value, and nothing by
                // its name exists when the program runs.
                Decl::TypeAlias { .. } => {}
                Decl::Import { names, .. } => match names {
                    ImportNames::All | ImportNames::Namespace(_) => {}
                    ImportNames::List(items) => {
                        for (orig, alias) in items {
                            push(alias.as_deref().unwrap_or(orig), &mut order);
                        }
                    }
                },
            },
            _ => {}
        }
    }
    order
}

/// The aliases in scope whose names are *only* types here: every alias but
/// those sharing a name with something the module itself declares as a
/// value. Naming one of them in an expression is a mistake worth its own
/// message — see [`SemanticError::AliasAsValue`](crate::SemanticError).
///
/// An imported alias does have a module slot, since an `import` list cannot
/// tell what kind of thing each name is; this set is what tells them apart.
pub(crate) fn type_only_names(module: &Module) -> HashSet<String> {
    let mut names: HashSet<String> =
        crate::aliases::with_aliases(|scope| scope.keys().cloned().collect());
    if names.is_empty() {
        return names;
    }
    for s in &module.stmts {
        match &s.value {
            Stmt::Local { name, .. } => {
                names.remove(name);
            }
            Stmt::LocalMulti { names: locals, .. } => {
                for (n, _, _) in locals {
                    names.remove(n);
                }
            }
            Stmt::Decl(d) => match &d.value {
                Decl::Function { name, .. }
                | Decl::Class { name, .. }
                | Decl::Interface { name, .. }
                | Decl::Enum { name, .. }
                | Decl::Variable { name, .. } => {
                    names.remove(name);
                }
                Decl::TypeAlias { .. } | Decl::Import { .. } => {}
            },
            _ => {}
        }
    }
    names
}
