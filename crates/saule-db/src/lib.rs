//! Answers about Saule source files, computed once and reused until an
//! edit makes them wrong.
//!
//! Everything that wants to know something about a Saule program — the
//! language server on every keystroke, `saule check` over a whole project —
//! goes through the same sequence: read the file, lex it, parse it, walk its
//! imports to learn what its dependencies declare, then analyse it. The
//! expensive step is the import walk, by two orders of magnitude: measured
//! against `examples/UI Project`, a release build spent ~27ms there and
//! ~0.4ms in lex, parse and all analysis combined.
//!
//! Neither caller could reuse any of it. The CLI recomputed everything per
//! file, so a project's shared imports were re-read once per importer. The
//! language server had a hand-rolled cache for the seed and nothing for the
//! rest, with invalidation rules written per cache and reasoned about by
//! hand.
//!
//! This crate replaces both with a dependency graph. Ask it a question and
//! it records what the answer was derived from; change a file and only the
//! answers that actually read it are affected. See [`engine`] for the
//! validation rule, and [`Db::seed`] for the one case where getting it right
//! is the difference between a responsive editor and a laggy one.
//!
//! ```no_run
//! # use std::path::Path;
//! let mut db = saule_db::Db::new();
//! db.set_overlay(Path::new("/p/main.sau"), "class Main\nend\n".into());
//! let parsed = db.parsed(Path::new("/p/main.sau"));
//! assert!(parsed.is_clean());
//! ```

mod engine;
mod parse;
mod seed;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use engine::{Memo, Query, Rev, Runtime};

pub use engine::Stats;
pub use parse::{Imports, Parsed};

/// A per-path answer and the revision it was computed at, reachable through
/// `&self` so the import walk can use it from inside a callback.
///
/// Not a memoised query: there are no dependency edges to record, because
/// the answer is a function of exactly one file's text. `None` is a real
/// answer — a file that is not there, or does not parse — and worth keeping
/// for the same reason the successful ones are.
type Cache<V> = std::cell::RefCell<HashMap<PathBuf, (Rev, Option<V>)>>;

/// The database. One per workspace; not `Sync`, because the analysis passes
/// it drives keep their registries in thread-local storage and must be run
/// on one thread anyway.
pub struct Db {
    rt: Runtime,
    /// Editor buffers, keyed by canonical path. Consulted ahead of disk by
    /// every read, so an unsaved edit in an imported file is visible to its
    /// importers.
    overlays: HashMap<PathBuf, Arc<str>>,
    /// What was last read from disk for a path, and the revision it was
    /// read at. `None` records that the file was not there — which is an
    /// answer worth caching too.
    disk: Cache<Arc<str>>,
    /// Strictly-parsed trees of the modules the import walk reaches, and
    /// the revision each was parsed at. See [`Db::walk_tree`].
    trees: Cache<Arc<saule_ast::Module>>,
    parsed: HashMap<PathBuf, Memo<Arc<Parsed>>>,
    imports: HashMap<PathBuf, Memo<Arc<Imports>>>,
    seeds: HashMap<PathBuf, Memo<Arc<saule_semantic::ModuleSeed>>>,
    /// Where each file's declarations lived at its last *clean* parse, which
    /// is what lets recovery untangle a deleted `end` in an unindented file.
    ///
    /// Deliberately not a memoised query: it is a memory of the file's
    /// history rather than a function of its current text, and feeding a
    /// recovered tree back into it would let one wrong guess reinforce
    /// itself for the rest of the session.
    shapes: HashMap<PathBuf, saule_parser::PriorShape>,
}

impl Default for Db {
    fn default() -> Self {
        Db::new()
    }
}

impl Db {
    pub fn new() -> Db {
        Db {
            rt: Runtime::new(),
            overlays: HashMap::new(),
            disk: std::cell::RefCell::new(HashMap::new()),
            trees: std::cell::RefCell::new(HashMap::new()),
            parsed: HashMap::new(),
            imports: HashMap::new(),
            seeds: HashMap::new(),
            shapes: HashMap::new(),
        }
    }

    // ── inputs ───────────────────────────────────────────────────────────

