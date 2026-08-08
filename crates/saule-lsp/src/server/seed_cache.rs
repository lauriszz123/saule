//! Memoises [`collect_import_seed_with`] across requests.
//!
//! Every language feature — diagnostics, completion, hover, signature
//! help, inlay hints, highlight, goto — starts by rebuilding the import
//! seed so the semantic registries know what the current file's imports
//! declare. That walk reads and parses every reachable module, and it
//! dominates the request: measured against `examples/UI Project`, a
//! release build spends ~27ms in the seed and ~0.4ms in everything else
//! (lex, parse, and the analysis passes combined).
//!
//! Nine handlers pay that toll, several of them on the same keystroke —
//! Neovim fires `didChange`, completion, signature help and inlay hints
//! together, and they serialise behind the analysis lock. The visible
//! result was an editor that lagged a keystroke or more behind.
//!
//! The seed does not depend on the file being edited, only on
//!
//! 1. that file's `import` statements, and
//! 2. the contents of the modules they reach.
//!
//! So typing in `main.sau` cannot invalidate `main.sau`'s own seed
//! unless the edit changed an `import` line. (1) is compared on every
//! lookup; (2) is handled by recording which files the walk read and
//! dropping any entry that read a file the editor then changed.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use dashmap::DashMap;
use saule_ast::{Decl, ImportNames, Module, Stmt};

/// One memoised seed, plus what would make it stale.
pub(crate) struct CachedSeed {
    /// The `import` statements the seed was built from, rendered so two
    /// runs compare equal iff they'd walk the same graph.
    imports: Vec<String>,
    /// Every module the walk actually read. A change to any of them
    /// invalidates this entry.
    reads: HashSet<PathBuf>,
    seed: saule_semantic::ModuleSeed,
}

/// Keyed by the *importing* document's canonical path — each file has
/// its own import list and therefore its own seed.
#[derive(Default)]
pub(crate) struct SeedCache {
    entries: DashMap<PathBuf, CachedSeed>,
}

impl SeedCache {
    /// The seed for `module`, built fresh only when nothing usable is
    /// cached.
    ///
    /// `overlay` is the caller's open-buffer overlay. It is wrapped so
    /// that every path the walk asks about is recorded — the walk reads
    /// every module through it, which is what makes the read set exact
    /// without threading a second return value through the interpreter.
    pub(crate) fn seed_for(
        &self,
        doc_path: Option<&Path>,
        module: &Module,
        dir: &Path,
        overlay: impl Fn(&Path) -> Option<String>,
    ) -> saule_semantic::ModuleSeed {
        let imports = import_key(module);

        // No path to key on (an unsaved buffer with no file URI) — build
        // it and don't cache. Rare, and caching under a synthetic key
        // would be worse than not caching.
        let Some(doc_path) = doc_path else {
            return saule_interpreter::module::collect_import_seed_with(module, dir, &overlay);
        };

        if let Some(hit) = self.entries.get(doc_path)
            && hit.imports == imports
        {
            return hit.seed.clone();
        }

        let reads = std::cell::RefCell::new(HashSet::new());
        let recording = |path: &Path| {
            reads.borrow_mut().insert(path.to_path_buf());
            overlay(path)
        };
        let seed = saule_interpreter::module::collect_import_seed_with(module, dir, &recording);

        self.entries.insert(
            doc_path.to_path_buf(),
            CachedSeed {
                imports,
                reads: reads.into_inner(),
                seed: seed.clone(),
            },
        );
        seed
    }

    /// Drop every entry whose seed was built by reading `path`.
    ///
    /// Note what is *not* dropped: the entry keyed by `path` itself. A
    /// file's own text is not an input to its seed — only its import
    /// lines are, and those are compared on lookup. Dropping it here
    /// would defeat the whole cache, since the file being edited is the
    /// one that changes on every keystroke.
    pub(crate) fn invalidate_dependents_of(&self, path: &Path) {
        self.entries.retain(|_, v| !v.reads.contains(path));
    }

    /// Forget everything.
    ///
    /// Used when a file appears or disappears: a previously unresolvable
    /// `import` may now resolve, and the walk never read the missing
    /// file, so no read set mentions it and targeted invalidation cannot
    /// see the change.
    pub(crate) fn clear(&self) {
        self.entries.clear();
    }
}

/// Render a module's `import` statements into a comparable key.
///
/// Only the parts that steer the walk: the path, and the names bound
/// (with aliases, which decide what the seed is keyed under). Spans and
/// everything else in the file are deliberately absent — that's what
/// lets an edit anywhere else in the file reuse the cached seed.
fn import_key(module: &Module) -> Vec<String> {
    let mut out = Vec::new();
    for stmt in &module.stmts {
        let Stmt::Decl(d) = &stmt.value else { continue };
        let Decl::Import { names, path, .. } = &d.value else {
            continue;
        };
        let names = match names {
            ImportNames::All => "*".to_string(),
            ImportNames::List(items) => items
                .iter()
                .map(|(orig, alias)| match alias {
                    Some(a) => format!("{orig} as {a}"),
                    None => orig.clone(),
                })
                .collect::<Vec<_>>()
                .join(","),
        };
        out.push(format!("{names}\u{0}{path}"));
    }
    out
}
