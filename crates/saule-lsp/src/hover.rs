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
mod render;
#[cfg(test)]
mod tests;
mod util;
mod walker;

use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;

use saule_ast::{Decl, Module, Stmt};

use imports::{
    aliases_for_file, aliases_for_native, render_file_import_blurb, render_native_import_blurb,
    render_unresolved_import,
};
use render::render_function_sig;

/// Out-of-band import information passed into [`hover_at_with`] so the
/// resolver can answer questions the AST + registries don't cover on
/// their own:
///
/// * `fn_sigs` — top-level functions imported from another `.sau` file
///   or a native package, keyed by the *local alias* under which they
///   appear in the importing module's scope. Free functions never make
///   it into the class / interface / enum registry, so without this map
///   hovering on `foo` after `import { foo } from "lib"` would fall
///   through to "unknown ident".
/// * `import_blurbs` — pre-rendered Markdown for each `import` statement
///   keyed by its source span. The cursor is matched against the keys
///   so hovering anywhere on `import Storage from "storage"` surfaces
///   "imports `Storage` from `…/storage.sau`" without re-resolving the
///   path during the AST walk.
#[derive(Default, Clone, Debug)]
pub struct ImportContext {
    pub fn_sigs: HashMap<String, String>,
    pub import_blurbs: Vec<(Range<usize>, String)>,
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
/// 1. Top-level free function signatures, keyed by the local alias the
///    importer sees them under.
/// 2. A pre-rendered "imports `X` from `Y`" blurb keyed by the import
///    statement's source span, so hovering anywhere on the statement
///    shows where the names come from.
///
/// Best-effort: any import that fails to resolve / read / parse is
/// silently skipped — semantic analysis or the runtime will surface
/// the user-facing error elsewhere. Native packages contribute their
/// `exports` list and "native package" label.
pub fn build_import_context(module: &Module, dir: Option<&Path>) -> ImportContext {
    let mut ctx = ImportContext::default();

    for stmt in &module.stmts {
        let Stmt::Decl(d) = &stmt.value else { continue };
        let Decl::Import { names, path } = &d.value else {
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

        // Collect every top-level function the imported file declares,
        // keyed by its declared name. The alias map below decides
        // which ones (and under what local name) actually land in the
        // importing module's scope.
        let mut imported_fns: HashMap<String, String> = HashMap::new();
        for s in &imported.stmts {
            if let Stmt::Decl(d) = &s.value
                && let Decl::Function {
                    name,
                    type_params,
                    params,
                    return_ty,
                    ..
                } = &d.value
            {
                imported_fns.insert(
                    name.clone(),
                    render_function_sig(name, type_params, params, return_ty.as_ref()),
                );
            }
        }

        let aliases = aliases_for_file(&imported, names);
        for (orig, alias) in &aliases {
            if let Some(md) = imported_fns.get(orig) {
                // Re-render with the alias name so hovering on the
                // local binding shows the name the user actually
                // typed, not the upstream one.
                if alias != orig {
                    if let Some(rendered) = imported_fns.get(orig) {
                        ctx.fn_sigs.insert(
                            alias.clone(),
                            rendered.replacen(&format!("fn {orig}"), &format!("fn {alias}"), 1),
                        );
                        continue;
                    }
                }
                ctx.fn_sigs.insert(alias.clone(), md.clone());
            }
        }

        ctx.import_blurbs.push((
            d.span.clone(),
            render_file_import_blurb(path, &abs.display().to_string(), &aliases),
        ));
    }

    ctx
}