    /// Install an editor buffer's text as the authoritative content of
    /// `path`, ahead of whatever is on disk.
    ///
    /// Idempotent in the sense that matters: setting the same text again
    /// does not bump the revision, so a client that re-sends an unchanged
    /// buffer costs nothing.
    pub fn set_overlay(&mut self, path: &Path, text: String) {
        let path = path.to_path_buf();
        if self.overlays.get(&path).is_some_and(|old| **old == *text) {
            return;
        }
        self.overlays.insert(path.clone(), text.into());
        self.rt.bump(path);
    }

    /// Drop the buffer for `path`; subsequent reads come from disk again.
    pub fn clear_overlay(&mut self, path: &Path) {
        if self.overlays.remove(path).is_some() {
            self.rt.bump(path.to_path_buf());
        }
    }

    /// Note that `path` changed on disk — created, edited or deleted.
    ///
    /// Creation matters as much as modification: a query that read a path
    /// that did not exist recorded that read, so the file appearing
    /// invalidates exactly the answers that were waiting for it.
    pub fn file_changed(&mut self, path: &Path) {
        self.rt.bump(path.to_path_buf());
    }

    /// Invalidate everything. For changes no single path describes — a new
    /// `saule.config`, a project's dependencies being re-resolved.
    pub fn invalidate_all(&mut self) {
        self.rt.bump_all();
        self.parsed.clear();
        self.trees.borrow_mut().clear();
        self.disk.borrow_mut().clear();
        self.imports.clear();
        self.seeds.clear();
    }

    /// The text of `path`, from the overlay if one is installed and from
    /// disk otherwise, recording the read either way.
    ///
    /// Takes `&self` so the import walk can read through it while the memo
    /// tables are borrowed.
    pub fn text(&self, path: &Path) -> Option<Arc<str>> {
        self.rt.record(Query::File(path.to_path_buf()));
        if let Some(t) = self.overlays.get(path) {
            return Some(t.clone());
        }
        // Read once per revision, not once per asker. Checking a project
        // walks the import graph from every file in it, so the modules near
        // the root of that graph are asked for again and again — and a
        // failed read is worth remembering too, since an import that does
        // not resolve is probed by every file that makes it.
        let mut disk = self.disk.borrow_mut();
        if let Some((rev, text)) = disk.get(path)
            && *rev >= self.rt.file_changed_at(path)
        {
            return text.clone();
        }
        let text = std::fs::read_to_string(path).ok().map(Arc::from);
        disk.insert(path.to_path_buf(), (self.rt.rev(), text.clone()));
        text
    }

    /// The tree the import walk should use for `abs`: parsed strictly,
    /// once per revision, and shared by every walk that reaches it.
    ///
    /// This is where the duplicated work actually was. The walk starts from
    /// each file in turn, and the modules near the root of a project's
    /// import graph sit on almost every path through it — so a `saule check`
    /// over 34 files re-lexed and re-parsed the same handful of modules
    /// dozens of times. Nothing in the walk needed that; it just had no way
    /// to be told the answer.
    ///
    /// Strict, and separate from [`Db::parsed`], on purpose: the walk skips
    /// a module it cannot parse, and handing it a recovered tree would let
    /// a file that does not compile contribute declarations to its
    /// importers.
    pub(crate) fn walk_tree(&self, abs: &Path) -> Option<Arc<saule_ast::Module>> {
        if let Some((rev, tree)) = self.trees.borrow().get(abs)
            && *rev >= self.rt.file_changed_at(abs)
        {
            // A hit still has to record the read. This cache is shared
            // across queries, so returning early without recording gave the
            // edge to whichever seed happened to parse the module first and
            // to no one else — every *other* importer of it then had no
            // dependency on it at all, and went on serving the answer it
            // computed before the module changed.
            self.rt.record(Query::File(abs.to_path_buf()));
            return tree.clone();
        }
        // Through `text`, so the read is recorded as a dependency of
        // whatever seed is being computed and an unsaved buffer wins.
        let tree = self
            .text(abs)
            .and_then(|source| saule_lexer::Lexer::new(&source).tokenize().ok())
            .and_then(|tokens| saule_parser::parse(tokens).ok())
            .map(Arc::new);
        self.trees
            .borrow_mut()
            .insert(abs.to_path_buf(), (self.rt.rev(), tree.clone()));
        tree
    }

    // ── queries ──────────────────────────────────────────────────────────

