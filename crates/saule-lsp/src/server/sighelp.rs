//! Signature help — shows the active call's signature with the
//! parameter under the cursor highlighted. Triggered by `(`, `,`, and
//! `:` (for named-argument keys).
//!
//! Implementation strategy:
//!
//! 1. Lex / parse the document. A buffer mid-edit usually *doesn't*
//!    parse — the call being typed has no `)` yet — so on failure
//!    [`repair_parse`] appends the missing closers and parses that.
//!    Appending never shifts the offsets before the cursor, so the
//!    same walk works on the repaired tree.
//! 2. Walk the AST to find the smallest `Expr::Call` / pipeline stage
//!    whose argument span (the `(...)` parens region) contains the
//!    cursor.
//! 3. Resolve the callee to a parameter list: free function, class
//!    constructor, method (on a local, field, parameter, or call
//!    result), sibling / static method, `self.super`, enum tuple
//!    variant, function-typed local, or a stdlib native.
//! 4. Compute `active_parameter` from how many `,`-separated
//!    arguments precede the cursor — or, for a `name: value`
//!    argument, from the slot its key names.
//! 5. Render `name(p1: T1, p2: T2, ...)` with the active param
//!    highlighted via `parameters[*]` ranges into the label string.
//!
//! [`textual_fallback`] remains as a last resort for buffers even the
//! repair can't parse; it resolves the callee by name from raw text.
mod render;
mod repair;
#[cfg(test)]
mod tests;
mod textual;
pub(crate) mod walk;

pub(crate) use render::*;
pub(crate) use repair::*;
pub(crate) use textual::*;
pub(crate) use walk::*;

use saule_ast::{Module, Param, Type};
use saule_semantic::{lookup_function, lookup_method, with_classes};
use tower_lsp::lsp_types::{Position, SignatureHelp, Url};

use crate::line_index::LineIndex;

use super::{Backend, canonical};

impl Backend {
    /// Resolve signature help at `pos` inside `uri`. Returns `None`
    /// when the cursor isn't inside any call's arg list, the callee
    /// can't be resolved to a known signature, or the document is
    /// closed / fails to parse.
    pub(super) async fn signature_help_at(
        &self,
        uri: &Url,
        pos: Position,
    ) -> Option<SignatureHelp> {
        let entry = self.docs.get(uri.as_str())?;
        let source = entry.source.clone();
        drop(entry);

        let line_index = LineIndex::new(&source);
        let offset = line_index.offset(&source, pos);

        // Seed registries from the last successfully-parsed view of
        // this module so identifier resolution works even when the
        // current edit is mid-keystroke and the parser fails.
        let module_dir = uri
            .to_file_path()
            .ok()
            .and_then(|p| canonical(&p))
            .and_then(|p| p.parent().map(|d| d.to_path_buf()));
        let _guard = self.analysis_lock.lock().await;
        if let Some(info) = self.project_info.lock().await.clone() {
            saule_interpreter::project::set(info);
        }

        let parsed = saule_lexer::Lexer::new(&source)
            .tokenize()
            .ok()
            .and_then(|tokens| saule_parser::parse(tokens).ok());

        if let Some(module) = parsed.as_ref() {
            let seed = match &module_dir {
                Some(d) => saule_interpreter::module::collect_import_seed_with(
                    module,
                    d,
                    &self.source_overlay(),
                ),
                None => saule_semantic::ModuleSeed::default(),
            };
            let _ = saule_semantic::analyze_with_seed(module, seed);

            match answer_from_module(module, &source, offset) {
                Answer::Help(help) => return Some(help),
                Answer::Suppressed => return None,
                Answer::Unresolved => {}
            }
        } else if let Some(module) = repair_parse(&source, offset) {
            // Mid-keystroke (`w.moveTo(`, `add(1, `): close the call the
            // user is typing and re-run the real walker. Everything is
            // appended at the end, so byte offsets up to the cursor —
            // and therefore the cursor itself — are unaffected.
            let seed = match &module_dir {
                Some(d) => saule_interpreter::module::collect_import_seed_with(
                    &module,
                    d,
                    &self.source_overlay(),
                ),
                None => saule_semantic::ModuleSeed::default(),
            };
            let _ = saule_semantic::analyze_with_seed(&module, seed);

            match answer_from_module(&module, &source, offset) {
                Answer::Help(help) => return Some(help),
                Answer::Suppressed => return None,
                Answer::Unresolved => {}
            }
        }

        // Last resort: the repair didn't parse either (unbalanced
        // brackets elsewhere, a broken string, ...). Scan the raw source
        // for the innermost unmatched `(` and resolve the call by name.
        textual_fallback(&source, offset).and_then(drop_parameterless)
    }
}

