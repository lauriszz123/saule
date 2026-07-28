//! Diagnostic pipeline: cache → analyse → publish, plus the reverse
//! import graph and the initial workspace scan that runs at startup.

use std::ops::Range;
use std::path::{Path, PathBuf};

use saule_ast::{Decl, Module, Stmt};
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Url};

use crate::line_index::LineIndex;
use crate::workspace;

use super::{Backend, Document, canonical, path_to_uri};

impl Backend {
    /// so stale results from an older revision can be discarded.
    pub(super) async fn update(&self, uri: Url, source: String, version: i32) {
        self.docs
            .insert(uri.to_string(), Document { source, version });
        self.refresh(uri).await;
    }

    /// Re-analyse `uri` from the cached source (or disk) and publish
    /// diagnostics, then chase the reverse-import graph and re-publish
    /// every file that imports this one so cross-file errors stay live.
    pub(super) async fn refresh(&self, uri: Url) {
        let abs = uri.to_file_path().ok().and_then(|p| canonical(&p));
        if let Some(a) = &abs {
            self.refresh_path(a, uri.clone()).await;
            // Re-analyse importers (best-effort; missing entries skip).
            let importers: Vec<PathBuf> = self
                .rev_imports
                .get(a)
                .map(|e| e.iter().cloned().collect())
                .unwrap_or_default();
            for importer in importers {
                if &importer == a {
                    continue;
                }
                if let Some(dep_uri) = path_to_uri(&importer) {
                    self.refresh_path(&importer, dep_uri).await;
                }
            }
        } else {
            // No file path (e.g. `untitled:` buffer) — analyse the open
            // doc only, no cross-file follow-up possible.
            let Some(entry) = self.docs.get(uri.as_str()) else {
                return;
            };
            let source = entry.source.clone();
            let version = entry.version;
            drop(entry);
            let diagnostics = self.collect_diagnostics(&source, None, None).await;
            self.client
                .publish_diagnostics(uri, diagnostics, Some(version))
                .await;
        }
    }

    /// Analyse a single file by absolute path. Source is taken from the
    /// open-document cache if present, otherwise read from disk.
    pub(super) async fn refresh_path(&self, abs: &Path, uri: Url) {
        let (source, version) = if let Some(entry) = self.docs.get(uri.as_str()) {
            let s = entry.source.clone();
            let v = Some(entry.version);
            drop(entry);
            (s, v)
        } else {
            match std::fs::read_to_string(abs) {
                Ok(s) => (s, None),
                Err(_) => return,
            }
        };
        let module_dir = abs.parent().map(|d| d.to_path_buf());
        let diagnostics = self
            .collect_diagnostics(&source, module_dir, Some(abs))
            .await;
        self.client
            .publish_diagnostics(uri, diagnostics, version)
            .await;
    }

    async fn collect_diagnostics(
        &self,
        source: &str,
        module_dir: Option<PathBuf>,
        abs_path: Option<&Path>,
    ) -> Vec<Diagnostic> {
        let line_index = LineIndex::new(source);
        let mut out = Vec::new();

        // ---- lex ----------------------------------------------------------
        let tokens = match saule_lexer::Lexer::new(source).tokenize() {
            Ok(t) => t,
            Err(err) => {
                out.push(diag_from(&err, source, &line_index));
                return out;
            }
        };

        // ---- parse --------------------------------------------------------
        let module = match saule_parser::parse(tokens) {
            Ok(m) => m,
            Err(err) => {
                out.push(diag_from(&err, source, &line_index));
                return out;
            }
        };

        // ---- doc comments -------------------------------------------------
        // Pure source + AST, so this runs before we contend for the
        // analysis lock. Warnings only: a stale `@param` is worth
        // flagging but never blocks anything downstream.
        for w in saule_docs::validate(&module, source) {
            out.push(doc_warning_diag(&w, source, &line_index));
        }

        // ---- semantic + typeck --------------------------------------------
        // Both use a shared thread-local registry; serialise the pair.
        let _guard = self.analysis_lock.lock().await;

        // Install cached project info on whatever tokio worker we landed
        // on — `project::set` is thread-local and the multi-thread
        // runtime can dispatch us anywhere. Cheap clone, idempotent.
        if let Some(info) = self.project_info.lock().await.clone() {
            saule_interpreter::project::set(info);
        }

        // Refresh the reverse-import graph so future edits to imported
        // modules know to re-check this file.
        if let (Some(abs), Some(dir)) = (abs_path, module_dir.as_deref()) {
            self.update_rev_imports(abs, dir, &module);
        }

        // ---- import resolution -------------------------------------------
        // Surface unresolved `import` paths as errors so typos like
        // `import "intrprtr"` show up in the editor instead of silently
        // becoming a missing-symbol pile-up downstream. Project info was
        // installed above, so src_dirs / dependency resolution works.
        if let Some(dir) = module_dir.as_deref() {
            for stmt in &module.stmts {
                let Stmt::Decl(d) = &stmt.value else {
                    continue;
                };
                let Decl::Import { path, .. } = &d.value else {
                    continue;
                };
                if saule_interpreter::module::resolve_import_path(dir, path).is_none() {
                    out.push(import_error_diag(path, d.span.clone(), source, &line_index));
                }
            }
        }

        // Walk imports to seed the semantic registry with classes /
        // interfaces / enums from sibling files so cross-file lookups
        // (e.g. `Json.decode(...)` from `import "json"`) resolve.
        let seed = match &module_dir {
            Some(d) => saule_interpreter::module::collect_import_seed(&module, d),
            None => saule_semantic::ModuleSeed::default(),
        };
        for e in saule_semantic::analyze_with_seed(&module, seed) {
            out.push(diag_from(&e, source, &line_index));
        }
        // Run typeck unconditionally — even if semantic flagged issues, the
        // type errors are usually still informative. Typeck reads the
        // registries that `analyze_with_seed` just installed, so the order
        // matters.
        for e in saule_typeck::check(&module) {
            out.push(diag_from(&e, source, &line_index));
        }
        out
    }