    /// The recovered parse tree for `path`, with the lexical and syntactic
    /// errors found on the way.
    ///
    /// Always produces a tree: complete where the text was complete, holed
    /// where it was not. A language server that goes blank the moment a file
    /// stops parsing is a language server people turn off.
    pub fn parsed(&mut self, path: &Path) -> Arc<Parsed> {
        self.query(
            path,
            Query::Parsed,
            |db| &mut db.parsed,
            |db, p| Arc::new(db.compute_parsed(p)),
            // A tree carries spans for every token, so two trees are equal
            // only when the file is byte-identical — which the revision
            // check has already ruled out. Comparing them would cost a deep
            // walk to always answer "changed".
            |_, _| false,
        )
    }

    /// The `import` statements of `path`, and nothing else about it.
    ///
    /// This query exists to be *stable*. It is the firewall between "the
    /// file changed" and "everything derived from the file's imports is
    /// stale": edit a function body and this recomputes to an equal value,
    /// which stops the invalidation right here. See [`Db::seed`].
    pub fn imports(&mut self, path: &Path) -> Arc<Imports> {
        self.query(
            path,
            Query::Imports,
            |db| &mut db.imports,
            |db, p| {
                let parsed = db.parsed(p);
                Arc::new(Imports::of(&parsed.module))
            },
            |old, new| old.key == new.key,
        )
    }

    /// What `path`'s imports bring into scope: the classes, interfaces and
    /// enums declared by every module reachable from it.
    ///
    /// The expensive one. It reads and parses every reachable module
    /// transitively, and every language feature needs it before it can
    /// answer anything about a file that imports something.
    ///
    /// Its dependencies are the file's *import list* — not the file — plus
    /// the text of every module the walk actually read. So typing in a
    /// function body does not rebuild it, and changing an imported file
    /// does.
    pub fn seed(&mut self, path: &Path) -> Arc<saule_semantic::ModuleSeed> {
        self.query(
            path,
            Query::Seed,
            |db| &mut db.seeds,
            |db, p| Arc::new(db.compute_seed(p)),
            // The cutoff that matters most, because it is the one the
            // *editor* feels. A seed's dependencies include the text of
            // every module the walk read, so editing a function body in a
            // widely-imported file invalidates the seed of every importer —
            // and every one of them rebuilds to exactly the value it had,
            // because a body is not part of anything a seed collects.
            // Comparing costs a walk over the collected declarations; not
            // comparing costs re-analysing each of those importers on every
            // keystroke, which is two orders of magnitude more.
            |old, new| old.same_surface(new),
        )
    }

    /// The revision at which anything `path`'s analysis depends on last
    /// *changed value*: its own text, and what its imports declare.
    ///
    /// A caller that has already analysed `path` at some revision can
    /// compare against this and skip the work entirely — which is how the
    /// language server avoids re-checking a file whose inputs are all
    /// unchanged. Validating the seed is the cheap path when nothing moved;
    /// this is deliberately the same query the analysis itself would ask
    /// for, so asking costs nothing extra.
    pub fn analysis_revision(&mut self, path: &Path) -> u64 {
        // Ask for the seed rather than reach into the memo, so it is
        // validated (and, if a dependency really moved, recomputed) before
        // its revision is read.
        let _ = self.seed(path);
        let seed_at = self.seeds.get(path).map(|m| m.changed_at).unwrap_or(0);
        seed_at.max(self.rt.file_changed_at(path))
    }

    /// [`Db::seed`] for a module that is not a file on disk — an unsaved
    /// `untitled:` buffer. Computed every time, since there is no stable
    /// key to cache it under.
    pub fn seed_of(&self, module: &saule_ast::Module, dir: &Path) -> saule_semantic::ModuleSeed {
        seed::collect(self, module, dir)
    }

    /// Lex and parse `source` on its own, with no memoisation and no file
    /// identity. For buffers with no path.
    pub fn parse_anonymous(&self, source: &str) -> Parsed {
        parse::analyze(source, None)
    }

    /// Where `path`'s declarations lived at its last clean parse, for a
    /// caller that needs to run recovery itself — completion repairs the
    /// buffer around the cursor before parsing it, so it cannot go through
    /// [`Db::parsed`], but it wants the same memory of the file.
    pub fn prior_shape(&self, path: &Path) -> Option<saule_parser::PriorShape> {
        self.shapes.get(path).cloned()
    }

    /// Cache effectiveness since the last [`Db::reset_stats`].
    pub fn stats(&self) -> Stats {
        self.rt.stats()
    }

