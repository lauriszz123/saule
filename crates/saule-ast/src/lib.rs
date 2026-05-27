//! AST node types and the generic `Spanned<T>` wrapper for the Saule language.

use std::ops::Range;

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

// ──────────────────────────────────────────────────────────────────────────────
// Types
// ──────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    /// `integer`, `string`, `Player`, ...
    Named(String),
    /// `T?`
    Nullable(Box<Type>),
    /// `table<T>` (array-style, key implicit `integer`) when `key` is `None`;
    /// `table<K, V>` (hashmap-style) when `key` is `Some(K)`.
    Table {
        key: Option<Box<Type>>,
        value: Box<Type>,
    },
    /// `(A, B, C)` — currently used primarily for multi-return signatures.
    Tuple(Vec<Type>),
    /// `fn(A, B): R`
    Function { params: Vec<Type>, ret: Box<Type> },
}

// ──────────────────────────────────────────────────────────────────────────────
// Expressions
// ──────────────────────────────────────────────────────────────────────────────

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

    /// `{a, b, c}` — array-style table literal
    Table(Vec<Spanned<Expr>>),

    // Lambdas
    Lambda {
        params: Vec<Param>,
        return_ty: Option<Type>,
        body: LambdaBody,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum CallArg {
    Positional(Spanned<Expr>),
    Named { name: String, value: Spanned<Expr> },
}

#[derive(Debug, Clone, PartialEq)]
pub enum LambdaBody {
    Expr(Box<Spanned<Expr>>),
    Block(Vec<Spanned<Stmt>>),
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

// ──────────────────────────────────────────────────────────────────────────────
// Function parameters
// ──────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub ty: Type,
    pub default: Option<Spanned<Expr>>,
    pub variadic: bool,
    pub span: std::ops::Range<usize>,
}

// ──────────────────────────────────────────────────────────────────────────────
// Statements
// ──────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    /// `local x: T = expr`
    Local {
        name: String,
        ty: Option<Type>,
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

// ──────────────────────────────────────────────────────────────────────────────
// Declarations
// ──────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Decl {
    Function {
        exported: bool,
        name: String,
        params: Vec<Param>,
        return_ty: Option<Type>,
        body: Vec<Spanned<Stmt>>,
    },
    Class {
        exported: bool,
        name: String,
        extends: Option<String>,
        implements: Vec<String>,
        members: Vec<Spanned<ClassMember>>,
    },
    Interface {
        exported: bool,
        name: String,
        extends: Vec<String>,
        methods: Vec<MethodSig>,
    },
    Enum {
        exported: bool,
        name: String,
        variants: Vec<Spanned<EnumVariant>>,
        methods: Vec<Method>,
    },
    Import {
        names: ImportNames,
        path: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ImportNames {
    /// `import * from "path"`
    All,
    /// `import A, B as C, D from "path"`
    List(Vec<(String, Option<String>)>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClassMember {
    Field {
        is_static: bool,
        is_private: bool,
        name: String,
        ty: Type,
        default: Option<Spanned<Expr>>,
    },
    Method(Method),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Method {
    pub is_static: bool,
    pub is_private: bool,
    pub name: String,
    pub params: Vec<Param>,
    pub return_ty: Option<Type>,
    pub body: Vec<Spanned<Stmt>>,
    pub span: std::ops::Range<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MethodSig {
    pub name: String,
    pub params: Vec<Param>,
    pub return_ty: Option<Type>,
    pub span: std::ops::Range<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EnumVariant {
    /// `North`
    Bare(String),
    /// `Alive = "alive"`
    Valued(String, Spanned<Expr>),
}

// ──────────────────────────────────────────────────────────────────────────────
// Top-level module
// ──────────────────────────────────────────────────────────────────────────────

/// A whole source file: a sequence of statements (which may be declarations).
#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub stmts: Vec<Spanned<Stmt>>,
}
