//! tower-lsp `LanguageServer` implementation: document cache + diagnostic
//! publishing on open / change, plus full-document and range formatting.
//!
//! Pipeline per document: lex → parse → semantic::analyze_with_seed
//! (seeded from `import` statements so imported class/method signatures
//! resolve) → typeck::check. All diagnostics from a single pass are
//! batched into one `publishDiagnostics` notification per file.
//!
//! ## Thread safety
//!
//! `saule-semantic` and `saule-typeck` share a thread-local registry,
//! installed by `analyze` and consulted by `check`. To keep that
//! invariant intact under concurrent LSP requests we serialize the whole
//! `analyze → check` window behind an async mutex.

use std::collections::HashSet;
use std::ops::Range;
use std::path::{Path, PathBuf};

use dashmap::DashMap;
use saule_ast::{Decl, Module, Stmt};
use saule_fmt::{Comment, CommentKind};
use saule_lexer::Token;
use tokio::sync::Mutex;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::{
    Diagnostic, DiagnosticSeverity, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DidSaveTextDocumentParams, DocumentFormattingParams,
    DocumentRangeFormattingParams, Hover, HoverContents, HoverParams, HoverProviderCapability,
    InitializeParams, InitializeResult, InitializedParams, MarkupContent, MarkupKind, MessageType,
    OneOf, Position, SaveOptions, ServerCapabilities, ServerInfo, TextDocumentSyncCapability,
    TextDocumentSyncKind, TextDocumentSyncOptions, TextDocumentSyncSaveOptions, TextEdit, Url,
};
use tower_lsp::{Client, LanguageServer};

use crate::hover;
use crate::line_index::LineIndex;
use crate::workspace;

/// Per-document state: the latest source we've been told about plus the
/// client's version counter. Stored verbatim so any part of the pipeline
/// (diagnostics today, formatting / hover later) can re-run against the
/// authoritative text without re-reading from disk or trusting whatever
/// snapshot the request happens to carry.
struct Document {
    source: String,
    version: i32,
}