impl Backend {
    /// Log one line per signature-help request to the client's LSP
    /// console: the caret's line as *the server* has it, with `[|]`
    /// marking where the client said the caret is, and the signature we
    /// answered with.
    ///
    /// Worth its noise because the three ways this feature goes wrong —
    /// a stale document, a caret that isn't where it looks like it is,
    /// and a genuinely wrong resolution — are indistinguishable from a
    /// screenshot of the popup. Set `SAULE_LSP_TRACE=1` to enable.
    pub(super) async fn trace_signature_help(
        &self,
        uri: &Url,
        pos: Position,
        help: &Option<SignatureHelp>,
    ) {
        if std::env::var_os("SAULE_LSP_TRACE").is_none() {
            return;
        }
        let line = self
            .docs
            .get(uri.as_str())
            .and_then(|e| e.source.lines().nth(pos.line as usize).map(str::to_string))
            .unwrap_or_else(|| "<no such line>".into());
        // Insert the marker by UTF-16 column, the unit the client counts in.
        let units: Vec<u16> = line.encode_utf16().collect();
        let col = (pos.character as usize).min(units.len());
        let marked = format!(
            "{}[|]{}",
            String::from_utf16_lossy(&units[..col]),
            String::from_utf16_lossy(&units[col..])
        );
        let got = help
            .as_ref()
            .and_then(|h| h.signatures.first())
            .map(|s| s.label.as_str())
            .unwrap_or("<none>");
        self.client
            .log_message(
                tower_lsp::lsp_types::MessageType::INFO,
                format!(
                    "sighelp {}:{} -> {got}  ::  {marked}",
                    pos.line, pos.character
                ),
            )
            .await;
    }
}

