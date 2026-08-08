//! Hover-information lookup over a parsed [`Module`].
//!
//! Given a byte offset into the source, walks the AST to find the
//! smallest enclosing node and renders a Markdown blurb for it,
//! consulting the thread-local semantic registries (`saule_semantic`)
//! for class / interface / enum / method metadata.
//!
//! The caller must ensure the registries are populated for the module
//! before invoking [`hover_at`] — `Backend::hover` does this by running
//! `saule_semantic::analyze_with_seed` under the analysis lock, exactly
//! like the diagnostic pipeline.
//!
//! Resolution is intentionally conservative: we don't have a per-span
//! type table from typeck yet, so member / method hovers only fire when
//! the receiver is `self` or a known class name (static access). This
//! still covers the high-leverage cases (`fn` signatures, class /
//! interface / enum heads, parameters, `self.foo`, `Class.method`).
//!
//! Each match returns `(markdown, span)`; the LSP layer uses `span` for
//! the `Hover.range` field so editors can highlight the exact node.

mod imports;
/// `pub(crate)` for `server::sighelp`, which renders the same
/// parameters into a different popup and must not drift from these.
pub(crate) mod render;
#[cfg(test)]
mod tests;
mod util;
mod walker;

use std::ops::Range;
use std::path::Path;

use saule_ast::{Decl, Module, Stmt};

use imports::{
    aliases_for_dynamic, aliases_for_file, aliases_for_native, render_file_import_blurb,
    render_native_import_blurb, render_unresolved_import,
};

/// Out-of-band import information passed into [`hover_at_with`] so the
/// resolver can answer questions the AST + registries don't cover on
/// their own:
///
/// * `import_blurbs` — pre-rendered Markdown for each `import` statement
///   keyed by its source span. The cursor is matched against the keys
///   so hovering anywhere on `import Storage from "storage"` surfaces
///   "imports `Storage` from `…/storage.sau`" without re-resolving the
///   path during the AST walk.
/// * `docs` — `---` doc comments harvested from each imported file,
///   keyed by qualified name (`Storage`, `Storage.put`). The walker
///   resolves most cross-file symbols through the semantic registries,
///   which carry types but no source text and therefore no docs; this
///   index is what lets a hover on an imported class or method show the
///   prose written next to its declaration in the other file.
///
/// Imported *functions* are deliberately absent. They live in the
/// semantic function registry, which `analyze_with_seed` fills — from a
/// seed that already follows re-export barrels and already applies
/// `as`-aliases. Collecting them a second time here meant walking the
/// import graph twice per request, and cost far more than it sounds:
/// see [`build_import_context`].
#[derive(Default, Clone, Debug)]
pub struct ImportContext {
    pub import_blurbs: Vec<(Range<usize>, String)>,
    pub docs: saule_docs::DocIndex,
}

/// Find the most specific hover info for `offset` inside `module`.
///
/// Returns `None` if no AST node contains the offset (e.g. cursor on
/// pure whitespace at the top level) or if the deepest enclosing node
/// has no useful hover content (a literal, a `nil`, etc.).
///
/// Convenience wrapper around [`hover_at_with`] for callers that don't
/// have an [`ImportContext`] (currently just the unit tests — Backend
/// always builds one). Kept `pub` for that ergonomic.
#[allow(dead_code)]
pub fn hover_at(module: &Module, offset: usize) -> Option<(String, Range<usize>)> {
    hover_at_with_source(module, "", offset, &ImportContext::default())
}

/// Like [`hover_at`] but also consults `imports` when resolving bare
/// identifiers and import-statement spans. Backend::hover builds a
/// fresh context per request from the cached source's `import`
/// declarations.
#[allow(dead_code)]
pub fn hover_at_with(
    module: &Module,
    offset: usize,
    imports: &ImportContext,
) -> Option<(String, Range<usize>)> {
    hover_at_with_source(module, "", offset, imports)
}

/// Full entry point: like [`hover_at_with`] but also consumes the raw
/// `source` text so the walker can scan within parent spans for
/// identifiers that aren't directly span-tracked in the AST (parameter
/// types, class field types, `extends` / `implements` heads, named
/// call argument keys, per-name import resolution, ...).
pub fn hover_at_with_source(
    module: &Module,
    source: &str,
    offset: usize,
    imports: &ImportContext,
) -> Option<(String, Range<usize>)> {
    walker::run(module, source, offset, imports)
}

