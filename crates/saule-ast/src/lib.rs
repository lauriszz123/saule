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
//!
//! All public items are re-exported flat so downstream crates keep using
//! `saule_ast::{Type, Expr, Stmt, ...}` without caring about the split.

use std::ops::Range;

mod decl;
mod expr;
mod stmt;
mod types;

pub use decl::*;
pub use expr::*;
pub use stmt::*;
pub use types::*;

/// A value paired with the byte range it occupies in the source text.
/// Wraps every AST node that callers may want to highlight in diagnostics.
#[derive(Debug, Clone, PartialEq)]
pub struct Spanned<T> {
    pub value: T,
    pub span: Range<usize>,
}

impl<T> Spanned<T> {
    pub fn new(value: T, span: Range<usize>) -> Self {
        Self { value, span }
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
