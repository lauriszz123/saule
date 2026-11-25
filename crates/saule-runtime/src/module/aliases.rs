//! An imported module as its importers must read it: with every `type`
//! alias written out.
//!
//! A class's field says `onTap: Handler`, and `Handler` is an alias of the
//! module that declares the class — perhaps a private one. A module that
//! imports the class has no `Handler`, or has another. So what an import
//! brings in is read from a tree in which the alias has already been
//! replaced by what it stands for *there*, and no name of the imported
//! module's leaks into the importing one.
//!
//! Writing a module's aliases out needs the aliases *its* imports bring in,
//! so this walks the import graph, each module after the ones it imports.
//! [`resolved_module`] is the one entry; it answers from a table kept for
//! the length of one seed collection, so a module reached along many paths
//! is worked out once.

use saule_ast::{Decl, ImportNames, Module, Stmt};
use saule_semantic::AliasRegistry;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::resolve_import_path;
use super::seed::{SeedIo, local_names, parsed_module};

/// A module with its aliases written out, and the aliases it exports.
pub(super) struct Resolved {
    /// The tree with no alias left in it. The very tree the parser made
    /// when the module has no alias in scope, which is most modules.
    pub(super) tree: Arc<Module>,
    /// What `export type` publishes, and what `export from … import …`
    /// passes on, each already written out.
    pub(super) exported: AliasRegistry,
}

/// One `import` of a Saule module, with the file it leads to.
struct Edge {
    names: ImportNames,
    exported: bool,
    target: PathBuf,
}

/// A module's imports, and the tree they were read from.
type EdgeEntry = (Arc<Module>, Arc<Vec<Edge>>);

/// A tree written out before, with what it was written out against.
struct Kept {
    parsed: Arc<Module>,
    imported: AliasRegistry,
    tree: Arc<Module>,
}

/// One seed collection's worth of answers.
#[derive(Default)]
struct Pass {
    /// How many [`PassGuard`]s are alive. The table lasts while one is.
    depth: usize,
    done: HashMap<PathBuf, Option<Arc<Resolved>>>,
    /// Modules being resolved now, to stop at an import cycle.
    in_flight: HashSet<PathBuf>,
}

thread_local! {
    static PASS: RefCell<Pass> = RefCell::new(Pass::default());

    /// Each module's imports, with the tree they were read from. Finding
    /// the file an `import` names means asking the file system, and every
    /// seed collection walks the same modules.
    static EDGES: RefCell<HashMap<PathBuf, EdgeEntry>> =
        RefCell::new(HashMap::new());

    /// The last tree written out for each module. Kept across collections
    /// so that an unchanged module is the *same* tree each time — which is
    /// what the tables keyed on a tree's identity (`DECLARED`) rely on —
    /// and so that it is copied once, not once per importer.
    static KEPT: RefCell<HashMap<PathBuf, Kept>> = RefCell::new(HashMap::new());
}

/// Keeps the table of answers alive. Taken by whatever starts a seed
/// collection; the table is emptied when the last one goes.
pub(super) struct PassGuard;

impl PassGuard {
    pub(super) fn enter() -> PassGuard {
        PASS.with(|p| p.borrow_mut().depth += 1);
        PassGuard
    }
}

impl Drop for PassGuard {
    fn drop(&mut self) {
        PASS.with(|p| {
            let mut pass = p.borrow_mut();
            pass.depth -= 1;
            if pass.depth == 0 {
                pass.done.clear();
                pass.in_flight.clear();
            }
        });
    }
}