/// Build an [`ImportContext`] for `module` by walking every `import`
/// statement, resolving the target file (or native package), and
/// extracting:
///
/// 1. A pre-rendered "imports `X` from `Y`" blurb keyed by the import
///    statement's source span, so hovering anywhere on the statement
///    shows where the names come from.
/// 2. Doc comments from the imported file, merged into
///    [`ImportContext::docs`] beneath this module's own — so hovering a
///    *usage* of a documented symbol shows its prose whether it was
///    declared here or next door.
///
/// Best-effort: any import that fails to resolve / read / parse is
/// silently skipped — semantic analysis or the runtime will surface
/// the user-facing error elsewhere. Native packages contribute their
/// `exports` list and "native package" label.
///
/// **One hop only, deliberately.** This runs on every hover request, and
/// the obvious-looking extension — following `import *` re-export chains
/// so a barrel's contents are reachable — is a trap. Sibling modules
/// import each other, so without a visited set the walk re-parses shared
/// files along every path that reaches them: for a project importing a
/// 24-file UIKit barrel that turned a 24ms graph walk into 1.8 *seconds*
/// per hover. Nothing needs it either. Classes, interfaces, enums and
/// functions all arrive through `analyze_with_seed`, whose seed does
/// follow barrels — and does keep a visited set.
pub fn build_import_context(module: &Module, source: &str, dir: Option<&Path>) -> ImportContext {
    // This module's own docs go in first so they win the `merge`
    // tie-break against any imported name they shadow.
    let mut ctx = ImportContext {
        docs: saule_docs::collect(module, source),
        ..Default::default()
    };

    for stmt in &module.stmts {
        let Stmt::Decl(d) = &stmt.value else { continue };
        let Decl::Import { names, path, .. } = &d.value else {
            continue;
        };

        // Native package — synthesise a blurb listing the exports we
        // know about; the function signatures themselves are already
        // registered globally with `saule_typeck::sigs`, so the
        // identifier resolver will find them via the native-sig path
        // without needing per-alias entries here.
        if let Some(pkg) = saule_interpreter::native_packages::lookup(path) {
            let exports: Vec<&'static str> = pkg.exports.to_vec();
            let aliases = aliases_for_native(&exports, names);
            ctx.import_blurbs
                .push((d.span.clone(), render_native_import_blurb(path, &aliases)));
            continue;
        }

        // Dynamically-discovered native package (a manifest-described shared
        // library such as `"engine"`). `resolve_import_path` answers these
        // with a synthetic sentinel path that is *not* a real file, so they
        // must be handled here — otherwise the `read_to_string` below fails
        // and the import gets mislabelled "unresolved".
        if saule_interpreter::dynamic_packages::is_dynamic_package(path) {
            let exports = saule_interpreter::dynamic_packages::export_names(path);
            let aliases = aliases_for_dynamic(&exports, names);
            ctx.import_blurbs
                .push((d.span.clone(), render_native_import_blurb(path, &aliases)));
            continue;
        }

        let Some(dir) = dir else { continue };
        let Some(abs) = saule_interpreter::module::resolve_import_path(dir, path) else {
            ctx.import_blurbs
                .push((d.span.clone(), render_unresolved_import(path)));
            continue;
        };
        let Ok(source) = std::fs::read_to_string(&abs) else {
            ctx.import_blurbs
                .push((d.span.clone(), render_unresolved_import(path)));
            continue;
        };
        let Ok(tokens) = saule_lexer::Lexer::new(&source).tokenize() else {
            continue;
        };
        let Ok(imported) = saule_parser::parse(tokens) else {
            continue;
        };

        // Harvest the target's doc comments while its source is in hand
        // — the registries the walker consults keep types but drop the
        // text. One level only: this is the file the `import` statement
        // names, not the graph behind it.
        ctx.docs.merge(saule_docs::collect(&imported, &source));

        ctx.import_blurbs.push((
            d.span.clone(),
            render_file_import_blurb(
                path,
                &abs.display().to_string(),
                &aliases_for_file(&imported, names),
            ),
        ));
    }

    ctx
}