pub struct Backend {
    client: Client,
    /// Open documents, keyed by URI string. `DashMap` lets independent
    /// per-file operations interleave without a global lock.
    docs: DashMap<String, Document>,
    /// Every `.sau` file discovered under any workspace root, whether
    /// or not it's currently open. Keyed by absolute canonical path —
    /// the same key shape we use for the import graph.
    workspace_files: DashMap<PathBuf, ()>,
    /// Reverse import graph: `target → set of importers`. When `target`
    /// changes we re-analyse every importer so cross-file type errors
    /// stay in sync without making the editor open every dependent.
    rev_imports: DashMap<PathBuf, HashSet<PathBuf>>,
    /// Workspace root directories supplied at `initialize` time. Used
    /// to locate `saule.config` and bound the recursive file scan.
    workspace_roots: Mutex<Vec<PathBuf>>,
    /// Cached project info (from `saule.config`) so we can re-install
    /// it on whatever tokio worker happens to be running each analysis
    /// — `saule_interpreter::project` uses thread-local state.
    project_info: Mutex<Option<saule_interpreter::project::ProjectInfo>>,
    /// Serialises the analyze→typeck phase across all documents — the
    /// thread-local registries those passes use are global per thread,
    /// so concurrent runs would race even on different files.
    analysis_lock: Mutex<()>,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            docs: DashMap::new(),
            workspace_files: DashMap::new(),
            rev_imports: DashMap::new(),
            workspace_roots: Mutex::new(Vec::new()),
            project_info: Mutex::new(None),
            analysis_lock: Mutex::new(()),
        }
    }

    /// Replace the cached document for `uri` and re-run analysis. The
    /// version we publish back to the client is the one we just stored
    /// so stale results from an older revision can be discarded.
    async fn update(&self, uri: Url, source: String, version: i32) {
        self.docs
            .insert(uri.to_string(), Document { source, version });
        self.refresh(uri).await;
    }

    /// Re-analyse `uri` from the cached source (or disk) and publish
    /// diagnostics, then chase the reverse-import graph and re-publish
    /// every file that imports this one so cross-file errors stay live.
    async fn refresh(&self, uri: Url) {
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
    async fn refresh_path(&self, abs: &Path, uri: Url) {
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
    async fn initial_workspace_scan(&self) {
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

    /// Format the cached source for `uri` and return a single TextEdit
    /// replacing the whole document. Returns `None` if the document is
    /// missing or fails to lex/parse — the client should leave the
    /// buffer untouched on error.
    async fn format_document(&self, uri: &Url) -> Option<Vec<TextEdit>> {
        let entry = self.docs.get(uri.as_str())?;
        let source = entry.source.clone();
        drop(entry);

        let formatted = format_source(&source)?;
        // Skip the edit entirely when the file is already canonical;
        // avoids spurious undo entries on no-op formats.
        if formatted == source {
            return Some(Vec::new());
        }

        let line_index = LineIndex::new(&source);
        let end = line_index.position(&source, source.len());
        Some(vec![TextEdit {
            range: tower_lsp::lsp_types::Range {
                start: Position::new(0, 0),
                end,
            },
            new_text: formatted,
        }])
    }

    /// Resolve hover information at `pos` inside `uri`. Re-runs lex /
    /// parse / `analyze_with_seed` against the cached source so the
    /// thread-local semantic registries reflect the current document
    /// (including imports), then walks the AST under [`hover::hover_at`]
    /// to find the smallest enclosing node and render a Markdown blurb.
    ///
    /// Returns `None` for closed documents, lex / parse failures, or
    /// nodes that don't have anything useful to surface (literals,
    /// keywords, whitespace).
    async fn hover_at(&self, uri: &Url, pos: Position) -> Option<Hover> {
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
            Some(d) => saule_interpreter::module::collect_import_seed(&module, d),
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
        let import_ctx = hover::build_import_context(&module, module_dir.as_deref());

        let (md, span) = hover::hover_at_with(&module, offset, &import_ctx)?;
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: md,
            }),
            range: Some(line_index.range(&source, span.start, span.end)),
        })
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        // Capture every workspace root the client advertised so the
        // initial scan knows what to walk. Fall back to the deprecated
        // `root_uri` when no folders are provided (older clients).
        let mut roots: Vec<PathBuf> = Vec::new();
        if let Some(folders) = params.workspace_folders {
            for folder in folders {
                if let Ok(p) = folder.uri.to_file_path() {
                    roots.push(p);
                }
            }
        }
        #[allow(deprecated)]
        if roots.is_empty() {
            if let Some(root_uri) = params.root_uri {
                if let Ok(p) = root_uri.to_file_path() {
                    roots.push(p);
                }
            }
        }
        *self.workspace_roots.lock().await = roots;

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                // Full sync + explicit save notifications so editors that
                // batch / debounce `didChange` still trigger a refresh on
                // `:w`. We don't need the document text on save (we
                // already have the latest from `didChange`).
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::FULL),
                        save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                            include_text: Some(false),
                        })),
                        ..Default::default()
                    },
                )),
                document_formatting_provider: Some(OneOf::Left(true)),
                document_range_formatting_provider: Some(OneOf::Left(true)),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "saule-lsp".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "saule-lsp ready")
            .await;
        self.initial_workspace_scan().await;
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        self.update(uri, params.text_document.text, params.text_document.version)
            .await;
    }

    async fn did_change(&self, mut params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let version = params.text_document.version;
        // With `TextDocumentSyncKind::FULL` the client sends the entire
        // document on every change as a single content change event.
        let Some(change) = params.content_changes.pop() else {
            return;
        };
        self.update(uri, change.text, version).await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        // We advertised `include_text = false`, so `params.text` is
        // `None` — just re-run analysis against whatever we cached from
        // the last `did_change`. This catches setups where the client
        // debounces / suppresses `did_change` between saves.
        self.refresh(params.text_document.uri).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.docs.remove(uri.as_str());
        // Don't clear diagnostics — the file is still part of the
        // workspace. Re-analyse from disk so the Problems pane keeps
        // showing any errors that were live when the user closed it.
        if let Some(abs) = uri.to_file_path().ok().and_then(|p| canonical(&p)) {
            if self.workspace_files.contains_key(&abs) {
                self.refresh_path(&abs, uri).await;
                return;
            }
        }
        // File isn't tracked (untitled buffer, outside workspace) —
        // safe to clear stale diagnostics.
        self.client.publish_diagnostics(uri, Vec::new(), None).await;
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        Ok(self.format_document(&params.text_document.uri).await)
    }

    /// Range formatting is implemented as full-document formatting — the
    /// Saule formatter is whole-module by design, so any partial-range
    /// request just re-emits the entire file. The client merges the
    /// returned edit normally.
    async fn range_formatting(
        &self,
        params: DocumentRangeFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>> {
        Ok(self.format_document(&params.text_document.uri).await)
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let p = params.text_document_position_params;
        Ok(self.hover_at(&p.text_document.uri, p.position).await)
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
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

/// Lex (with trivia), parse, and pretty-print `source`. Returns `None`
/// when lex / parse fails — we never want to hand the editor a partial
/// or comment-stripped buffer if the file currently doesn't compile.
fn format_source(source: &str) -> Option<String> {
    let raw = saule_lexer::Lexer::new(source)
        .tokenize_with_trivia()
        .ok()?;

    // Split comments off from the parser token stream — saule-fmt
    // expects the AST and a separate, ordered comment slice.
    let mut comments: Vec<Comment> = Vec::new();
    let mut tokens = Vec::with_capacity(raw.len());
    for tok in raw {
        match tok.value {
            Token::LineComment(text) => comments.push(Comment {
                span: tok.span,
                kind: CommentKind::Line,
                text,
            }),
            Token::BlockComment(text) => comments.push(Comment {
                span: tok.span,
                kind: CommentKind::Block,
                text,
            }),
            other => tokens.push(saule_ast::Spanned {
                value: other,
                span: tok.span,
            }),
        }
    }

    let module = saule_parser::parse(tokens).ok()?;
    Some(saule_fmt::format_module_with_comments(
        &module, source, &comments,
    ))
}

/// Canonicalise a path, falling back to the input on failure. Used as
/// the stable key shape for `workspace_files` and `rev_imports` so the
/// same file referenced via different relative spellings hashes equally.
fn canonical(p: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(p).ok()
}

/// Convert an absolute file path to a `file://` URI suitable for
/// `publishDiagnostics`. Returns `None` if the path isn't absolute or
/// contains invalid UTF-8 the URI crate rejects.
fn path_to_uri(p: &Path) -> Option<Url> {
    Url::from_file_path(p).ok()
}