/// The module at `abs`, with its aliases written out.
///
/// `None` when it cannot be read or parsed, as with [`parsed_module`].
pub(super) fn resolved_module(abs: &Path, io: SeedIo<'_>) -> Option<Arc<Resolved>> {
    let _pass = PassGuard::enter();
    if let Some(answer) = PASS.with(|p| p.borrow().done.get(abs).cloned()) {
        return answer;
    }
    let parsed = parsed_module(abs, io);
    let Some(parsed) = parsed else {
        PASS.with(|p| p.borrow_mut().done.insert(abs.to_path_buf(), None));
        return None;
    };

    // Two modules that import each other: the one met again is read with
    // the aliases it declares itself, and no further. Not recorded — the
    // full answer is still on its way.
    if !PASS.with(|p| p.borrow_mut().in_flight.insert(abs.to_path_buf())) {
        return Some(Arc::new(write_out(
            abs,
            parsed,
            AliasRegistry::new(),
            AliasRegistry::new(),
        )));
    }

    let mut imported = AliasRegistry::new();
    let mut passed_on = AliasRegistry::new();
    for edge in edges(abs, &parsed).iter() {
        let Some(target) = resolved_module(&edge.target, io) else {
            continue;
        };
        for (name, info) in &target.exported {
            for local in local_names(&edge.names, name) {
                if edge.exported {
                    passed_on
                        .entry(local.clone())
                        .or_insert_with(|| info.clone());
                }
                // The first import to bind a name keeps it, as everywhere.
                imported.entry(local).or_insert_with(|| info.clone());
            }
        }
    }

    let resolved = Arc::new(write_out(abs, parsed, imported, passed_on));
    PASS.with(|p| {
        let mut pass = p.borrow_mut();
        pass.in_flight.remove(abs);
        pass.done.insert(abs.to_path_buf(), Some(resolved.clone()));
    });
    Some(resolved)
}

/// `parsed` with its aliases written out against `imported`, and what it
/// exports: its own `export type`s, then `passed_on`.
fn write_out(
    abs: &Path,
    parsed: Arc<Module>,
    imported: AliasRegistry,
    passed_on: AliasRegistry,
) -> Resolved {
    // Errors are the module's own to report, when it is checked itself.
    let (scope, _) = saule_semantic::alias_scope(&parsed, &imported);
    let mut exported = saule_semantic::exported_aliases(&parsed, &scope);
    for (name, info) in passed_on {
        exported.entry(name).or_insert(info);
    }
    if scope.is_empty() {
        return Resolved {
            tree: parsed,
            exported,
        };
    }

    let kept = KEPT.with(|k| {
        k.borrow()
            .get(abs)
            .filter(|kept| Arc::ptr_eq(&kept.parsed, &parsed) && kept.imported == imported)
            .map(|kept| kept.tree.clone())
    });
    let tree = kept.unwrap_or_else(|| {
        let mut copy = (*parsed).clone();
        saule_semantic::expand_aliases(&mut copy, &scope);
        let tree = Arc::new(copy);
        KEPT.with(|k| {
            k.borrow_mut().insert(
                abs.to_path_buf(),
                Kept {
                    parsed,
                    imported,
                    tree: tree.clone(),
                },
            )
        });
        tree
    });
    Resolved { tree, exported }
}

/// The Saule modules `parsed` imports from, in the order written. Native
/// packages are left out: they declare no aliases.
fn edges(abs: &Path, parsed: &Arc<Module>) -> Arc<Vec<Edge>> {
    let cached = EDGES.with(|c| {
        c.borrow()
            .get(abs)
            .filter(|(tree, _)| Arc::ptr_eq(tree, parsed))
            .map(|(_, edges)| edges.clone())
    });
    if let Some(edges) = cached {
        return edges;
    }

    let dir = abs.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut out = Vec::new();
    for stmt in &parsed.stmts {
        let Stmt::Decl(d) = &stmt.value else { continue };
        let Decl::Import {
            names,
            path,
            exported,
            ..
        } = &d.value
        else {
            continue;
        };
        if crate::native_packages::lookup(path).is_some()
            || crate::dynamic_packages::is_dynamic_package(path)
        {
            continue;
        }
        if let Some(target) = resolve_import_path(&dir, path) {
            out.push(Edge {
                names: names.clone(),
                exported: *exported,
                target,
            });
        }
    }
    let edges = Arc::new(out);
    EDGES.with(|c| {
        c.borrow_mut()
            .insert(abs.to_path_buf(), (parsed.clone(), edges.clone()))
    });
    edges
}
