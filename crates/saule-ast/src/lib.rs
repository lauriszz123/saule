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
}

/// A whole source file: a sequence of statements (which may be declarations).
#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub stmts: Vec<Spanned<Stmt>>,
}
