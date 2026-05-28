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

use std::ops::Range;
use std::path::PathBuf;

use dashmap::DashMap;
use saule_fmt::{Comment, CommentKind};
use saule_lexer::Token;
use tokio::sync::Mutex;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::{
    Diagnostic, DiagnosticSeverity, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DidSaveTextDocumentParams, DocumentFormattingParams,
    DocumentRangeFormattingParams, InitializeParams, InitializeResult, InitializedParams,
    MessageType, OneOf, Position, SaveOptions, ServerCapabilities, ServerInfo, TextEdit,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions,
    TextDocumentSyncSaveOptions, Url,
};
use tower_lsp::{Client, LanguageServer};

use crate::line_index::LineIndex;

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

    /// Re-analyse `uri` from the cached source and publish diagnostics.
    /// No-op if the document isn't open (we only ever refresh files the
    /// client has told us about).
    async fn refresh(&self, uri: Url) {
        let Some(entry) = self.docs.get(uri.as_str()) else {
            return;
        };
        // Clone out so we don't hold the DashMap shard guard across the
        // analysis (which awaits on the lock) — guard isn't `Send` safe
        // across awaits and would deadlock a concurrent writer anyway.
        let source = entry.source.clone();
        let version = entry.version;
        drop(entry);

        let module_dir = uri
            .to_file_path()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()));
        let diagnostics = self.collect_diagnostics(&source, module_dir).await;
        self.client
            .publish_diagnostics(uri, diagnostics, Some(version))
            .await;
    }

    async fn collect_diagnostics(
        &self,
        source: &str,
        module_dir: Option<PathBuf>,
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
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
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
        // Clear any lingering diagnostics for the file.
        self.client.publish_diagnostics(uri, Vec::new(), None).await;
    }

    async fn formatting(
        &self,
        params: DocumentFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>> {
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
    let raw = saule_lexer::Lexer::new(source).tokenize_with_trivia().ok()?;

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
    Some(saule_fmt::format_module_with_comments(&module, source, &comments))
}
