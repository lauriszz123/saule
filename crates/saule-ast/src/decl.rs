//! Top-level declaration shapes: `fn`, `class`, `interface`, `enum`, `import`,
//! plus the class-member and enum-variant nodes those use.

use crate::{Expr, Param, Spanned, Stmt, Type};

#[derive(Debug, Clone, PartialEq)]
pub enum Decl {
    Function {
        exported: bool,
        /// `true` when declared with the `local` qualifier inside a block
        /// (`local fn name(...) ... end`). Only meaningful for the
        /// pretty-printer; semantically `local fn` is identical to a
        /// non-exported `fn`.
        is_local: bool,
        name: String,
        /// Generic type parameters declared with `<T, U>` after the name.
        ///
        /// Erased at runtime. Inside the body the typechecker treats each
        /// name as *rigid*: it stands for whatever the caller picked, so
        /// it matches only itself. Widening into `any` is free; narrowing
        /// to a concrete type needs the checked `as`.
        type_params: Vec<String>,
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
    /// `export name: T = expr` — a module-level variable published to
    /// importers, the module-scope counterpart of a class's public field.
    ///
    /// Only the exported spelling reaches this node: an unexported
    /// module-level binding is written `local name = expr` and stays a
    /// [`Stmt::Local`].
    Variable {
        exported: bool,
        name: String,
        /// Byte range of the declared name, for tooling (hover,
        /// go-to-definition) that wants the identifier rather than the
        /// whole statement.
        name_span: std::ops::Range<usize>,
        ty: Option<Type>,
        /// Byte range of the type ascription, when one was written.
        ty_span: Option<std::ops::Range<usize>>,
        value: Option<Spanned<Expr>>,
    },
    Import {
        names: ImportNames,
        /// The module path with `.` / `/` separators, e.g. `some.folder.module`
        /// or `entities/Player`. Stored without surrounding quotes either way.
        path: String,
        /// How the path was spelled: `true` for `from "some/path"`, `false`
        /// for the unquoted `from some.folder.module`. Purely cosmetic — it
        /// only exists so the formatter can preserve the author's style.
        quoted: bool,
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
    /// Generic type parameters declared with `<T, U>` after the method name.
    pub type_params: Vec<String>,
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
    /// `Alive = "alive"` — single payload value set at decl time, exposed
    /// via `.value`. Treated as a singleton: every reference to the variant
    /// returns the same value.
    Valued(String, Spanned<Expr>),
    /// `Click(x: integer, y: integer)` — tuple-style payload variant. The
    /// fields' types are recorded for the typechecker; at runtime each call
    /// `Click(10, 20)` constructs a fresh variant instance whose payload is
    /// an array-style table of the positional arguments.
    Tuple { name: String, fields: Vec<Param> },
}
