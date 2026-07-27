//! Expression-tree nodes: literals, operators, postfix chains, lambdas,
//! `match`, plus the function-parameter shape that lambdas and declarations
//! share.

use std::sync::Arc;

use crate::{Spanned, Stmt, Type};

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    // Literals
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Nil,

    // Names
    Ident(String),
    Self_,

    // Operators
    Unary {
        op: UnaryOp,
        rhs: Box<Spanned<Expr>>,
    },
    Binary {
        op: BinOp,
        lhs: Box<Spanned<Expr>>,
        rhs: Box<Spanned<Expr>>,
    },

    // Postfix
    /// `obj.name`
    Member {
        obj: Box<Spanned<Expr>>,
        name: String,
    },
    /// `obj?.name`
    SafeMember {
        obj: Box<Spanned<Expr>>,
        name: String,
    },
    /// `obj[index]`
    Index {
        obj: Box<Spanned<Expr>>,
        index: Box<Spanned<Expr>>,
    },
    /// `f(a, b, c)`
    Call {
        callee: Box<Spanned<Expr>>,
        args: Vec<CallArg>,
    },
    /// `obj:method(a, b)`
    MethodCall {
        obj: Box<Spanned<Expr>>,
        method: String,
        args: Vec<CallArg>,
    },
    /// `x!`
    ForceUnwrap(Box<Spanned<Expr>>),

    /// `{a, b, c}` or `{name: "alice", "x y": 1, 42}` — array, map, and
    /// mixed table literals all share this single shape.
    Table(Vec<TableEntry>),

    // Lambdas
    Lambda {
        params: Vec<Param>,
        return_ty: Option<Type>,
        body: LambdaBody,
    },

    /// `match scrutinee case <pat> [when <guard>] then <expr-or-block> ... end`
    ///
    /// `match` is an expression: every arm evaluates to a value of the same
    /// type, and that's the value of the whole `match`. Used as a statement,
    /// the value is simply discarded. See the README for the full surface.
    Match {
        scrutinee: Box<Spanned<Expr>>,
        arms: Vec<MatchArm>,
    },

    /// `when(source):stage1(args):stage2(args)…` — colon-based piping
    /// ("Saule style"). Each stage is a free-function call where the
    /// previous stage's value is implicitly threaded in as the first
    /// argument. Lowering happens late (the typechecker and interpreter
    /// know how to walk a `Pipe`) so the formatter can round-trip the
    /// surface syntax faithfully.
    Pipe {
        source: Box<Spanned<Expr>>,
        stages: Vec<PipeStage>,
    },
}

/// One `:name(args)` step inside an [`Expr::Pipe`] chain.
#[derive(Debug, Clone, PartialEq)]
pub struct PipeStage {
    /// Free-function name invoked at this step. Resolved as a regular
    /// identifier (locals / globals / top-level `fn`).
    pub name: String,
    /// Extra positional/named arguments. The piped value is *prepended*
    /// to this list at call time, so the function's first parameter is
    /// always the upstream value.
    pub args: Vec<CallArg>,
    /// Span covering `:name(args)` for diagnostics.
    pub span: std::ops::Range<usize>,
}

/// One `case <pattern> [when <guard>] then <body>` clause inside a `match`.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    pub pattern: Spanned<Pattern>,
    /// Optional `when <expr>` guard — evaluated only after the pattern matches.
    pub guard: Option<Spanned<Expr>>,
    /// Arm body. Either a single expression (typical `case x then 1`) or a
    /// block of statements whose final value becomes the arm's value.
    pub body: MatchBody,
    pub span: std::ops::Range<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MatchBody {
    Expr(Spanned<Expr>),
    Block(Vec<Spanned<Stmt>>),
}

/// Patterns supported in `match` arms.
#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    /// `_` — matches anything, binds nothing.
    Wildcard,
    /// Lowercase identifier — matches anything, binds the value under that
    /// name in the arm's scope.
    Bind(String),
    /// `nil` — matches only the nil value.
    Nil,
    /// Literal pattern: `1`, `"x"`, `true`, `false`.
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    /// Enum variant: `Status.Ok` or `Event.Click(x, y)`.
    Variant {
        enum_name: String,
        variant: String,
        /// Optional payload sub-patterns (empty when matching a bare variant).
        fields: Vec<Spanned<Pattern>>,
    },
    /// Tuple destructuring: `(q, r)` — bound positionally from a multi-return
    /// scrutinee.
    Tuple(Vec<Spanned<Pattern>>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum CallArg {
    Positional(Spanned<Expr>),
    Named { name: String, value: Spanned<Expr> },
}

/// One entry inside a `{ ... }` table literal.
///
/// * `Positional` — appended to the array part with successive 1-based keys.
/// * `Field` — written into the map part. Both `name: expr` and `"text": expr`
///   parse the key into a `Spanned<Expr::Str>`; arbitrary computed keys are
///   not currently exposed in the surface but the shape leaves room for them.
#[derive(Debug, Clone, PartialEq)]
pub enum TableEntry {
    Positional(Spanned<Expr>),
    Field {
        key: Spanned<Expr>,
        value: Spanned<Expr>,
    },
}

/// The body of a lambda expression.
///
/// Held behind `Arc` rather than `Box`/`Vec` because evaluating a lambda
/// expression builds a runtime function object from this body, and a lambda
/// written inside a loop is evaluated once per iteration. Sharing means
/// that's a refcount bump instead of a deep copy of the whole body.
#[derive(Debug, Clone, PartialEq)]
pub enum LambdaBody {
    Expr(Arc<Spanned<Expr>>),
    Block(Arc<[Spanned<Stmt>]>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg, // `-x`
    Not, // `not x`
    Len, // `#x`
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    // Arithmetic
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    // Comparison
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    // Logical (short-circuit)
    And,
    Or,
    // Strings
    Concat,
    // Null-coalescing
    Coalesce,
}

/// Function/method/lambda parameter.
///
/// Shared between [`Expr::Lambda`], [`crate::Decl::Function`], [`crate::Method`],
/// and [`crate::EnumVariant::Tuple`] payloads.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub ty: Type,
    pub default: Option<Spanned<Expr>>,
    pub variadic: bool,
    pub span: std::ops::Range<usize>,
}
