//! Document highlight handler — emits a highlight range for every
//! occurrence of the symbol under the cursor in the current file. Built
//! on top of `refs::collect_in_module`, the same machinery that drives
//! goto-definition and find-references, so what gets highlighted stays
//! in sync with what those features see.

use tower_lsp::lsp_types::{DocumentHighlight, DocumentHighlightKind, Position, Url};

use crate::line_index::LineIndex;
use crate::refs;

use super::{Backend, canonical};

impl Backend {
    /// Resolve the cursor and return one highlight per occurrence in
    /// the same file. Definition sites are tagged
    /// [`DocumentHighlightKind::WRITE`]; reference sites
    /// [`DocumentHighlightKind::READ`]. Returns an empty vec for
    /// closed documents, lex/parse failures, or cursors on whitespace.
    pub(super) async fn highlights_at(&self, uri: &Url, pos: Position) -> Vec<DocumentHighlight> {
        let entry = match self.docs.get(uri.as_str()) {
            Some(e) => e,
            None => return Vec::new(),
        };
        let source = entry.source.clone();
        drop(entry);

        let Ok(tokens) = saule_lexer::Lexer::new(&source).tokenize() else {
            return Vec::new();
        };
        let Ok(module) = saule_parser::parse(tokens) else {
            return Vec::new();
        };
        let line_index = LineIndex::new(&source);
        let offset = line_index.offset(&source, pos);

        // Seed the registries from this file's imports so receiver-class
        // resolution inside `find_symbol_at` matches what the diagnostic
        // pipeline saw last.
        let module_dir = uri
            .to_file_path()
            .ok()
            .and_then(|p| canonical(&p))
            .and_then(|p| p.parent().map(|d| d.to_path_buf()));
        let _guard = self.analysis_lock.lock().await;
        if let Some(info) = self.project_info.lock().await.clone() {
            saule_interpreter::project::set(info);
        }
        let seed = match &module_dir {
            Some(d) => self.import_seed(uri, &module, d),
            None => saule_semantic::ModuleSeed::default(),
        };
        let _ = saule_semantic::analyze_with_seed(&module, seed);

        let Some(resolved) = refs::find_symbol_at(&module, &source, offset) else {
            return Vec::new();
        };

        refs::collect_in_module(&module, &source, &resolved.symbol)
            .into_iter()
            .map(|hit| DocumentHighlight {
                range: line_index.range(&source, hit.span.start, hit.span.end),
                kind: Some(if hit.is_def {
                    DocumentHighlightKind::WRITE
                } else {
                    DocumentHighlightKind::READ
                }),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    //! Pure-function tests that bypass `Backend`. Each test parses
    //! source, runs analysis, and exercises the same `refs` calls the
    //! handler does, then asserts on the (kind, span-text) pairs.

    use super::*;

    /// Compute (kind, highlighted-substring) pairs at the cursor placed
    /// at the byte offset of `cursor_at` (first occurrence).
    fn highlights(src: &str, cursor_at: &str) -> Vec<(DocumentHighlightKind, String)> {
        let pos = src.find(cursor_at).expect("needle") + 1;
        let tokens = saule_lexer::Lexer::new(src).tokenize().expect("lex");
        let module = saule_parser::parse(tokens).expect("parse");
        let _ = saule_semantic::analyze(&module);
        let resolved = refs::find_symbol_at(&module, src, pos).expect("resolve");
        refs::collect_in_module(&module, src, &resolved.symbol)
            .into_iter()
            .map(|hit| {
                let kind = if hit.is_def {
                    DocumentHighlightKind::WRITE
                } else {
                    DocumentHighlightKind::READ
                };
                (kind, src[hit.span.clone()].to_string())
            })
            .collect()
    }

    #[test]
    fn highlights_local_def_and_uses() {
        let src = "fn main()\n  local x = 1\n  local y = x + x\nend\n";
        let hits = highlights(src, "x = 1");
        let writes = hits
            .iter()
            .filter(|(k, _)| *k == DocumentHighlightKind::WRITE)
            .count();
        let reads = hits
            .iter()
            .filter(|(k, _)| *k == DocumentHighlightKind::READ)
            .count();
        assert_eq!(writes, 1, "expected 1 def, got {hits:?}");
        assert_eq!(reads, 2, "expected 2 reads, got {hits:?}");
        assert!(hits.iter().all(|(_, s)| s == "x"), "got {hits:?}");
    }

    #[test]
    fn highlights_top_level_function() {
        let src = "fn add(a: integer, b: integer) -> integer\n  return a + b\nend\n\nfn main()\n  local r = add(1, 2)\n  local s = add(3, 4)\nend\n";
        let hits = highlights(src, "add(1");
        assert_eq!(hits.len(), 3, "want def + 2 calls, got {hits:?}");
        assert!(hits.iter().all(|(_, s)| s == "add"));
    }

    #[test]
    fn highlights_class_name() {
        let src = "class Point\n  x: integer = 0\nend\n\nfn main()\n  local p = Point()\n  local q = Point()\nend\n";
        let hits = highlights(src, "Point\n");
        assert!(hits.len() >= 3, "want def + uses, got {hits:?}");
    }
}