fn resolve_hit(hit: CallHit, offset: usize) -> Option<SignatureHelp> {
    // How the call is headed in the popup: the dotted path the reader
    // wrote, so `Theme.of(...)` and `One.two.three(...)` name themselves
    // in full rather than collapsing to a bare `of` / `three`.
    let ml = hit.multiline;
    let shown =
        |fallback: &str| -> String { hit.display.clone().unwrap_or_else(|| fallback.to_string()) };
    match &hit.callee {
        CalleeRef::Free(name) => {
            let name = name.clone();
            let head = shown(&name);
            if with_classes(|r| r.contains_key(&name)) {
                return build_help(&head, lookup_method(&name, "init"), &hit.args, offset, ml);
            }
            if let Some(class) = &hit.enclosing_class
                && let Some(sig) = lookup_method(class, &name)
            {
                return build_help(&head, Some(sig), &hit.args, offset, ml);
            }
            // Callback held in a local / parameter: `f(...)` where
            // `f: fn(integer) -> string`. The type carries no parameter
            // names, so they're synthesised.
            if let Some((params, ret)) = hit.local_fn.clone() {
                let params: Vec<Param> = params
                    .into_iter()
                    .enumerate()
                    .map(|(i, ty)| Param {
                        name: format!("arg{i}"),
                        ty,
                        default: None,
                        variadic: false,
                        span: 0..0,
                    })
                    .collect();
                return build_help_user_fn(&head, &params, &Some(ret), &hit.args, offset, ml);
            }
            // User-defined top-level function (collected by the AST
            // walker into `hit.user_fn`).
            if let Some((params, ret)) = hit.user_fn.clone() {
                return build_help_user_fn(&head, &params, &ret, &hit.args, offset, ml);
            }
            // Declared in another file and imported — `showToast`,
            // `showDialog`, and every other helper a UI file reaches
            // through a barrel. `analyze_with_seed` folds imported
            // top-level functions into the semantic registry, following
            // re-export chains as it goes, so the signature is already
            // in hand and no import graph has to be walked here.
            if let Some(sig) = lookup_function(&name) {
                return build_help_user_fn(
                    &head,
                    &sig.params,
                    &sig.return_ty,
                    &hit.args,
                    offset,
                    ml,
                );
            }
            // Bare native (`println`, `assert`, ...).
            if let Some(native) = saule_typeck::sigs::lookup(&name) {
                return build_help_native(&head, &name, &native, &hit.args, offset, ml);
            }
            None
        }
        CalleeRef::Method { class, name } => {
            let head = shown(&format!("{class}.{name}"));
            if let Some(sig) = lookup_method(class, name) {
                return build_help(&head, Some(sig), &hit.args, offset, ml);
            }
            // Stdlib value-type instance method (e.g. `file.write` where
            // `file: File`). Native sigs are registered as `File.write`.
            let qname = format!("{class}.{name}");
            if let Some(native) = saule_typeck::sigs::lookup(&qname) {
                return build_help_native(&head, &qname, &native, &hit.args, offset, ml);
            }
            None
        }
        CalleeRef::SuperInit { owner } => build_help(
            &format!("{owner}.init"),
            lookup_method(owner, "init"),
            &hit.args,
            offset,
            ml,
        ),
        CalleeRef::Variant { display, fields } => {
            build_help_user_fn(display, fields, &None, &hit.args, offset, ml)
        }
        CalleeRef::PipeStage(name) => {
            // Resolve the stage like a free call, then drop the first
            // parameter — the pipeline supplies it.
            let sig = hit
                .enclosing_class
                .as_ref()
                .and_then(|c| lookup_method(c, name))
                .or_else(|| {
                    hit.user_fn
                        .clone()
                        .map(|(params, ret)| saule_semantic::MethodSig {
                            is_static: false,
                            is_private: false,
                            type_params: Vec::new(),
                            params,
                            return_ty: ret,
                        })
                })?;
            let piped = saule_semantic::MethodSig {
                params: sig.params.iter().skip(1).cloned().collect(),
                ..sig
            };
            build_help(name, Some(piped), &hit.args, offset, ml)
        }
        CalleeRef::Native(qname) => {
            let native = saule_typeck::sigs::lookup(qname)?;
            // The qualified name in full — `Os.exists`, not `exists`.
            let head = shown(qname);
            build_help_native(&head, qname, &native, &hit.args, offset, ml)
        }
    }
}

#[derive(Clone)]
pub(crate) enum CalleeRef {
    Free(String),
    Method {
        class: String,
        name: String,
    },
    /// `self.super(...)` — the `init` of `owner`, the nearest ancestor
    /// that declares one. Kept separate from `Method` so the rendered
    /// label can say `View.init` instead of a bare `init`.
    SuperInit {
        owner: String,
    },
    /// `Shape.Circle(...)` — a tuple-style enum variant used as a
    /// constructor. Its fields are `Param`s on the AST, so they're
    /// carried here directly rather than looked up in a registry.
    Variant {
        display: String,
        fields: Vec<Param>,
    },
    /// `when(x):stage(...)` — the piped value fills the first parameter,
    /// so the rendered signature drops it.
    PipeStage(String),
    /// Stdlib qualified name like `"Os.exists"` or bare native like
    /// `"println"` — resolved through `saule_typeck::sigs::lookup`.
    Native(String),
}

