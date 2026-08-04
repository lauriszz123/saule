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
    /// `x!`
    ForceUnwrap(Box<Spanned<Expr>>),

    /// `x as T` — a **checked** downcast out of `any`.
    ///
    /// The sole escape hatch from `any`, and the reason `any` can be sound:
    /// values enter `any` freely, but nothing leaves it without a runtime
    /// type test. Evaluates to `T?` — the value when it really is a `T`,
    /// `nil` otherwise — so it composes with the existing nullable
    /// machinery (`x as integer ?? 0`, `(x as integer)!`) instead of
    /// introducing a second failure mode.
    ///
    /// Legal only when the operand is `any` or `any?`; anywhere else the
    /// cast is redundant and the typechecker says so.
    Cast {
        value: Box<Spanned<Expr>>,
        ty: Type,
    },

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

impl CallArg {
    /// Whether this argument has the shape a **trailing block** desugars to:
    /// a positional, block-bodied lambda.
    ///
    /// `f(a) do … end` is sugar for `f(a, fn() … end)` — the parser produces
    /// the same tree for both, so this is a shape test, not a flag. Only the
    /// *final* argument of a call is a trailing block; see
    /// [`resolve_arg_slots`] for the binding rule that follows from it.
    pub fn is_trailing_block(&self) -> bool {
        matches!(
            self,
            CallArg::Positional(e)
                if matches!(&e.value, Expr::Lambda { body: LambdaBody::Block(_), .. })
        )
    }
}

/// Resolve which parameter slot each call argument fills.
///
/// Positional arguments consume slots left to right; a named argument targets
/// the slot carrying its name. The one exception is a **trailing block** — the
/// final argument when it is a positional block-bodied lambda, which is what
/// `f(x) do … end` produces. It binds to the callee's **last parameter**,
/// wherever the arguments before it landed, and it is the only positional
/// argument allowed to follow named ones.
///
/// Binding to the last parameter rather than to the next free slot is what
/// makes the form work with defaults in between:
///
/// ```text
/// fn panel(title: string, spacing: integer = 0, body: fn() -> nil) -> nil
///
/// panel(title: "Stats") do … end     -- block → `body`, `spacing` defaults
/// panel("Stats", 2) do … end         -- block → `body`, spacing → 2
/// ```
///
/// Filling the next free slot would put the block in `spacing` and report
/// `body` as missing. Swift and Kotlin resolve trailing closures the same way.
///
/// `param_names` is the callee's parameter list in declaration order. A slot
/// past its end, or a name matching no parameter, yields `None` — reporting
/// arity, duplicate and unknown-name errors is the caller's job. In particular
/// a trailing block can land on a slot a named argument already claimed
/// (`view(body: f) do … end`), which is a duplicate for the caller to report.
pub fn resolve_arg_slots(args: &[CallArg], param_names: &[&str]) -> Vec<Option<usize>> {
    let trailing = args
        .len()
        .checked_sub(1)
        .filter(|&i| args[i].is_trailing_block());

    let mut next = 0usize;
    args.iter()
        .enumerate()
        .map(|(idx, a)| match a {
            CallArg::Named { name, .. } => param_names.iter().position(|n| n == name),
            CallArg::Positional(_) if Some(idx) == trailing => param_names.len().checked_sub(1),
            CallArg::Positional(_) => {
                let i = next;
                next += 1;
                (i < param_names.len()).then_some(i)
            }
        })
        .collect()
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
    Pow,
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
