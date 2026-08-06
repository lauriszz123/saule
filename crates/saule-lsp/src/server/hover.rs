//! Hover handler — runs analysis under the shared lock and dispatches
//! to the standalone `hover` walker module.

use tower_lsp::lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind, Position, Url};

use crate::hover;
use crate::line_index::LineIndex;

use super::{Backend, canonical};

impl Backend {
    /// Resolve hover information at `pos` inside `uri`. Re-runs lex /
    /// parse / `analyze_with_seed` against the cached source so the
    /// thread-local semantic registries reflect the current document
    /// (including imports), then walks the AST under [`hover::hover_at`]
    /// to find the smallest enclosing node and render a Markdown blurb.
    ///
    /// Returns `None` for closed documents, lex / parse failures, or
    /// nodes that don't have anything useful to surface (literals,
    /// keywords, whitespace).
    pub(super) async fn hover_at(&self, uri: &Url, pos: Position) -> Option<Hover> {
        let entry = self.docs.get(uri.as_str())?;
        let source = entry.source.clone();
        drop(entry);

        // Lex + parse first so failures don't grab the analysis lock.
        let tokens = saule_lexer::Lexer::new(&source).tokenize().ok()?;
        let module = saule_parser::parse(tokens).ok()?;

        let line_index = LineIndex::new(&source);
        let offset = line_index.offset(&source, pos);

        let module_dir = uri
            .to_file_path()
            .ok()
            .and_then(|p| canonical(&p))
            .and_then(|p| p.parent().map(|d| d.to_path_buf()));

        // Both `analyze_with_seed` and the registry consultation inside
        // `hover_at` use the thread-local registry slots — serialise
        // against the diagnostic pipeline so a concurrent refresh
        // can't swap registries out from under us mid-walk.
        let _guard = self.analysis_lock.lock().await;

        if let Some(info) = self.project_info.lock().await.clone() {
            saule_interpreter::project::set(info);
        }

        let seed = match &module_dir {
            Some(d) => saule_interpreter::module::collect_import_seed_with(
                &module,
                d,
                &self.source_overlay(),
            ),
            None => saule_semantic::ModuleSeed::default(),
        };
        // Diagnostics are discarded: hover should still work on a file
        // that has analysis errors elsewhere. We only need the side
        // effect of populating the class / interface / enum registries.
        let _ = saule_semantic::analyze_with_seed(&module, seed);

        // Build the per-request import context after analysis so any
        // class names introduced by `import` are also visible to the
        // identifier resolver via the registries (the function/blurb
        // info this map adds is purely additive).
        let import_ctx = hover::build_import_context(&module, &source, module_dir.as_deref());

        let (md, span) = hover::hover_at_with_source(&module, &source, offset, &import_ctx)?;
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: md,
            }),
            range: Some(line_index.range(&source, span.start, span.end)),
        })
    }
}
