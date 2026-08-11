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
//!
//! The handler implementations live in submodules: [`diagnostics`] for
//! the analyse → publish pipeline, [`format`] for formatting,
//! [`hover`] for hover, and [`nav`] for goto-definition / references.

mod completion;
mod diagnostics;
mod format;
mod highlight;
mod hover;
mod inlay;
mod native_names;
mod nav;
mod seed_cache;
mod sighelp;
mod symbols;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::syntax;
use crate::workspace;

use dashmap::DashMap;
use tokio::sync::Mutex;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::{
    DidChangeTextDocumentParams, DidChangeWatchedFilesParams,
    DidChangeWatchedFilesRegistrationOptions, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DidSaveTextDocumentParams, DocumentFormattingParams,
    DocumentHighlight, DocumentHighlightParams, DocumentRangeFormattingParams,
    DocumentSymbolParams, DocumentSymbolResponse, FileChangeType, FileSystemWatcher, GlobPattern,
    GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverParams, HoverProviderCapability,
    InitializeParams, InitializeResult, InitializedParams, InlayHint, InlayHintParams, Location,
    MessageType, OneOf, ReferenceParams, Registration, SaveOptions, ServerCapabilities, ServerInfo,
    SignatureHelp, SignatureHelpOptions, SignatureHelpParams, TextDocumentSyncCapability,
    TextDocumentSyncKind, TextDocumentSyncOptions, TextDocumentSyncSaveOptions, TextEdit, Url,
};
use tower_lsp::{Client, LanguageServer};

/// Per-document state: the latest source we've been told about plus the
/// client's version counter. Stored verbatim so any part of the pipeline
/// (diagnostics today, formatting / hover later) can re-run against the
/// authoritative text without re-reading from disk or trusting whatever
/// snapshot the request happens to carry.
pub(crate) struct Document {
    pub(crate) source: String,
    pub(crate) version: i32,
}

pub struct Backend {
    pub(crate) client: Client,
    /// Open documents, keyed by URI string. `DashMap` lets independent
    /// per-file operations interleave without a global lock.
    pub(crate) docs: DashMap<String, Document>,
    /// Every `.sau` file discovered under any workspace root, whether
    /// or not it's currently open. Keyed by absolute canonical path —
    /// the same key shape we use for the import graph.
    pub(crate) workspace_files: DashMap<PathBuf, ()>,
    /// Reverse import graph: `target → set of importers`. When `target`
    /// changes we re-analyse every importer so cross-file type errors
    /// stay in sync without making the editor open every dependent.
    pub(crate) rev_imports: DashMap<PathBuf, HashSet<PathBuf>>,
    /// Workspace root directories supplied at `initialize` time. Used
    /// to locate `saule.config` and bound the recursive file scan.
    pub(crate) workspace_roots: Mutex<Vec<PathBuf>>,
    /// Cached project info (from `saule.config`) so we can re-install
    /// it on whatever tokio worker happens to be running each analysis
    /// — `saule_interpreter::project` uses thread-local state.
    pub(crate) project_info: Mutex<Option<saule_interpreter::project::ProjectInfo>>,
    /// Serialises the analyze→typeck phase across all documents — the
    /// thread-local registries those passes use are global per thread,
    /// so concurrent runs would race even on different files.
    pub(crate) analysis_lock: Mutex<()>,
    /// Memoised import seeds. Rebuilding one costs ~27ms against the
    /// `UI Project` sample and every request begins with it, so without
    /// this the editor lags a keystroke behind. See [`seed_cache`].
    pub(crate) seed_cache: seed_cache::SeedCache,
    /// Whether the client accepts a runtime `workspace/didChangeWatchedFiles`
    /// registration, captured from its `initialize` capabilities. There is
    /// no static server capability for file watching — the only way to ask
    /// for it is `client/registerCapability`, and sending that to a client
    /// that didn't advertise support is a protocol error.
    pub(crate) watched_files_dynamic: Mutex<bool>,
    /// Where each file's declarations lived the last time it parsed cleanly,
    /// keyed by URI string like [`Self::docs`]. See [`Self::syntax`].
    pub(crate) shapes: DashMap<String, saule_parser::PriorShape>,
}

