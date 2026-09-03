//! What the database has to get right, stated as assertions.
//!
//! Three properties carry the design, and each of them was a hand-written
//! invalidation rule in the caches this replaces:
//!
//! 1. An edit to a file's *body* must not rebuild its import seed.
//! 2. An edit to an *imported* file must rebuild it.
//! 3. An edit to a file's *import lines* must rebuild it.
//!
//! The tests below are deliberately end-to-end over a real directory: the
//! read set is recorded from paths the interpreter's resolver builds by
//! joining onto a module directory, while invalidation keys off paths the
//! caller supplies. Nothing guarantees those two spellings agree — on
//! Windows, canonicalising yields a `\\?\` verbatim prefix — and if they
//! diverge the cache silently never invalidates.

use std::fs;
use std::path::{Path, PathBuf};

use super::*;

struct Fixture {
    dir: PathBuf,
    main: PathBuf,
    lib: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Fixture {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir =
            std::env::temp_dir().join(format!("saule-db-{tag}-{}-{nanos}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        let dir = fs::canonicalize(&dir).expect("canonicalize");
        fs::write(dir.join("main.sau"), "import * from \"lib\"\n").expect("write main");
        fs::write(dir.join("lib.sau"), "export class Alpha\nend\n").expect("write lib");
        Fixture {
            main: dir.join("main.sau"),
            lib: dir.join("lib.sau"),
            dir,
        }
    }

    fn write(&self, path: &Path, text: &str) {
        fs::write(path, text).expect("write");
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Property 1, and the reason the whole engine exists.
///
/// The edit changes `main.sau`, so its text, its parse tree and its
/// diagnostics are all genuinely stale — but its *import list* comes out
/// equal, and the seed hangs off that. Asserted through `recomputes` rather
/// than through timing, so it fails for the right reason.
#[test]
fn editing_a_body_does_not_rebuild_the_seed() {
    let fx = Fixture::new("body-edit");
    let mut db = Db::new();
    assert!(db.seed(&fx.main).classes.contains_key("Alpha"));

    db.reset_stats();
    db.set_overlay(&fx.main, "import * from \"lib\"\n\nlocal x = 1\n".into());
    assert!(db.seed(&fx.main).classes.contains_key("Alpha"));

    let stats = db.stats();
    assert_eq!(stats.cutoffs, 1, "the import list should have cut off");
    assert!(
        stats.recomputes <= 2,
        "expected only parse + imports to recompute, got {stats:?}"
    );
}

/// Property 2. The seed's edges include every module the walk read, so this
/// must be picked up — and picked up without anyone telling the database
/// which files depend on which.
#[test]
fn editing_an_imported_file_rebuilds_the_seed() {
    let fx = Fixture::new("import-edit");
    let mut db = Db::new();
    assert!(db.seed(&fx.main).classes.contains_key("Alpha"));

    fx.write(&fx.lib, "export class Beta\nend\n");
    db.file_changed(&fx.lib);

    let seed = db.seed(&fx.main);
    assert!(
        seed.classes.contains_key("Beta"),
        "invalidation did not take"
    );
    assert!(!seed.classes.contains_key("Alpha"));
}

/// Property 3: the cutoff is on the import list's *value*, so changing it
/// does propagate.
#[test]
fn editing_an_import_line_rebuilds_the_seed() {
    let fx = Fixture::new("import-line");
    let mut db = Db::new();
    assert!(db.seed(&fx.main).classes.contains_key("Alpha"));

    fs::write(fx.dir.join("other.sau"), "export class Gamma\nend\n").expect("write other");
    db.set_overlay(
        &fx.main,
        "import * from \"lib\"\nimport * from \"other\"\n".into(),
    );

    let seed = db.seed(&fx.main);
    assert!(seed.classes.contains_key("Gamma"), "import change ignored");
    assert!(seed.classes.contains_key("Alpha"));
}

/// An overlay is the editor's unsaved buffer and outranks the file on disk,
/// transitively — the walk must see it for imported modules too, or an
/// importer reports against a version of its dependency that no longer
/// exists anywhere but the editor's memory.
#[test]
fn an_unsaved_buffer_outranks_the_file_on_disk() {
    let fx = Fixture::new("overlay");
    let mut db = Db::new();
    assert!(db.seed(&fx.main).classes.contains_key("Alpha"));

    db.set_overlay(&fx.lib, "export class Edited\nend\n".into());
    assert!(db.seed(&fx.main).classes.contains_key("Edited"));

    db.clear_overlay(&fx.lib);
    assert!(db.seed(&fx.main).classes.contains_key("Alpha"));
}

/// Re-sending an unchanged buffer is something editors do constantly; it
/// must not cost a revision, or every keystroke's worth of caching is lost
/// to a client that likes to resend.
#[test]
fn setting_the_same_text_again_is_free() {
    let fx = Fixture::new("idempotent");
    let mut db = Db::new();
    let text = "import * from \"lib\"\n";
    db.set_overlay(&fx.main, text.into());
    db.seed(&fx.main);

    db.reset_stats();
    db.set_overlay(&fx.main, text.into());
    db.seed(&fx.main);
    assert_eq!(db.stats().recomputes, 0, "{:?}", db.stats());
}

/// A second question about an unchanged file is answered from the cache
/// outright, without walking the dependency graph again.
#[test]
fn a_repeated_query_is_a_hit() {
    let fx = Fixture::new("hit");
    let mut db = Db::new();
    db.seed(&fx.main);

    db.reset_stats();
    db.seed(&fx.main);
    assert_eq!(db.stats().recomputes, 0);
    assert!(db.stats().hits >= 1);
}

/// A file with a syntax error still yields a tree, and its imports are still
/// readable — recovery is what keeps every other feature working while
/// someone is mid-edit.
#[test]
fn a_broken_file_still_parses_and_still_has_imports() {
    let fx = Fixture::new("broken");
    let mut db = Db::new();
    db.set_overlay(&fx.main, "import * from \"lib\"\n\nclass Half\n".into());

    let parsed = db.parsed(&fx.main);
    assert!(
        !parsed.is_clean(),
        "expected the missing `end` to be reported"
    );
    assert_eq!(db.imports(&fx.main).key.len(), 1);
    assert!(db.seed(&fx.main).classes.contains_key("Alpha"));
}

/// Deleting an imported file is a change like any other: whatever it
/// declared stops being in scope.
#[test]
fn deleting_an_imported_file_empties_the_seed() {
    let fx = Fixture::new("delete");
    let mut db = Db::new();
    assert!(db.seed(&fx.main).classes.contains_key("Alpha"));

    fs::remove_file(&fx.lib).expect("remove lib");
    db.file_changed(&fx.lib);
    assert!(!db.seed(&fx.main).classes.contains_key("Alpha"));
}

/// A buffer with no path on disk is still answerable — it is just not
/// cached, rather than panicking or caching under a bogus key.
#[test]
fn an_anonymous_buffer_is_served_without_caching() {
    let fx = Fixture::new("anon");
    let db = Db::new();
    let parsed = db.parse_anonymous("import * from \"lib\"\n");
    assert!(parsed.is_clean());
    let seed = db.seed_of(&parsed.module, &fx.dir);
    assert!(seed.classes.contains_key("Alpha"));
}

// ── the revision the language server skips on ───────────────────────────────

/// Property 4, and what makes an editor responsive on a widely-imported
/// file: [`Db::analysis_revision`] moves when — and only when — something a
/// file's analysis actually reads came out different.
///
/// Editing a body in `lib.sau` invalidates `main.sau`'s seed, which is
/// rebuilt; the point is that it rebuilds to the *same* value, so the
/// revision stays put and the caller can skip re-checking `main.sau`
/// entirely. Without the comparison every importer of an edited file is
/// re-analysed on every keystroke.
#[test]
fn analysis_revision_tracks_value_not_edits() {
    let f = Fixture::new("analysis-rev");
    let mut db = Db::new();

    f.write(
        &f.lib,
        "export class Alpha\n    fn go() -> integer\n        return 1\n    end\nend\n",
    );
    db.file_changed(&f.lib);
    let start = db.analysis_revision(&f.main);

    // A body edit in the imported file: the surface is untouched, and every
    // declaration below it has shifted, so the spans differ and the values
    // do not.
    f.write(
        &f.lib,
        "export class Alpha\n    fn go() -> integer\n        local x = 1\n        return x\n    end\nend\n",
    );
    db.file_changed(&f.lib);
    let after_body = db.analysis_revision(&f.main);
    assert_eq!(
        after_body, start,
        "a body edit in an imported file must not move the importer's revision"
    );

    // Its *surface*: a new exported class.
    f.write(
        &f.lib,
        "export class Alpha\n    fn go() -> integer\n        return 1\n    end\nend\nexport class Beta\nend\n",
    );
    db.file_changed(&f.lib);
    let after_surface = db.analysis_revision(&f.main);
    assert!(
        after_surface > after_body,
        "a new export must move the importer's revision ({after_surface} vs {after_body})"
    );

    // Renaming it away moves it again — and the importer's own edits count
    // as much as its imports'.
    f.write(&f.lib, "export class Gamma\nend\n");
    db.file_changed(&f.lib);
    let after_rename = db.analysis_revision(&f.main);
    assert!(after_rename > after_surface, "a rename must move it");

    f.write(&f.main, "import * from \"lib\"\n-- a note\n");
    db.file_changed(&f.main);
    let after_own = db.analysis_revision(&f.main);
    assert!(
        after_own > after_rename,
        "the file's own text is an input too"
    );

    // Asking again with nothing changed is stable — the caller compares
    // against it, so a revision that drifted on its own would re-check
    // everything forever.
    assert_eq!(db.analysis_revision(&f.main), after_own);
    assert_eq!(db.analysis_revision(&f.main), after_own);
}

/// Two importers of the same module, one of which also imports the other —
/// the shape the language server actually sees. Both revisions have to move
/// when the shared dependency's surface does; a file whose diagnostics stop
/// being re-checked shows stale errors forever.
#[test]
fn every_importer_of_a_changed_surface_moves() {
    let f = Fixture::new("two-importers");
    // lib.sau is the shared dependency; mid.sau imports it; top.sau imports
    // both.
    f.write(&f.lib, "export class Alpha\nend\n");
    let mid = f.dir.join("mid.sau");
    let top = f.dir.join("top.sau");
    f.write(&mid, "import * from \"lib\"\n");
    f.write(&top, "import * from \"lib\"\nimport * from \"mid\"\n");

    let mut db = Db::new();
    db.analysis_revision(&mid);
    db.analysis_revision(&top);

    // A body edit: neither importer's surface view changes.
    f.write(&f.lib, "export class Alpha\n    fn go()\n    end\nend\n");
    db.file_changed(&f.lib);
    let mid1 = db.analysis_revision(&mid);
    let top1 = db.analysis_revision(&top);

    // A surface change: both must move.
    f.write(&f.lib, "export class Renamed\n    fn go()\n    end\nend\n");
    db.file_changed(&f.lib);
    let mid2 = db.analysis_revision(&mid);
    let top2 = db.analysis_revision(&top);
    assert!(mid2 > mid1, "direct importer must move ({mid1} -> {mid2})");
    assert!(
        top2 > top1,
        "the second importer must move too ({top1} -> {top2})"
    );
}
