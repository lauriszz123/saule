//! AST node types and the generic [`Spanned<T>`] wrapper for the Saule
//! language.
//!
//! The module is split into:
//!
//! | File | Contents |
//! |------|----------|
//! | [`types`] | The [`Type`] enum |
//! | [`expr`]  | [`Expr`], [`Pattern`], [`MatchArm`]/[`MatchBody`], [`CallArg`], [`LambdaBody`], [`UnaryOp`]/[`BinOp`], [`Param`] |
//! | [`stmt`]  | The [`Stmt`] enum |
//! | [`decl`]  | [`Decl`], [`ClassMember`], [`Method`]/[`MethodSig`], [`EnumVariant`], [`ImportNames`] |
//! | [`ops`]   | Operator-overloading contracts (the built-in `Op*` interfaces) |
//!
//! All public items are re-exported flat so downstream crates keep using
//! `saule_ast::{Type, Expr, Stmt, ...}` without caring about the split.

use std::ops::Range;

mod decl;
mod expr;
mod ids;
pub mod ops;
mod stmt;
mod types;
mod visit;

pub use decl::*;
pub use expr::*;
pub use ids::assign_ids;
pub use stmt::*;
pub use types::*;
pub use visit::{Visitor, visit, visit_exprs};

/// A stable identity for an AST node, assigned by [`assign_ids`].
///
/// Spans are not usable as a key: two nodes can share one (a `Spanned<Expr>`
/// and the `Spanned<Stmt>` wrapping it), and error-recovered trees can carry
/// empty ones. Passes that want to publish a side table keyed by node —
/// `saule-typeck`'s type table, `saule-semantic`'s binding table, the
/// language server's caches — key it on this instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

/// Defaults to [`NodeId::NONE`], not to `NodeId(0)` — a derived default
/// would silently alias node zero.
impl Default for NodeId {
    fn default() -> Self {
        NodeId::NONE
    }
}

impl NodeId {
    /// The id every node carries until [`assign_ids`] runs. A side-table
    /// lookup on it must miss rather than collide, which is why it is
    /// `u32::MAX` and not `0`.
    pub const NONE: NodeId = NodeId(u32::MAX);

    pub fn is_none(self) -> bool {
        self == NodeId::NONE
    }

    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// A value paired with the byte range it occupies in the source text.
/// Wraps every AST node that callers may want to highlight in diagnostics.
#[derive(Debug, Clone)]
pub struct Spanned<T> {
    pub value: T,
    pub span: Range<usize>,
    /// Stable node identity. [`NodeId::NONE`] until [`assign_ids`] runs.
    pub id: NodeId,
}

/// Equality **deliberately ignores `id`**.
///
/// This impl is hand-written rather than derived for exactly one reason: the
/// parser's tests compare a parsed tree against a hand-built one, and a
/// hand-built tree has no ids. Deriving over the new field would fail every
/// one of those comparisons for a difference that carries no meaning — two
/// nodes with the same value and the same span *are* the same node as far as
/// anything outside a side table is concerned.
impl<T: PartialEq> PartialEq for Spanned<T> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value && self.span == other.span
    }
}

impl<T: Eq> Eq for Spanned<T> {}

impl<T> Spanned<T> {
    pub fn new(value: T, span: Range<usize>) -> Self {
        Self {
            value,
            span,
            id: NodeId::NONE,
        }
    }

    /// Convert this node's byte range into a `miette::SourceSpan` for
    /// diagnostic emission. Convenience wrapper around [`to_source_span`].
    pub fn source_span(&self) -> miette::SourceSpan {
        to_source_span(self.span.clone())
    }
}

/// Convert a byte-range span into a `miette::SourceSpan`. The single
/// canonical conversion used by every compiler stage (lexer, parser,
/// typeck, interpreter) when handing spans to miette.
///
/// `Range::end` is exclusive, while `SourceSpan` carries `(offset, len)`;
/// `saturating_sub` keeps an inverted/empty range from underflowing.
pub fn to_source_span(r: Range<usize>) -> miette::SourceSpan {
    (r.start, r.end.saturating_sub(r.start)).into()
}

/// A whole source file: a sequence of statements (which may be declarations).
#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub stmts: Vec<Spanned<Stmt>>,
}