impl Backend {
    /// The recovered tree for `source`, using what a previous clean parse of
    /// the same document knew about it — and refreshing that knowledge when
    /// this parse is itself clean.
    ///
    /// This is what closes the gap indentation can't. A file with no
    /// indentation offers no evidence of where a forgotten `end` belonged, so
    /// every declaration below it is parsed one scope too deep and drops out
    /// of the outline. Editing history says what whitespace can't: `after`
    /// was a top-level function a keystroke ago, so the edit that buried it
    /// was a deleted `end`, not a restructuring.
    ///
    /// Only a clean parse updates the shape. Learning from a recovered tree
    /// would feed the repair's own guesses back into it, and a wrong guess
    /// would then reinforce itself for the rest of the session.
    pub(crate) fn syntax(&self, uri: &Url, source: &str) -> saule_ast::Module {
        self.syntax_full(uri, source).0
    }

    /// [`Self::syntax`], keeping the diagnostics as well as the tree.
    pub(crate) fn syntax_full(
        &self,
        uri: &Url,
        source: &str,
    ) -> (saule_ast::Module, syntax::SyntaxErrors) {
        let key = uri.as_str();
        // Cloned rather than borrowed: the insert below would deadlock
        // against a live read guard on the same shard.
        let prior = self.shapes.get(key).map(|e| e.clone());
        let (module, errors) = syntax::analyze(source, prior.as_ref());
        if errors.is_empty() {
            self.shapes
                .insert(key.to_string(), saule_parser::PriorShape::of(&module));
        }
        (module, errors)
    }
}

