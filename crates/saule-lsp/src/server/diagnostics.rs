//! Diagnostic pipeline: cache → analyse → publish, plus the reverse
//! import graph and the initial workspace scan that runs at startup.

use std::ops::Range;
use std::path::{Path, PathBuf};

use saule_ast::{Decl, Module, Stmt};
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Url};

use crate::line_index::LineIndex;

use super::{Backend, Document, canonical, path_to_uri};

impl Backend {
    /// so stale results from an older revision can be discarded.
    pub(super) async fn update(&self, uri: Url, source: String, version: i32) {
        // Into the database first: this file's new text is an input to
        // every answer derived from it, including every *other* file's
        // import seed. What survives the edit is decided by the dependency
        // graph, not here.
        if let Some(abs) = uri.to_file_path().ok().and_then(|p| canonical(&p)) {
            self.with_db(|db| db.set_overlay(&abs, source.clone()));
        }
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
                // Only the ones an edit can actually have changed. The
                // database knows whether anything this file reads came out
                // different; when nothing did, its diagnostics are the ones
                // already on screen.
                if !self.analysis_inputs_moved(&importer).await {
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
            let (diagnostics, _) = self.collect_diagnostics(&uri, &source, None, None).await;
            self.client
                .publish_diagnostics(uri, diagnostics, Some(version))
                .await;
        }
    }

    /// Has anything `path`'s analysis reads changed value since its
    /// diagnostics were last published?
    ///
    /// A path with nothing published yet answers `true` — there are no
    /// diagnostics on screen for it to still be right.
    async fn analysis_inputs_moved(&self, path: &Path) -> bool {
        let Some(previous) = self.analysed_at.get(path).map(|r| *r) else {
            return true;
        };
        // Same window the analysis itself runs in — see `collect_diagnostics`.
        let _guard = self.analysis_lock.lock().await;
        self.install_project_for(path.parent()).await;
        previous != self.with_db(|db| db.analysis_revision(path))
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
        let (diagnostics, revision) = self
            .collect_diagnostics(&uri, &source, module_dir, Some(abs))
            .await;
        self.client
            .publish_diagnostics(uri, diagnostics, version)
            .await;
        // What is recorded is what is on screen. A file that did not get as
        // far as analysis records nothing, so the next edit re-checks it.
        match revision {
            Some(rev) => {
                self.analysed_at.insert(abs.to_path_buf(), rev);
            }
            None => {
                self.analysed_at.remove(abs);
            }
        }
    }

    async fn collect_diagnostics(
        &self,
        uri: &Url,
        source: &str,
        module_dir: Option<PathBuf>,
        abs_path: Option<&Path>,
    ) -> (Vec<Diagnostic>, Option<u64>) {
        let line_index = LineIndex::new(source);
        let mut out = Vec::new();

        // ---- lex + parse --------------------------------------------------
        // Recovering rather than strict, so a file with a syntax error near
        // the top still reports the errors further down instead of hiding
        // behind the first one. Everything after this point is skipped when
        // any of them fired: the recovered tree has holes in it by
        // construction, and "undefined name" against a hole is a diagnostic
        // about our repair, not about the user's code.
        //
        // Through the database, so this file's remembered shape is both
        // consulted and — when the parse comes back clean — refreshed, and
        // so the other handlers firing on this same keystroke get the tree
        // rather than parse it again. This pipeline runs on every keystroke
        // and on every file the startup scan touches, which makes it the one
        // that keeps every other feature warm.
        let parsed = self.parsed(uri, source);
        if !parsed.is_clean() {
            out.extend(parsed.lex.iter().map(|e| diag_from(e, source, &line_index)));
            out.extend(
                parsed
                    .parse
                    .iter()
                    .map(|e| diag_from(e, source, &line_index)),
            );
            return (out, None);
        }
        let module = &parsed.module;

        // ---- doc comments -------------------------------------------------
        // Pure source + AST, so this runs before we contend for the
        // analysis lock. Warnings only: a stale `@param` is worth
        // flagging but never blocks anything downstream.
        for w in saule_docs::validate(module, source) {
            out.push(doc_warning_diag(&w, source, &line_index));
        }

        // ---- semantic + typeck --------------------------------------------
        // Both use a shared thread-local registry; serialise the pair.
        let _guard = self.analysis_lock.lock().await;

        // Install this file's own project on whatever tokio worker we landed
        // on — `project::set` is thread-local and the multi-thread runtime
        // can dispatch us anywhere.
        self.install_project_for(module_dir.as_deref()).await;

        // Refresh the reverse-import graph so future edits to imported
        // modules know to re-check this file.
        if let (Some(abs), Some(dir)) = (abs_path, module_dir.as_deref()) {
            self.update_rev_imports(abs, dir, module);
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
            Some(d) => self.import_seed_at(abs_path, module, d),
            None => saule_semantic::ModuleSeed::default(),
        };
        // Read here and nowhere else. The seed walk resolves import paths
        // against the *project*, which lives in thread-local state, so a
        // revision asked for outside this window — different worker, whatever
        // project it was last handed — would be derived from a seed built
        // against the wrong `src_dirs`, and the memo would keep it.
        let revision = abs_path.map(|p| self.with_db(|db| db.analysis_revision(p)));
        for e in saule_semantic::analyze_with_seed(module, seed) {
            out.push(diag_from(&e, source, &line_index));
        }
        // Run typeck unconditionally — even if semantic flagged issues, the
        // type errors are usually still informative. Typeck reads the
        // registries that `analyze_with_seed` just installed, so the order
        // matters.
        for e in saule_typeck::check(module) {
            out.push(diag_from(&e, source, &line_index));
        }
        (out, revision)
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
            if let Some(project_root) = saule_project::find_root(root)
                && let Some(info) = saule_project::load(&project_root)
            {
                *self.project_info.lock().await = Some(info);
                break;
            }
        }

        for root in &roots {
            for file in saule_project::scan_sources(root) {
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
