//! Inlay hint provider — emits two kinds of hints:
//!
//! * **Type hints on locals** — `local x = Foo()` → `x : Foo` ghost
//!   text after the local's name. Only fires when the right-hand side
//!   has a confidently-inferrable named type (constructor call,
//!   string/integer/float/bool literal, identifier referencing a
//!   typed local). This deliberately stays narrow: false-positive
//!   inlay hints are visually loud and we'd rather skip than mislead.
//!
//! * **Parameter-name hints on positional call args** — `add(2, 3)` →
//!   `add(a: 2, b: 3)` ghost labels in front of each argument so the
//!   reader sees what slot they're filling. Suppressed when the arg
//!   expression already mentions the param name (`add(a, b)` shouldn't
//!   show `a: a`) and for trivially-named single-arg calls
//!   (`println("..."`)).
//!
//! Hints are produced from the same thread-local class / interface /
//! enum registries the hover walker uses, so they reflect imports
//! seeded by `analyze_with_seed`.

mod infer;
mod visit;

use saule_ast::{CallArg, Decl, Expr, Module, Param, Stmt, Type};
use std::collections::HashMap;
use tower_lsp::lsp_types::{InlayHint, InlayHintKind, InlayHintLabel, Url};

use crate::line_index::LineIndex;

use super::{Backend, canonical};

impl Backend {
    /// Compute inlay hints for `uri`. Re-runs analysis under the shared
    /// lock so the registries reflect the current document. Returns an
    /// empty vec on lex / parse failure or for closed documents.
    pub(super) async fn inlay_hints(&self, uri: &Url) -> Vec<InlayHint> {
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
            Some(d) => saule_interpreter::module::collect_import_seed_with(
                &module,
                d,
                &self.source_overlay(),
            ),
            None => saule_semantic::ModuleSeed::default(),
        };
        let _ = saule_semantic::analyze_with_seed(&module, seed);

        let mut cx = Cx {
            source: &source,
            out: Vec::new(),
            locals: Vec::new(),
            enclosing_class: None,
            user_fns: collect_user_fns(&module),
        };
        cx.visit_module(&module);
        cx.out
            .into_iter()
            .map(|raw| InlayHint {
                position: line_index.position(&source, raw.byte),
                label: InlayHintLabel::String(raw.label),
                kind: Some(raw.kind),
                text_edits: None,
                tooltip: None,
                padding_left: raw.padding_left,
                padding_right: raw.padding_right,
                data: None,
            })
            .collect()
    }
}

struct RawHint {
    byte: usize,
    label: String,
    kind: InlayHintKind,
    padding_left: Option<bool>,
    padding_right: Option<bool>,
}

struct Local {
    name: String,
    ty: Type,
}

struct Cx<'a> {
    #[allow(dead_code)]
    source: &'a str,
    out: Vec<RawHint>,
    locals: Vec<Local>,
    enclosing_class: Option<String>,
    /// Top-level user functions discovered in the module, populated by
    /// `collect_user_fns` before traversal so call sites resolve
    /// regardless of declaration order.
    user_fns: HashMap<String, UserFn>,
}

/// A top-level user function, in the two shapes the hint passes need:
/// its declared parameters (for argument-name hints) and a
/// [`NativeSig`](saule_typeck::sigs::NativeSig) view so a call's return
/// type can be instantiated against the actual argument types.
struct UserFn {
    params: Vec<Param>,
    sig: saule_typeck::sigs::NativeSig,
}

impl crate::exprty::TypeSource for Cx<'_> {
    fn type_of(&self, expr: &Expr) -> Option<Type> {
        self.infer_type(expr)
    }

    fn stage_sig(&self, name: &str) -> Option<saule_typeck::sigs::NativeSig> {
        self.user_fns
            .get(name)
            .map(|f| f.sig.clone())
            .or_else(|| saule_typeck::sigs::lookup(name))
    }

    fn arg_types(&self, args: &[CallArg]) -> Vec<Option<Type>> {
        self.positional_arg_types(args)
    }
}

/// Either a list of AST `Param`s (which may have explicit names and
/// types) or a list of synthesised names from a native sig together
/// with a flag marking whether the trailing slot is variadic. The
/// two shapes are kept apart so the caller can decide whether to
/// consult `param.name` and `param.variadic` (only meaningful for
/// named params).
enum CalleeParams {
    Named(Vec<Param>),
    Native {
        names: Vec<String>,
        has_variadic: bool,
    },
}

/// Pre-pass: collect every top-level `fn name(...)` declaration so the
/// param-hint walker can resolve free-call expressions whose target
/// is a user-defined function (not a class init, not a stdlib native).
fn collect_user_fns(module: &Module) -> HashMap<String, UserFn> {
    let mut out = HashMap::new();
    for s in &module.stmts {
        if let Stmt::Decl(d) = &s.value
            && let Decl::Function {
                name,
                type_params,
                params,
                return_ty,
                ..
            } = &d.value
        {
            out.insert(
                name.clone(),
                UserFn {
                    params: params.clone(),
                    sig: saule_typeck::sigs::NativeSig {
                        type_params: type_params.clone(),
                        params: params.iter().map(|p| p.ty.clone()).collect(),
                        variadic: None,
                        returns: return_ty.clone().into_iter().collect(),
                    },
                },
            );
        }
    }
    out
}

/// Render a `Type` as a short label suitable for an inlay hint. Returns
/// `None` for types we don't want to surface (`any` is noise; unnamed
/// fallbacks would be misleading).
fn render_type(ty: &Type) -> Option<String> {
    match ty {
        Type::Named(n) if n == "any" || n.is_empty() => None,
        Type::Named(n) => Some(n.clone()),
        Type::Nullable(inner) => render_type(inner).map(|s| format!("{s}?")),
        // Element types are worth showing — `local xs = map(names, …)`
        // reads much better as `xs : table<integer>` than as nothing at
        // all. Rendered only when every component renders, so an `any`
        // anywhere inside still suppresses the whole hint rather than
        // spending screen width on `table<any>`.
        Type::Table { key: None, value } => render_type(value).map(|v| format!("table<{v}>")),
        Type::Table {
            key: Some(key),
            value,
        } => match (render_type(key), render_type(value)) {
            (Some(k), Some(v)) => Some(format!("table<{k}, {v}>")),
            _ => None,
        },
        _ => None,
    }
}

/// Peel one `Nullable` wrapper — what `!` asserts, and what a safe
/// chain re-applies over the field it reads.
fn strip_nullable(ty: Type) -> Type {
    match ty {
        Type::Nullable(inner) => *inner,
        other => other,
    }
}

/// Collapse an inferred type to the single value it yields in a single-value
/// context: a multi-return tuple becomes its first component, anything else
/// passes through, and `None` becomes `any`.
fn first_value_type(ty: Option<Type>) -> Type {
    match ty {
        Some(Type::Tuple(parts)) => parts
            .into_iter()
            .next()
            .unwrap_or_else(|| Type::Named("any".into())),
        Some(t) => t,
        None => Type::Named("any".into()),
    }
}

#[cfg(test)]
mod tests;
