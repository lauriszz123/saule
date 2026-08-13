//! Goto-definition and find-references handlers — shared cursor →
//! symbol resolution plus single-file and workspace-wide reference
//! walks. Backed by the standalone `refs` module.

use std::path::PathBuf;

use tower_lsp::lsp_types::{Location, Position, Url};

use crate::line_index::LineIndex;
use crate::refs::{self, Symbol};

use super::{Backend, canonical, path_to_uri};

/// Cursor resolution result threaded between the resolve step and the
/// workspace walk: the URI we resolved against (used to bound a local
/// search) and the symbol the cursor was on.
pub(super) struct ResolvedCtx {
    pub(super) uri: Url,
    pub(super) symbol: Symbol,
}

impl Backend {
    /// Resolve the cursor at `pos` to a symbol and return its
    /// definition site(s). Workspace symbols (classes, methods, etc.)
    /// are searched across every `.sau` file under the workspace
    /// roots; locals are bounded to the file in which they're declared.
    pub(super) async fn definitions_at(&self, uri: &Url, pos: Position) -> Vec<Location> {
        let Some(ctx) = self.resolve_symbol(uri, pos).await else {
            return Vec::new();
        };
        match ctx.symbol {
            Symbol::Local { .. } => self.collect_in_file(&ctx.uri, &ctx.symbol, true, true),
            Symbol::ImportPath(ref path) => {
                // Definition = the imported file's start. Resolve
                // against the importer's directory.
                let Some(importer) = ctx.uri.to_file_path().ok() else {
                    return Vec::new();
                };
                let Some(dir) = importer.parent() else {
                    return Vec::new();
                };
                let Some(target) = saule_interpreter::module::resolve_import_path(dir, path) else {
                    return Vec::new();
                };
                let Some(target_uri) = path_to_uri(&target) else {
                    return Vec::new();
                };
                vec![Location {
                    uri: target_uri,
                    range: tower_lsp::lsp_types::Range {
                        start: Position::new(0, 0),
                        end: Position::new(0, 0),
                    },
                }]
            }
            _ => self.collect_in_workspace(&ctx, /*defs_only=*/ true).await,
        }
    }

    /// Collect every reference to the symbol under the cursor across
    /// the workspace. Honours the LSP `include_declaration` flag.
    pub(super) async fn references_at(
        &self,
        uri: &Url,
        pos: Position,
        include_def: bool,
    ) -> Vec<Location> {
        let Some(ctx) = self.resolve_symbol(uri, pos).await else {
            return Vec::new();
        };
        match ctx.symbol {
            Symbol::Local { .. } => self.collect_in_file(&ctx.uri, &ctx.symbol, false, include_def),
            Symbol::ImportPath(ref _path) => {
                // References to an import path = every importer.
                self.collect_in_workspace(&ctx, false).await
            }
            _ => self.collect_in_workspace(&ctx, false).await,
        }
    }

    /// Lex/parse the document at `uri` and resolve the cursor at `pos`
    /// to a [`Symbol`]. Runs `analyze_with_seed` first so the resolver
    /// can consult the class / interface / enum registries.
    async fn resolve_symbol(&self, uri: &Url, pos: Position) -> Option<ResolvedCtx> {
        let entry = self.docs.get(uri.as_str())?;
        let source = entry.source.clone();
        drop(entry);

        let module = self.syntax(uri, &source);
        let line_index = LineIndex::new(&source);
        let offset = line_index.offset(&source, pos);
        let module_dir = uri
            .to_file_path()
            .ok()
            .and_then(|p| canonical(&p))
            .and_then(|p| p.parent().map(|d| d.to_path_buf()));

        let _guard = self.analysis_lock.lock().await;
        if let Some(info) = self.project_info.lock().await.clone() {
            saule_project::set(info);
        }
        let seed = match &module_dir {
            Some(d) => self.import_seed(uri, &module, d),
            None => saule_semantic::ModuleSeed::default(),
        };
        let _ = saule_semantic::analyze_with_seed(&module, seed);

        let resolved = refs::find_symbol_at(&module, &source, offset)?;
        Some(ResolvedCtx {
            uri: uri.clone(),
            symbol: resolved.symbol,
        })
    }

    /// Walk a single file (already loaded into the doc cache or read
    /// from disk) collecting hits. `defs_only` keeps just the defining
    /// site; `include_def` controls whether the def is included when
    /// `defs_only` is false.
    fn collect_in_file(
        &self,
        uri: &Url,
        symbol: &Symbol,
        defs_only: bool,
        include_def: bool,
    ) -> Vec<Location> {
        let source = self
            .docs
            .get(uri.as_str())
            .map(|e| e.source.clone())
            .or_else(|| {
                uri.to_file_path()
                    .ok()
                    .and_then(|p| std::fs::read_to_string(&p).ok())
            });
        let Some(source) = source else {
            return Vec::new();
        };
        let module = self.syntax(uri, &source);
        let line_index = LineIndex::new(&source);
        refs::collect_in_module(&module, &source, symbol)
            .into_iter()
            .filter(|hit| {
                if defs_only {
                    hit.is_def
                } else {
                    include_def || !hit.is_def
                }
            })
            .map(|hit| Location {
                uri: uri.clone(),
                range: line_index.range(&source, hit.span.start, hit.span.end),
            })
            .collect()
    }

    /// Walk every known workspace `.sau` file collecting hits for
    /// `ctx.symbol`. Each file's analysis registry is reseeded so
    /// receiver-class lookups work cross-file.
    async fn collect_in_workspace(&self, ctx: &ResolvedCtx, defs_only: bool) -> Vec<Location> {
        let _guard = self.analysis_lock.lock().await;
        if let Some(info) = self.project_info.lock().await.clone() {
            saule_project::set(info);
        }

        let files: Vec<PathBuf> = self
            .workspace_files
            .iter()
            .map(|e| e.key().clone())
            .collect();

        let mut out: Vec<Location> = Vec::new();
        for abs in files {
            let Some(uri) = path_to_uri(&abs) else {
                continue;
            };
            let source = self
                .docs
                .get(uri.as_str())
                .map(|e| e.source.clone())
                .or_else(|| std::fs::read_to_string(&abs).ok());
            let Some(source) = source else { continue };
            let module = self.syntax(&uri, &source);

            // Reseed registries from this file's own imports so
            // receiver-class resolution sees imported classes.
            let module_dir = abs.parent().map(|d| d.to_path_buf());
            let seed = match &module_dir {
                Some(d) => self.import_seed(&uri, &module, d),
                None => saule_semantic::ModuleSeed::default(),
            };
            let _ = saule_semantic::analyze_with_seed(&module, seed);

            let line_index = LineIndex::new(&source);
            for hit in refs::collect_in_module(&module, &source, &ctx.symbol) {
                if defs_only && !hit.is_def {
                    continue;
                }
                out.push(Location {
                    uri: uri.clone(),
                    range: line_index.range(&source, hit.span.start, hit.span.end),
                });
            }
        }
        out
    }
}
