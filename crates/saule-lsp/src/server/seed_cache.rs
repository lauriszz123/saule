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

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway workspace: `lib.sau` plus a `main.sau` that imports it.
    struct Fixture {
        dir: PathBuf,
        main: PathBuf,
        lib: PathBuf,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("saule-seed-cache-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create temp dir");
            let main = dir.join("main.sau");
            let lib = dir.join("lib.sau");
            std::fs::write(&main, "import * from \"lib\"\n").expect("write main");
            std::fs::write(&lib, "export class Alpha\nend\n").expect("write lib");
            // The handlers all derive the module dir by canonicalising the
            // document path — mirror that exactly, since the whole question
            // is whether the recorded read paths and the invalidation key
            // are spelled the same way.
            let dir = std::fs::canonicalize(&dir).expect("canonicalize");
            Self {
                main: dir.join("main.sau"),
                lib: dir.join("lib.sau"),
                dir,
            }
        }

        fn seed(&self, cache: &SeedCache) -> saule_semantic::ModuleSeed {
            let source = std::fs::read_to_string(&self.main).expect("read main");
            let tokens = saule_lexer::Lexer::new(&source).tokenize().expect("lex");
            let module = saule_parser::parse(tokens).expect("parse");
            cache.seed_for(Some(&self.main), &module, &self.dir, |_| None)
        }

        fn write_lib(&self, text: &str) {
            std::fs::write(&self.lib, text).expect("write lib");
        }

        fn write_main(&self, text: &str) {
            std::fs::write(&self.main, text).expect("write main");
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// The point of the cache: a second lookup does not re-read the
    /// import graph. Proven by changing the imported file behind its
    /// back — a cache miss would pick the change up.
    #[test]
    fn a_second_lookup_reuses_the_cached_seed() {
        let fx = Fixture::new("reuse");
        let cache = SeedCache::default();
        assert!(fx.seed(&cache).classes.contains_key("Alpha"));

        fx.write_lib("export class Beta\nend\n");
        let again = fx.seed(&cache);
        assert!(again.classes.contains_key("Alpha"), "expected a cache hit");
        assert!(!again.classes.contains_key("Beta"));
    }

    /// Invalidating by the changed file's path drops the entry.
    ///
    /// This is the assertion that matters most, and it is deliberately
    /// end-to-end on a real filesystem: the read set is recorded from
    /// paths the *resolver* builds by joining onto the module dir, while
    /// invalidation keys off a path the *server* canonicalises. Nothing
    /// guarantees those two spellings agree — on Windows canonicalising
    /// yields a `\?\` verbatim prefix — and if they diverge the cache
    /// silently never invalidates.
    #[test]
    fn changing_an_imported_file_invalidates_the_seed() {
        let fx = Fixture::new("invalidate");
        let cache = SeedCache::default();
        assert!(fx.seed(&cache).classes.contains_key("Alpha"));

        fx.write_lib("export class Beta\nend\n");
        cache.invalidate_dependents_of(&fx.lib);

        let after = fx.seed(&cache);
        assert!(
            after.classes.contains_key("Beta"),
            "invalidation did not take: read set and invalidation key disagree"
        );
        assert!(!after.classes.contains_key("Alpha"));
    }

    /// Editing the importer's own text keeps its seed — that is what
    /// makes typing cheap — but editing an `import` line does not.
    #[test]
    fn the_importers_own_edits_only_matter_at_the_import_lines() {
        let fx = Fixture::new("imports");
        let cache = SeedCache::default();
        assert!(fx.seed(&cache).classes.contains_key("Alpha"));

        // Body edit: same imports, so the cached seed still stands even
        // though `lib.sau` has changed underneath.
        fx.write_lib("export class Beta\nend\n");
        fx.write_main("import * from \"lib\"\n\nlocal x = 1\n");
        assert!(fx.seed(&cache).classes.contains_key("Alpha"));

        // Import edit: a different graph, so the entry cannot be reused.
        std::fs::write(fx.dir.join("other.sau"), "export class Gamma\nend\n").expect("write");
        fx.write_main("import * from \"lib\"\nimport * from \"other\"\n");
        let after = fx.seed(&cache);
        assert!(after.classes.contains_key("Gamma"), "import change ignored");
        assert!(after.classes.contains_key("Beta"));
    }

    /// A file with no path on disk is still answerable — it just isn't
    /// cached, rather than panicking or caching under a bogus key.
    #[test]
    fn an_unsaved_buffer_is_served_without_caching() {
        let fx = Fixture::new("unsaved");
        let cache = SeedCache::default();
        let source = std::fs::read_to_string(&fx.main).expect("read");
        let tokens = saule_lexer::Lexer::new(&source).tokenize().expect("lex");
        let module = saule_parser::parse(tokens).expect("parse");

        let seed = cache.seed_for(None, &module, &fx.dir, |_| None);
        assert!(seed.classes.contains_key("Alpha"));
        assert!(
            cache.entries.is_empty(),
            "unkeyed seed should not be cached"
        );
    }
}
