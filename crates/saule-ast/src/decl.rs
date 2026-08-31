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
        /// Generic type parameters declared with `<T, U>` after the name.
        ///
        /// Erased at runtime, like a function's. Inside the body each name is
        /// a rigid type standing for whatever the *user of the class* picked.
        type_params: Vec<String>,
        extends: Option<TypeRef>,
        implements: Vec<TypeRef>,
        members: Vec<Spanned<ClassMember>>,
    },
    Interface {
        exported: bool,
        name: String,
        /// Generic type parameters declared with `<T, U>` after the name.
        type_params: Vec<String>,
        extends: Vec<TypeRef>,
        methods: Vec<MethodSig>,
    },
    Enum {
        exported: bool,
        name: String,
        /// Generic type parameters declared with `<T, U>` after the name.
        ///
        /// A variant's payload may be typed by one (`Ok(value: T)`), which is
        /// what makes `enum Result<T>` worth having: the arm that matches
        /// `Ok` binds a real `T`, substituted for whatever the value's own
        /// type argument turned out to be.
        type_params: Vec<String>,
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

/// A reference to a named type in a declaration header — the `Animal` in
/// `class Dog extends Animal`, the `Repository<Player>` in
/// `implements Repository<Player>`.
///
/// A bare name carries an empty `args`, so the common non-generic case reads
/// the same as the `String` it replaced. Kept as its own struct rather than a
/// [`Type`] because these positions accept only a named type: `extends fn()`
/// or `implements table<integer>` are not things to represent.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeRef {
    pub name: String,
    pub args: Vec<Type>,
}

impl TypeRef {
    /// A reference with no type arguments.
    pub fn plain(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            args: Vec::new(),
        }
    }

    /// The [`Type`] this reference denotes: `Named` when bare, `Generic`
    /// when it carries arguments.
    pub fn to_type(&self) -> Type {
        if self.args.is_empty() {
            Type::Named(self.name.clone())
        } else {
            Type::generic(self.name.clone(), self.args.clone())
        }
    }
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
    /// Generic type parameters declared with `<T, U>` after the method name.
    /// A method's own parameters, distinct from the interface's — in
    /// `interface Seq<T> … fn mapTo<U>(f: fn(T) -> U) -> Seq<U>`, `T` belongs
    /// to the interface and `U` to this one method.
    pub type_params: Vec<String>,
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