#[derive(Clone)]
pub(crate) struct CallArgInfo {
    span: std::ops::Range<usize>,
    /// Key of a `name: value` argument. Resolved to a parameter slot in
    /// [`build_help`], where the signature is known, so that typing
    /// inside `Widget(y: …)` highlights `y` rather than slot 0.
    name: Option<String>,
    /// Slot this argument fills, once resolved against the signature.
    named_index: Option<usize>,
}

pub(crate) struct CallHit {
    callee: CalleeRef,
    args: Vec<CallArgInfo>,
    enclosing_class: Option<String>,
    /// Resolved params for a user-defined free function — populated
    /// by the walker when it sees `Expr::Call` whose callee identifier
    /// matches a top-level `Decl::Function`.
    user_fn: Option<(Vec<Param>, Option<Type>)>,
    /// Param / return types when the callee identifier is a local or
    /// parameter of function type. Takes precedence over `user_fn`,
    /// matching the resolver's locals-shadow-globals rule.
    local_fn: Option<(Vec<Type>, Type)>,
    /// Source span of the entire arg list region — used to disambiguate
    /// nested calls (the smallest enclosing call wins).
    args_span: std::ops::Range<usize>,
    /// The callee as the reader wrote it — `add`, `Theme.of`,
    /// `One.two.three`. `None` when the callee isn't a plain dotted
    /// chain of names, and the resolved owner is used instead.
    display: Option<String>,
    /// Whether the call's argument list is spread over more than one
    /// line in the source. The rendered signature mirrors it, so a call
    /// written across lines reads as one.
    multiline: bool,
}

/// Render the signature of the one call the cursor is inside — the
/// innermost enclosing call that takes arguments.
///
/// Only that call. A nested widget expression like
/// `SizedBox(width: …, child: Align(alignment: …, child: ProgressBar(value: …)))`
/// contains three calls with parameters, and answering with all three
/// makes IntelliJ open three rows in the popup. Two of them describe
/// functions the caret is not in, and the reader has to work out which
/// row is theirs — which for Flutter-shaped code, where nesting is the
/// normal way to write anything, is most of the time.
///
/// Sibling calls the cursor is *not* inside are excluded even though
/// they sit in the same expression: `Alignment.centerLeft()` earlier on
/// the line has nothing to do with the argument being typed.
///
/// The cost is that the popup can go stale. LSP4IJ sets the row list
/// once, when the popup opens (`showParameterInfo` ->
/// `setItemsToShow`); a later `updateParameterInfo` re-requests but
/// feeds the response only into `setUIComponentEnabled` /
/// `setCurrentParameter`. So moving the caret from an outer call into a
/// nested one can leave the outer call's label on screen until the popup
/// is dismissed and reopened (Ctrl+P). That is the trade this function
/// makes deliberately: one row that is right when it opens, rather than
/// three rows one of which is right.
/// What the AST walk concluded at the cursor.
///
/// The three-way split exists because the handler has cruder strategies
/// to fall back on when this one comes up empty, and one of them —
/// [`textual_fallback`] — resolves by counting unmatched `(` in raw
/// source. It has no idea what a lambda is. So a deliberate "nothing
/// applies here" that reported a bare `None` came straight back as the
/// enclosing widget: inside `Switch(..., onChanged: fn(next: boolean)`
/// the `Switch(` paren is still unmatched at the caret, and the scanner
/// dutifully answered `Switch`. Suppression has to be stated, not
/// implied by absence.
enum Answer {
    Help(SignatureHelp),
    /// The cursor is inside a callback body. No enclosing call applies,
    /// and no less precise strategy should second-guess that.
    Suppressed,
    /// Nothing resolved. A fallback may still succeed.
    Unresolved,
}

