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
mod sighelp;
mod symbols;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use dashmap::DashMap;
use tokio::sync::Mutex;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DidSaveTextDocumentParams, DocumentFormattingParams, DocumentHighlight,
    DocumentHighlightParams, DocumentRangeFormattingParams, DocumentSymbolParams,
    DocumentSymbolResponse, GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverParams,
    HoverProviderCapability, InitializeParams, InitializeResult, InitializedParams, InlayHint,
    InlayHintParams, Location, MessageType, OneOf, ReferenceParams, SaveOptions,
    ServerCapabilities, ServerInfo, SignatureHelp, SignatureHelpOptions, SignatureHelpParams,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions,
    TextDocumentSyncSaveOptions, TextEdit, Url,
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