    pub fn reset_stats(&self) {
        self.rt.reset_stats();
    }

    // ── the engine ───────────────────────────────────────────────────────

    /// Serve one memoised query: validate what is cached, recompute if it
    /// no longer holds, and apply early cutoff when the new value matches
    /// the old.
    ///
    /// `table` selects the memo table, `compute` produces a fresh value,
    /// and `same` decides whether a recomputed value counts as unchanged —
    /// the hinge the whole design turns on, so each caller states it
    /// explicitly rather than inheriting a default.
    fn query<V: Clone>(
        &mut self,
        path: &Path,
        node: fn(PathBuf) -> Query,
        table: fn(&mut Db) -> &mut HashMap<PathBuf, Memo<V>>,
        compute: fn(&mut Db, &Path) -> V,
        same: fn(&V, &V) -> bool,
    ) -> V {
        let key = path.to_path_buf();
        self.rt.record(node(key.clone()));

        if self.is_current(table, &key) {
            self.rt.note_hit();
            return table(self).get(&key).expect("just verified").value.clone();
        }

        let now = self.rt.rev();
        self.rt.push_frame();
        let value = compute(self, path);
        let deps = self.rt.pop_frame();

        // Early cutoff: an answer that came out the same as last time keeps
        // its `changed_at`, so everything derived from it stays valid.
        let previous = table(self).get(&key);
        let unchanged = previous.is_some_and(|m| same(&m.value, &value));
        let changed_at = match previous {
            Some(m) if unchanged => m.changed_at,
            _ => now,
        };
        self.rt.note_recompute(unchanged);

        table(self).insert(
            key.clone(),
            Memo {
                value: value.clone(),
                deps,
                changed_at,
                verified_at: now,
            },
        );
        value
    }

    /// Whether the cached answer for `key` is still good, verifying its
    /// dependencies recursively and re-stamping it if so.
    fn is_current<V>(
        &mut self,
        table: fn(&mut Db) -> &mut HashMap<PathBuf, Memo<V>>,
        key: &PathBuf,
    ) -> bool {
        let now = self.rt.rev();
        let Some(memo) = table(self).get(key) else {
            return false;
        };
        if memo.verified_at == now {
            return true;
        }
        let since = memo.verified_at;
        let deps = memo.deps.clone();
        if deps.iter().any(|d| self.changed_since(d, since)) {
            return false;
        }
        if let Some(memo) = table(self).get_mut(key) {
            memo.verified_at = now;
        }
        true
    }

    /// Did `q`'s value change after revision `since`?
    fn changed_since(&mut self, q: &Query, since: Rev) -> bool {
        match q {
            Query::File(p) => self.rt.file_changed_at(p) > since,
            Query::Parsed(p) => {
                let p = p.clone();
                self.parsed(&p);
                self.changed_at(|db| &mut db.parsed, &p) > since
            }
            Query::Imports(p) => {
                let p = p.clone();
                self.imports(&p);
                self.changed_at(|db| &mut db.imports, &p) > since
            }
            Query::Seed(p) => {
                let p = p.clone();
                self.seed(&p);
                self.changed_at(|db| &mut db.seeds, &p) > since
            }
        }
    }

    fn changed_at<V>(
        &mut self,
        table: fn(&mut Db) -> &mut HashMap<PathBuf, Memo<V>>,
        key: &PathBuf,
    ) -> Rev {
        table(self).get(key).map_or(Rev::MAX, |m| m.changed_at)
    }

    fn compute_parsed(&mut self, path: &Path) -> Parsed {
        let source = self.text(path).unwrap_or_else(|| Arc::from(""));
        let parsed = parse::analyze(&source, self.shapes.get(path));
        // Only a clean parse teaches the recovery what this file looks like.
        if parsed.is_clean() {
            self.shapes.insert(
                path.to_path_buf(),
                saule_parser::PriorShape::of(&parsed.module),
            );
        }
        parsed
    }

    fn compute_seed(&mut self, path: &Path) -> saule_semantic::ModuleSeed {
        // Read through `imports` rather than `parsed`, so this query's only
        // edge into the file being edited is the one that survives typing.
        let imports = self.imports(path);
        let Some(dir) = path.parent() else {
            return saule_semantic::ModuleSeed::default();
        };
        seed::collect(self, &imports.module, dir)
    }
}

#[cfg(test)]
mod tests;