impl Backend {
    /// A [`saule_interpreter::module::SourceOverlay`] backed by the open-document
    /// cache, for the import walk to consult ahead of the filesystem.
    ///
    /// Without it every cross-file lookup — diagnostics, hover, completion,
    /// signature help, inlay hints, goto-definition — read imported modules
    /// straight off disk, so an *unsaved* edit in an imported file was
    /// invisible to its importers. Change a method's signature in `storage.sau`
    /// and `main.sau` would keep reporting against the version last written to
    /// disk until you saved.
    ///
    /// `docs` is keyed by the URI string the client sent, which is not
    /// guaranteed to be the canonical spelling the import resolver produces
    /// (symlinked roots, `..` segments). The exact-URI hit is the fast path;
    /// the scan below is the correctness path, and it is bounded by the number
    /// of *open* editor buffers, not by workspace size.
    pub(crate) fn source_overlay(&self) -> impl Fn(&Path) -> Option<String> + '_ {
        move |path: &Path| {
            let want = canonical(path).unwrap_or_else(|| path.to_path_buf());
            if let Some(uri) = path_to_uri(&want)
                && let Some(doc) = self.docs.get(uri.as_str())
            {
                return Some(doc.source.clone());
            }
            self.docs.iter().find_map(|entry| {
                let doc_path = Url::parse(entry.key())
                    .ok()
                    .and_then(|u| u.to_file_path().ok())?;
                let doc_path = canonical(&doc_path).unwrap_or(doc_path);
                (doc_path == want).then(|| entry.source.clone())
            })
        }
    }

    /// The import seed for `module`, memoised.
    ///
    /// Every feature handler needs this and each was calling
    /// `collect_import_seed_with` directly — the single most expensive
    /// step in a request by two orders of magnitude. Route them all
    /// through here so one cache serves the lot.
    ///
    /// `uri` identifies the importing document; `dir` is the directory
    /// import paths resolve against.
    pub(crate) fn import_seed(
        &self,
        uri: &Url,
        module: &saule_ast::Module,
        dir: &Path,
    ) -> saule_semantic::ModuleSeed {
        let doc_path = uri.to_file_path().ok();
        self.import_seed_at(doc_path.as_deref(), module, dir)
    }

    /// [`Backend::import_seed`] for callers that already hold the
    /// document's path rather than its URI.
    ///
    /// The path is canonicalised here so both entry points agree on the
    /// cache key — otherwise the same file gets two entries and neither
    /// sees the other's invalidation.
    pub(crate) fn import_seed_at(
        &self,
        doc_path: Option<&Path>,
        module: &saule_ast::Module,
        dir: &Path,
    ) -> saule_semantic::ModuleSeed {
        let key = doc_path.map(|p| canonical(p).unwrap_or_else(|| p.to_path_buf()));
        self.seed_cache
            .seed_for(key.as_deref(), module, dir, self.source_overlay())
    }

    /// Ask the client to watch every `.sau` file and `saule.config` in
    /// the workspace and report changes made outside the editor.
    ///
    /// Best-effort by design: a client that never advertised dynamic
    /// registration is skipped, and a client that rejects the request is
    /// logged rather than treated as fatal. Everything degrades to the
    /// pre-watcher behaviour — stale until the file is opened — which is
    /// where the server was before this existed.
    async fn register_file_watchers(&self) {
        if !*self.watched_files_dynamic.lock().await {
            self.client
                .log_message(
                    MessageType::INFO,
                    "client does not support file watching; \
                     changes made outside the editor need a reopen to be seen",
                )
                .await;
            return;
        }

        let watchers = ["**/*.sau", "**/saule.config"]
            .into_iter()
            .map(|glob| FileSystemWatcher {
                glob_pattern: GlobPattern::String(glob.to_string()),
                // `None` means create + change + delete; all three matter
                // here, and for different reasons. See
                // `did_change_watched_files`.
                kind: None,
            })
            .collect();

        let options = DidChangeWatchedFilesRegistrationOptions { watchers };
        let Ok(register_options) = serde_json::to_value(options) else {
            return;
        };

        if let Err(e) = self
            .client
            .register_capability(vec![Registration {
                id: "saule-watched-files".to_string(),
                method: "workspace/didChangeWatchedFiles".to_string(),
                register_options: Some(register_options),
            }])
            .await
        {
            self.client
                .log_message(
                    MessageType::WARNING,
                    format!("file watch registration: {e}"),
                )
                .await;
        }
    }

    /// Re-read `saule.config` from the workspace roots.
    ///
    /// The config names the source directories imports resolve against,
    /// so a stale copy silently misroutes every cross-file lookup.
    async fn reload_project_info(&self) {
        let roots: Vec<PathBuf> = self.workspace_roots.lock().await.clone();
        for root in &roots {
            if let Some(project_root) = workspace::find_project_root(root)
                && let Some(info) = workspace::load_project(&project_root)
            {
                *self.project_info.lock().await = Some(info);
                return;
            }
        }
        // Config removed — drop what we had rather than keep resolving
        // against directories that are no longer declared.
        *self.project_info.lock().await = None;
    }

    pub fn new(client: Client) -> Self {
        Self {
            client,
            docs: DashMap::new(),
            workspace_files: DashMap::new(),
            rev_imports: DashMap::new(),
            workspace_roots: Mutex::new(Vec::new()),
            project_info: Mutex::new(None),
            analysis_lock: Mutex::new(()),
            seed_cache: seed_cache::SeedCache::default(),
            watched_files_dynamic: Mutex::new(false),
            shapes: DashMap::new(),
        }
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
        if roots.is_empty()
            && let Some(root_uri) = params.root_uri
            && let Ok(p) = root_uri.to_file_path()
        {
            roots.push(p);
        }
        *self.workspace_roots.lock().await = roots;

        // File watching has no static capability — it is requested at
        // runtime from `initialized`, and only from clients that said
        // they'd accept the registration.
        *self.watched_files_dynamic.lock().await = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|w| w.did_change_watched_files.as_ref())
            .and_then(|f| f.dynamic_registration)
            .unwrap_or(false);

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
                definition_provider: Some(OneOf::Left(true)),
                references_provider: Some(OneOf::Left(true)),
                document_highlight_provider: Some(OneOf::Left(true)),
                document_symbol_provider: Some(OneOf::Left(true)),
                inlay_hint_provider: Some(tower_lsp::lsp_types::OneOf::Left(true)),
                completion_provider: Some(tower_lsp::lsp_types::CompletionOptions {
                    // `.` opens member completion; the rest re-trigger after
                    // a type annotation or a new argument.
                    trigger_characters: Some(vec![
                        ".".to_string(),
                        ":".to_string(),
                        ">".to_string(),
                    ]),
                    resolve_provider: Some(false),
                    ..Default::default()
                }),
                signature_help_provider: Some(SignatureHelpOptions {
                    // `(` opens the popup, `,` and `:` (named-arg key
                    // separator) move between slots.
                    trigger_characters: Some(vec![
                        "(".to_string(),
                        ",".to_string(),
                        ":".to_string(),
                    ]),
                    // Trigger characters only fire while the popup is
                    // *closed*; once it's open the client re-queries only
                    // on a retrigger character. So every trigger has to
                    // appear here too, or a nested call never updates the
                    // popup — typing `f(g(` left it stuck on `f`, since
                    // the second `(` arrived with the popup already open.
                    // `)` is here for the way back out: closing `g(...)`
                    // puts the caret in `f`'s argument list again.
                    retrigger_characters: Some(vec![
                        "(".to_string(),
                        ")".to_string(),
                        ",".to_string(),
                        ":".to_string(),
                    ]),
                    work_done_progress_options: Default::default(),
                }),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "saule-lsp".to_string(),
                version: Some(saule_version::FULL.to_string()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "saule-lsp ready")
            .await;
        self.register_file_watchers().await;
        self.initial_workspace_scan().await;
    }

    /// Files changed on disk by something other than the editor — a
    /// `git checkout`, a generator, a rename in a file tree.
    ///
    /// Two things go stale that no document notification would catch.
    /// The import seed cache keys off the files a module's import walk
    /// read, so a *newly created* file is invisible to it: the walk
    /// never read the missing file, no read set mentions it, and a
    /// previously-unresolvable `import` would keep resolving to nothing.
    /// And `workspace_files` — which decides whether a closed file still
    /// gets diagnostics — was only ever filled by the one scan at
    /// startup.
    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        let mut structural = false;
        let mut touched: Vec<(PathBuf, Url)> = Vec::new();

        for event in &params.changes {
            // `saule.config` decides how imports resolve at all, so a
            // change to it invalidates every cached seed and the project
            // info the resolver reads.
            if event.uri.path().ends_with("saule.config") {
                self.reload_project_info().await;
                structural = true;
                continue;
            }
            let Some(abs) = event.uri.to_file_path().ok().and_then(|p| canonical(&p)) else {
                // A deleted path can't be canonicalised — it's gone. Fall
                // back to the raw path so removal still finds its entry.
                if let Ok(raw) = event.uri.to_file_path() {
                    self.workspace_files.remove(&raw);
                    self.seed_cache.invalidate_dependents_of(&raw);
                    structural = true;
                }
                continue;
            };

            match event.typ {
                FileChangeType::CREATED => {
                    self.workspace_files.insert(abs.clone(), ());
                    structural = true;
                }
                FileChangeType::DELETED => {
                    self.workspace_files.remove(&abs);
                    self.rev_imports.remove(&abs);
                    // The file's own diagnostics would otherwise linger
                    // in the editor forever, pointing at nothing.
                    self.client
                        .publish_diagnostics(event.uri.clone(), Vec::new(), None)
                        .await;
                    structural = true;
                }
                // Content changed underneath us. Anything whose seed read
                // this file is stale; the file itself is re-analysed
                // below. An *open* buffer is unaffected either way — the
                // overlay serves the editor's text, not the disk's.
                _ => self.seed_cache.invalidate_dependents_of(&abs),
            }
            if event.typ != FileChangeType::DELETED {
                touched.push((abs, event.uri.clone()));
            }
        }

        // A file appearing or vanishing changes which imports resolve,
        // and that is not expressible as "these files were read" — drop
        // the lot rather than guess.
        if structural {
            self.seed_cache.clear();
        }

        for (abs, uri) in touched {
            self.refresh_path(&abs, uri).await;
        }
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        // A file appearing can make a previously-unresolvable `import`
        // resolve. The seed walk never read the missing file, so no read
        // set mentions it and targeted invalidation can't see the
        // change — drop everything. Opening a file is rare enough that
        // paying one rebuild for it is free.
        self.seed_cache.clear();
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
        // The remembered shape describes an unsaved buffer that no longer
        // exists. Re-analysis from disk below rebuilds it from the file.
        self.shapes.remove(uri.as_str());
        // The overlay stops answering for this path, so importers now
        // see the on-disk text instead of the buffer — a different seed.
        if let Some(abs) = uri.to_file_path().ok().and_then(|p| canonical(&p)) {
            self.seed_cache.invalidate_dependents_of(&abs);
        }
        // Don't clear diagnostics — the file is still part of the
        // workspace. Re-analyse from disk so the Problems pane keeps
        // showing any errors that were live when the user closed it.
        if let Some(abs) = uri.to_file_path().ok().and_then(|p| canonical(&p))
            && self.workspace_files.contains_key(&abs)
        {
            self.refresh_path(&abs, uri).await;
            return;
        }
        // File isn't tracked (untitled buffer, outside workspace) —
        // safe to clear stale diagnostics.
        self.client.publish_diagnostics(uri, Vec::new(), None).await;
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        Ok(self
            .format_document(&params.text_document.uri, &params.options)
            .await)
    }

    /// Range formatting is implemented as full-document formatting — the
    /// Saule formatter is whole-module by design, so any partial-range
    /// request just re-emits the entire file. The client merges the
    /// returned edit normally.
    async fn range_formatting(
        &self,
        params: DocumentRangeFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>> {
        Ok(self
            .format_document(&params.text_document.uri, &params.options)
            .await)
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let p = params.text_document_position_params;
        Ok(self.hover_at(&p.text_document.uri, p.position).await)
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let p = params.text_document_position_params;
        let locs = self.definitions_at(&p.text_document.uri, p.position).await;
        if locs.is_empty() {
            return Ok(None);
        }
        Ok(Some(GotoDefinitionResponse::Array(locs)))
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let p = params.text_document_position;
        let include_def = params.context.include_declaration;
        let locs = self
            .references_at(&p.text_document.uri, p.position, include_def)
            .await;
        if locs.is_empty() {
            return Ok(None);
        }
        Ok(Some(locs))
    }

    async fn document_highlight(
        &self,
        params: DocumentHighlightParams,
    ) -> Result<Option<Vec<DocumentHighlight>>> {
        let p = params.text_document_position_params;
        let hls = self.highlights_at(&p.text_document.uri, p.position).await;
        if hls.is_empty() {
            return Ok(None);
        }
        Ok(Some(hls))
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        Ok(self
            .document_symbols(&params.text_document.uri)
            .await
            .map(DocumentSymbolResponse::Nested))
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        Ok(Some(self.inlay_hints(&params.text_document.uri).await))
    }

    async fn completion(
        &self,
        params: tower_lsp::lsp_types::CompletionParams,
    ) -> Result<Option<tower_lsp::lsp_types::CompletionResponse>> {
        let p = params.text_document_position;
        Ok(self.completion_at(&p.text_document.uri, p.position).await)
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        let p = params.text_document_position_params;
        let help = self
            .signature_help_at(&p.text_document.uri, p.position)
            .await;
        // Never hand back a longer signature list than the popup was
        // opened with — IntelliJ indexes its existing rows by it.
        let prev = params
            .context
            .as_ref()
            .and_then(|c| c.active_signature_help.as_ref());
        let help = match (help, prev) {
            (Some(fresh), Some(prev)) => Some(sighelp::reconcile_with_client(fresh, prev)),
            (help, _) => help,
        };
        self.trace_signature_help(&p.text_document.uri, p.position, &help)
            .await;
        Ok(help)
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

/// Canonicalise a path, falling back to the input on failure. Used as
/// the stable key shape for `workspace_files` and `rev_imports` so the
/// same file referenced via different relative spellings hashes equally.
pub(crate) fn canonical(p: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(p).ok()
}

/// Convert an absolute file path to a `file://` URI suitable for
/// `publishDiagnostics`. Returns `None` if the path isn't absolute or
/// contains invalid UTF-8 the URI crate rejects.
pub(crate) fn path_to_uri(p: &Path) -> Option<Url> {
    Url::from_file_path(p).ok()
}