/// [`answer_from_module`] for callers that treat every empty answer the
/// same way — the tests that assert *what* was reported rather than how
/// the handler routes an absence. The handler itself must not use this:
/// collapsing `Suppressed` into `None` is precisely the bug that let
/// [`textual_fallback`] re-report a call the walker had ruled out.
#[cfg(test)]
fn help_from_module(module: &Module, source: &str, offset: usize) -> Option<SignatureHelp> {
    match answer_from_module(module, source, offset) {
        Answer::Help(h) => Some(h),
        Answer::Suppressed | Answer::Unresolved => None,
    }
}

fn answer_from_module(module: &Module, source: &str, offset: usize) -> Answer {
    let mut cx = Cx {
        source,
        offset,
        locals: Vec::new(),
        enclosing_class: None,
        user_fns: collect_user_fns(module),
        enum_variants: collect_enum_variants(module),
        region: None,
        barrier: None,
        hits: Vec::new(),
    };
    cx.visit_module(module);

    // A caret inside a callback body or a table literal answers to that
    // region, not to the call it was written as an argument to. Calls
    // that opened before the region began are out of scope, however many
    // argument lists they still have lexically open.
    let mut hits = cx.hits;
    if let Some(b) = &cx.barrier {
        hits.retain(|h| h.args_span.start >= b.start);
    }

    // Innermost first: the narrowest argument list containing the cursor
    // is the call being typed. Walk outward from there so a level we
    // can't resolve, or one that takes no arguments, yields to its
    // parent instead of losing the popup — `update(dt: Timer.getDelta(|))`
    // still answers with `update`.
    hits.sort_by_key(|h| h.args_span.end.saturating_sub(h.args_span.start));

    for hit in hits {
        let Some(help) = resolve_hit(hit, offset) else {
            continue;
        };
        // A parameterless call has nothing to say, and IntelliJ renders
        // it as a literal `<no parameters>` row.
        let Some(sig) = help
            .signatures
            .into_iter()
            .find(|s| s.parameters.as_ref().is_some_and(|p| !p.is_empty()))
        else {
            continue;
        };
        let active_parameter = sig.active_parameter;
        return Answer::Help(SignatureHelp {
            signatures: vec![sig],
            active_signature: Some(0),
            active_parameter,
        });
    }
    // Inside a barrier with nothing of its own to report, the answer is
    // a definite no — not an invitation to go looking with a blunter
    // instrument.
    match cx.barrier {
        Some(_) => Answer::Suppressed,
        None => Answer::Unresolved,
    }
}

/// Reconcile a fresh response against what the client already shows.
///
/// IntelliJ creates one UI row per signature when the popup opens and
/// never rebuilds them, but LSP4IJ's `updateParameterInfo` indexes those
/// rows by the position of every *later* response
/// (`setUIComponentEnabled(i, …)`). A response with more signatures than
/// the popup was opened with therefore throws ArrayIndexOutOfBounds
/// inside the IDE.
///
/// So the list must never grow on a retrigger. When the fresh response
/// is longer, keep the client's own list and just move the selection to
/// the matching entry.
///
/// Since [`help_from_module`] answers with a single signature this no
/// longer fires in practice, and it is kept as the guard on an invariant
/// the IDE crashes on rather than as live logic.
pub(super) fn reconcile_with_client(fresh: SignatureHelp, prev: &SignatureHelp) -> SignatureHelp {
    if fresh.signatures.len() <= prev.signatures.len() {
        return fresh;
    }
    let active_label = fresh
        .active_signature
        .and_then(|i| fresh.signatures.get(i as usize))
        .map(|s| s.label.clone());
    let idx = active_label
        .and_then(|l| prev.signatures.iter().position(|s| s.label == l))
        .unwrap_or(0);
    let mut out = prev.clone();
    out.active_signature = Some(idx as u32);
    out.active_parameter = fresh.active_parameter;
    if let Some(s) = out.signatures.get_mut(idx) {
        s.active_parameter = fresh.active_parameter;
    }
    out
}
