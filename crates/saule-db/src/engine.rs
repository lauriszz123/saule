//! The memoisation engine: revisions, dependency edges, and the validation
//! walk that decides whether a cached answer still stands.
//!
//! This is a small, deliberately un-generic version of what `salsa` does.
//! The reason to write it rather than take the dependency is that the query
//! set here is fixed and tiny — four node kinds — and the one property that
//! actually matters is [early cutoff](Memo::changed_at), which is a dozen
//! lines once the dependency edges exist. What is *not* negotiable is that
//! the edges be real: the ad-hoc caches this replaces each hard-coded one
//! invalidation rule apiece, and each was subtly wrong in a different way.
//!
//! ## The model
//!
//! There is one global revision counter. Changing an input — a file's text,
//! on disk or in an editor buffer — bumps it and stamps the new value on
//! that file.
//!
//! Every memoised answer records two revisions and the edges it was
//! computed from:
//!
//! * `changed_at` — when this answer last *changed value*.
//! * `verified_at` — when it was last confirmed still current.
//!
//! To validate an answer we ask each of its dependencies whether it changed
//! since we last verified. If none did, the answer stands and only
//! `verified_at` moves; nothing is recomputed. If one did, we recompute —
//! and if the *new value equals the old*, `changed_at` stays where it was,
//! so the answer's own dependents are still valid.
//!
//! That last step is the whole point. Typing a character in `main.sau`
//! changes its text, so its parse tree changes, so its import list is
//! recomputed — and comes out equal, because the edit was in a function
//! body. `changed_at` on the import list does not move, and the import
//! seed built from it — the ~27ms one — is never rebuilt.

use std::collections::HashMap;
use std::path::PathBuf;

pub(crate) type Rev = u64;

/// A node in the dependency graph: either an input file or a memoised query.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Query {
    /// The text of a file, from the overlay or from disk. The only input.
    File(PathBuf),
    Parsed(PathBuf),
    Imports(PathBuf),
    Seed(PathBuf),
}

/// One cached answer.
pub(crate) struct Memo<V> {
    pub value: V,
    pub deps: Vec<Query>,
    pub changed_at: Rev,
    pub verified_at: Rev,
}

/// Cache effectiveness, for tests that assert an edit did *not* rebuild the
/// expensive query, and for anyone wondering where a request went.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    /// Answers served without recomputing anything.
    pub hits: u64,
    /// Answers recomputed from scratch.
    pub recomputes: u64,
    /// Recomputes whose new value equalled the old, so dependents survived.
    pub cutoffs: u64,
}

/// Revisions, the dependency-collection stack, and the input table.
///
/// Separate from the memo tables so a query can record a dependency while
/// the table holding its own answer is borrowed — which is what lets the
/// seed walk read files through a `&self` closure.
pub(crate) struct Runtime {
    rev: Rev,
    /// When each file's text last changed. Absent means "never", which
    /// compares as revision 0 and so is older than any recorded answer.
    files: HashMap<PathBuf, Rev>,
    /// One frame per query currently being computed; edges are pushed onto
    /// the innermost. `RefCell` because the seed walk reads files through a
    /// closure that can only hold `&self`.
    frames: std::cell::RefCell<Vec<Vec<Query>>>,
    stats: std::cell::Cell<Stats>,
}

impl Runtime {
    pub(crate) fn new() -> Runtime {
        Runtime {
            // Answers are stamped with the revision they were computed at,
            // and files default to revision 0, so starting above 0 keeps a
            // never-touched file from looking newer than a fresh answer.
            rev: 1,
            files: HashMap::new(),
            frames: std::cell::RefCell::new(Vec::new()),
            stats: std::cell::Cell::new(Stats::default()),
        }
    }

    pub(crate) fn rev(&self) -> Rev {
        self.rev
    }

    /// Record that `path`'s contents changed, and move to a new revision.
    pub(crate) fn bump(&mut self, path: PathBuf) {
        self.rev += 1;
        self.files.insert(path, self.rev);
    }

    /// Move to a new revision without naming a file — for changes that no
    /// single path describes, like a new project config.
    pub(crate) fn bump_all(&mut self) {
        self.rev += 1;
    }

    pub(crate) fn file_changed_at(&self, path: &std::path::Path) -> Rev {
        self.files.get(path).copied().unwrap_or(0)
    }

    /// Note that whatever is currently being computed read `q`.
    pub(crate) fn record(&self, q: Query) {
        if let Some(frame) = self.frames.borrow_mut().last_mut() {
            frame.push(q);
        }
    }

    pub(crate) fn push_frame(&self) {
        self.frames.borrow_mut().push(Vec::new());
    }

    /// Pop the current frame, deduplicated. A seed walk reads the same
    /// barrel module once per importer; the edge only needs to exist once.
    pub(crate) fn pop_frame(&self) -> Vec<Query> {
        let mut deps = self.frames.borrow_mut().pop().unwrap_or_default();
        let mut seen = std::collections::HashSet::new();
        deps.retain(|q| seen.insert(q.clone()));
        deps
    }

    pub(crate) fn stats(&self) -> Stats {
        self.stats.get()
    }

    pub(crate) fn note_hit(&self) {
        let mut s = self.stats.get();
        s.hits += 1;
        self.stats.set(s);
    }

    pub(crate) fn note_recompute(&self, cutoff: bool) {
        let mut s = self.stats.get();
        s.recomputes += 1;
        if cutoff {
            s.cutoffs += 1;
        }
        self.stats.set(s);
    }

    pub(crate) fn reset_stats(&self) {
        self.stats.set(Stats::default());
    }
}
