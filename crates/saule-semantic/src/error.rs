//! Semantic diagnostics — issues that don't depend on types but require
//! understanding the whole program (declared names, scopes, control flow,
//! field initialization). Emitted by [`crate::analyze`] before typechecking.

use miette::Diagnostic;
use thiserror::Error;

#[derive(Debug, Error, Diagnostic)]
pub enum SemanticError {
    #[error("field `{field}` of class `{class}` is never initialized")]
    #[diagnostic(help(
        "assign `self.{field} = ...` in `init`, give the field a default value, or mark it nullable with `?`"
    ))]
    FieldNotInitialized {
        class: String,
        field: String,
        #[label("declared here")]
        span: miette::SourceSpan,
    },

    #[error("static field `{field}` of class `{class}` is never initialized")]
    #[diagnostic(help(
        "give it a value in the declaration (`static local {field}: ... = ...`) or mark the type nullable with `?` — a static has no `init` to assign it in"
    ))]
    StaticFieldNotInitialized {
        class: String,
        field: String,
        #[label("declared without a value")]
        span: miette::SourceSpan,
    },

    #[error("`{which}` is only valid inside a loop")]
    #[diagnostic(help("move this inside a `for`, `while`, or `repeat` loop"))]
    LoopControlOutsideLoop {
        which: &'static str,
        #[label("not inside a loop")]
        span: miette::SourceSpan,
    },

    #[error("undefined name `{name}` — it is not declared in this scope")]
    #[diagnostic(help("declare it with `local {name} = ...`, import it, or check the spelling"))]
    UndefinedName {
        name: String,
        #[label("not defined")]
        span: miette::SourceSpan,
    },

    #[error("cannot assign to undeclared variable `{name}`")]
    #[diagnostic(help("use `local {name} = ...` to declare it first"))]
    AssignToUndeclared {
        name: String,
        #[label("not declared")]
        span: miette::SourceSpan,
    },

    #[error("`self` is only valid inside a method body")]
    #[diagnostic(help(
        "`self` refers to the receiving instance of a method and isn't available at module scope"
    ))]
    SelfOutsideClass {
        #[label("not inside a method")]
        span: miette::SourceSpan,
    },

    #[error("`super` is only valid inside an instance method of a class with a parent")]
    #[diagnostic(help(
        "`super.member` and `self.super(...)` are only meaningful inside a subclass's methods"
    ))]
    SuperOutsideClass {
        #[label("not inside a method")]
        span: miette::SourceSpan,
    },

    #[error("`self.super(...)` is only valid inside the `init` constructor of a subclass")]
    #[diagnostic(help(
        "call the parent constructor from `fn init(...)` — not from a regular method"
    ))]
    SuperCallOutsideInit {
        #[label("not inside `init`")]
        span: miette::SourceSpan,
    },

    #[error("a function can declare at most one variadic parameter")]
    #[diagnostic(help(
        "remove the extra `...` parameter; variadic packs everything that follows the fixed params"
    ))]
    MultipleVariadicParams {
        #[label("second variadic")]
        span: miette::SourceSpan,
    },

    #[error("variadic parameter `{name}` must be the last parameter in the list")]
    #[diagnostic(help(
        "move `...{name}` to the end of the parameter list — nothing may come after it"
    ))]
    VariadicNotLast {
        name: String,
        #[label("variadic must come last")]
        span: miette::SourceSpan,
    },

    #[error("positional argument cannot follow a named argument")]
    #[diagnostic(help("pass every positional argument before any `name: value` arguments"))]
    PositionalAfterNamed {
        #[label("positional after named")]
        span: miette::SourceSpan,
    },

    #[error("function `{name}` declares return type `{ty}` but not every path returns a value")]
    #[diagnostic(help(
        "add a `return` on every path, end the function with `return ...`, or make the return type nullable with `?` (so missing returns yield `nil`)"
    ))]
    MissingReturn {
        name: String,
        ty: String,
        #[label("missing `return` on some path")]
        span: miette::SourceSpan,
    },

    #[error("`for ... in` supports one variable or a key/value pair, got {found}")]
    #[diagnostic(help(
        "use `for v in iter` for tables and iterators, or `for k, v in iter` for pairs"
    ))]
    ForInArity {
        found: usize,
        #[label("wrong number of variables")]
        span: miette::SourceSpan,
    },
}
