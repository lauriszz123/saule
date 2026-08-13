//! Running the import walk with the database as its filesystem.
//!
//! The walk lives in `saule-interpreter` (it is the same code that seeds the
//! checker for a real run) and reads every module it reaches through a
//! [`SourceOverlay`](saule_interpreter::module::SourceOverlay) hook. Feeding
//! it [`Db::text`] does two things at once: an unsaved editor buffer is what
//! the walk sees, and every module it reads becomes a recorded dependency of
//! the seed, so editing any file in the import graph invalidates exactly the
//! seeds that read it.
//!
//! What this does *not* catch is a file appearing where an import previously
//! failed to resolve. Resolution probes the filesystem for candidate spellings
//! (`x.sau`, `x.saule`, `x/init.sau`) inside `resolve_import_path`, which does
//! not go through the overlay, so those probes leave no edge behind. Creation
//! and deletion therefore still need [`Db::invalidate_all`]; modification does
//! not. Closing that gap means routing resolution's existence checks through
//! the database too, which is a change to the interpreter's module resolver
//! rather than to this crate.

use std::path::Path;

use crate::Db;

pub(crate) fn collect(
    db: &Db,
    module: &saule_ast::Module,
    dir: &Path,
) -> saule_semantic::ModuleSeed {
    let overlay = |path: &Path| db.text(path).map(|t| t.to_string());
    let modules = |path: &Path| db.walk_tree(path);
    saule_interpreter::module::collect_import_seed_io(
        module,
        dir,
        saule_interpreter::module::SeedIo {
            overlay: &overlay,
            modules: Some(&modules),
        },
    )
}
