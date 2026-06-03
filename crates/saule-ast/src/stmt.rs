//! Statement nodes — everything that can appear in a block body.

use std::ops::Range;

use crate::{Decl, Expr, Spanned, Type};

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    /// `local x: T = expr`
    Local {
        name: String,
        /// Byte range of the declaring identifier (`x` above) in the
        /// source. Used by tooling (hover, go-to-definition) to point
        /// at the binding name itself rather than the whole statement.
        name_span: Range<usize>,
        ty: Option<Type>,
        /// Byte range of the type ascription (`T` above), if one was
        /// written. Covers from the start of the type to the end of
        /// any trailing `?` suffix. `None` when the binding has no
        /// annotation.
        ty_span: Option<Range<usize>>,
        value: Option<Spanned<Expr>>,
    },
    /// `local a: T, b: U = e1, e2` — parallel binding. Extra names are nil,
    /// extra values are dropped.
    LocalMulti {
        names: Vec<(String, Option<Type>)>,
        values: Vec<Spanned<Expr>>,
    },
    /// `lhs = rhs` (lhs is restricted to an assignable expression)
    Assign {
        target: Spanned<Expr>,
        value: Spanned<Expr>,
    },
    /// `a, b = e1, e2` — RHS is evaluated entirely before any assignment,
    /// so `a, b = b, a` swaps.
    AssignMulti {
        targets: Vec<Spanned<Expr>>,
        values: Vec<Spanned<Expr>>,
    },
    /// Expression used as a statement (typically a call).
    Expr(Spanned<Expr>),

    If {
        cond: Spanned<Expr>,
        then_block: Vec<Spanned<Stmt>>,
        elseifs: Vec<(Spanned<Expr>, Vec<Spanned<Stmt>>)>,
        else_block: Option<Vec<Spanned<Stmt>>>,
    },
    While {
        cond: Spanned<Expr>,
        body: Vec<Spanned<Stmt>>,
    },
    Repeat {
        body: Vec<Spanned<Stmt>>,
        cond: Spanned<Expr>,
    },
    /// `for i: integer = from to to [step step] do ... end`
    ForNumeric {
        var: String,
        var_ty: Option<Type>,
        from: Spanned<Expr>,
        to: Spanned<Expr>,
        step: Option<Spanned<Expr>>,
        body: Vec<Spanned<Stmt>>,
    },
    /// `for v: T in iter do ... end` or `for i: int, v: T in iter do ... end`
    ForIn {
        vars: Vec<(String, Option<Type>)>,
        iter: Spanned<Expr>,
        body: Vec<Spanned<Stmt>>,
    },

    Return(Vec<Spanned<Expr>>),
    Throw(Spanned<Expr>),
    Try {
        body: Vec<Spanned<Stmt>>,
        catch_var: String,
        catch_ty: Type,
        catch_body: Vec<Spanned<Stmt>>,
    },

    Break,
    Continue,

    /// Nested declaration (function / class / interface / enum / import).
    Decl(Spanned<Decl>),
}
