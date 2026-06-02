//! Symbol resolution + reference collection used by goto-definition
//! and find-references.
//!
//! Two passes share the same AST walk shape:
//!
//! * [`find_symbol_at`] — given a byte offset, identify what semantic
//!   symbol (class, method, field, function, enum variant, local,
//!   import path) the cursor is on.
//!
//! * [`collect_in_module`] — walk a module and emit every span where a
//!   given [`Symbol`] is defined or referenced.
//!
//! The AST stores names as bare `String`s without inner spans, so name
//! locations are recovered by scanning the source within the parent
//! node's span (e.g. `class Foo` → locate `Foo` after `class`). This is
//! good enough because identifier names parse as a single token whose
//! position inside a known structural span is unambiguous.

mod collect;
mod resolve;
#[cfg(test)]
mod tests;
mod util;

use std::ops::Range;

use saule_ast::Module;

// ──────────────────────────────────────────────────────────────────────────────
// Symbols
// ──────────────────────────────────────────────────────────────────────────────

/// A semantic symbol identified by name + kind.
///
/// Workspace-scoped variants (everything except `Local`) drive a
/// cross-file walk; `Local` is bounded to the file in which the
/// declaration lives and identified by the declaration's byte span so
/// shadowing inner-scope variables of the same name don't get
/// conflated.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Symbol {
    Class(String),
    Interface(String),
    Enum(String),
    EnumVariant {
        enum_name: String,
        variant: String,
    },
    Function(String),
    Method {
        class: String,
        name: String,
    },
    Field {
        class: String,
        name: String,
    },
    Local {
        name: String,
        def_span: Range<usize>,
    },
    ImportPath(String),
}

impl Symbol {
    /// `true` when this symbol can have references in other files.
    #[allow(dead_code)]
    pub fn is_workspace(&self) -> bool {
        !matches!(self, Symbol::Local { .. })
    }
}

/// Cursor → symbol resolution result.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub symbol: Symbol,
    /// Source span of the identifier (or import path literal) that the
    /// cursor was inside.
    pub span: Range<usize>,
}

/// One reference site emitted by [`collect_in_module`].
#[derive(Debug, Clone)]
pub struct Hit {
    pub span: Range<usize>,
    /// `true` if this hit is the symbol's defining occurrence (class
    /// header, method declaration, parameter declaration, etc.).
    pub is_def: bool,
}

// ──────────────────────────────────────────────────────────────────────────────
// Public entry points
// ──────────────────────────────────────────────────────────────────────────────

/// Resolve the cursor at `offset` to a [`Symbol`]. Returns `None` for
/// positions on whitespace / literals / unknown identifiers.
pub fn find_symbol_at(module: &Module, source: &str, offset: usize) -> Option<Resolved> {
    resolve::run(module, source, offset)
}

/// Walk `module` and emit every span that defines or references
/// `symbol`. The order follows the AST traversal — the LSP layer
/// canonicalises duplicates if any.
pub fn collect_in_module(module: &Module, source: &str, symbol: &Symbol) -> Vec<Hit> {
    collect::run(module, source, symbol)
}