    /// Replace this file's outgoing-import edges in the reverse graph.
    /// We remove every prior back-edge pointing at `abs` first so deleted
    /// imports don't linger and trigger spurious re-analysis.
    fn update_rev_imports(&self, abs: &Path, dir: &Path, module: &Module) {
        self.rev_imports.iter_mut().for_each(|mut entry| {
            entry.remove(abs);
        });
        for stmt in &module.stmts {
            let Stmt::Decl(d) = &stmt.value else { continue };
            let Decl::Import { path, .. } = &d.value else {
                continue;
            };
            let Some(target) = saule_interpreter::module::resolve_import_path(dir, path) else {
                continue;
            };
            let target = canonical(&target).unwrap_or(target);
            self.rev_imports
                .entry(target)
                .or_default()
                .insert(abs.to_path_buf());
        }
    }

    /// One-shot scan invoked from `initialized`: locate `saule.config`,
    /// install the project info, then walk every workspace root and
    /// publish diagnostics for every `.sau` file found.
    pub(super) async fn initial_workspace_scan(&self) {
        let roots: Vec<PathBuf> = self.workspace_roots.lock().await.clone();
        if roots.is_empty() {
            return;
        }

        // First config wins — multi-root workspaces with multiple Saule
        // projects aren't supported (the interpreter holds a single
        // `ProjectInfo` slot).
        for root in &roots {
            if let Some(project_root) = workspace::find_project_root(root) {
                if let Some(info) = workspace::load_project(&project_root) {
                    *self.project_info.lock().await = Some(info);
                    break;
                }
            }
        }

        for root in &roots {
            for file in workspace::scan_saule_files(root) {
                let canon = canonical(&file).unwrap_or(file);
                self.workspace_files.insert(canon.clone(), ());
                if let Some(uri) = path_to_uri(&canon) {
                    self.refresh_path(&canon, uri).await;
                }
            }
        }
    }
}

/// Turn any `miette::Diagnostic` into an LSP `Diagnostic`. Uses the
/// first labeled span as the primary location.
fn diag_from<D: miette::Diagnostic>(err: &D, source: &str, line_index: &LineIndex) -> Diagnostic {
    let range = primary_range(err)
        .map(|r| line_index.range(source, r.start, r.end))
        .unwrap_or_default();

    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some("saule".to_string()),
        message: err.to_string(),
        ..Default::default()
    }
}

/// Diagnostic for a doc-comment problem. Emitted as a warning, not an
/// error — the code compiles and runs fine, the prose has just drifted
/// away from the signature it describes.
fn doc_warning_diag(
    w: &saule_docs::DocWarning,
    source: &str,
    line_index: &LineIndex,
) -> Diagnostic {
    Diagnostic {
        range: line_index.range(source, w.span.start, w.span.end),
        severity: Some(DiagnosticSeverity::WARNING),
        source: Some("saule".to_string()),
        message: w.message.clone(),
        ..Default::default()
    }
}

/// Diagnostic for an `import` whose path failed to resolve to a file on
/// disk (typo, missing dep, wrong src_dir). Uses the whole import
/// statement span so editors highlight the entire line.
fn import_error_diag(
    path: &str,
    span: Range<usize>,
    source: &str,
    line_index: &LineIndex,
) -> Diagnostic {
    Diagnostic {
        range: line_index.range(source, span.start, span.end),
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some("saule".to_string()),
        message: format!("unresolved import: `{path}` — no matching file or dependency"),
        ..Default::default()
    }
}

/// Extract the primary byte range from a miette diagnostic by taking the
/// first labelled span. Falls back to `None` if the diagnostic carries
/// no labels (unlikely for the Saule error types, all of which annotate
/// a span).
fn primary_range<D: miette::Diagnostic>(err: &D) -> Option<Range<usize>> {
    let mut labels = err.labels()?;
    let label = labels.next()?;
    let inner = label.inner();
    let start = inner.offset();
    let end = start + inner.len();
    Some(start..end)
}
