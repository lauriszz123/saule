//! All diagnostic kinds emitted by the typechecker. Each variant carries a
//! `miette` source span so the CLI can render it with the offending snippet
//! underlined.

use miette::Diagnostic;
use thiserror::Error;

#[derive(Debug, Error, Diagnostic)]
pub enum TypeCheckError {

    #[error("cannot assign `nil` to non-nullable type `{ty}`")]
    #[diagnostic(help(
        "mark the type nullable with `?` (e.g. `{ty}?`) or initialize it with a non-nil value"
    ))]
    NilToNonNullable {
        ty: String,
        #[label("`nil` not allowed here")]
        span: miette::SourceSpan,
    },

    #[error("cannot assign a nullable value of type `{from}` to non-nullable type `{to}`")]
    #[diagnostic(help(
        "guard with `if x != nil then ... end`, use `??` to provide a fallback, or force-unwrap with `!`"
    ))]
    NullableToNonNullable {
        from: String,
        to: String,
        #[label("this expression may be `nil`")]
        span: miette::SourceSpan,
    },

    #[error("cannot access `{member}` on nullable type `{ty}`")]
    #[diagnostic(help(
        "use `?.` for safe access, `!` to force-unwrap, or guard with `if x != nil then ... end`"
    ))]
    NullableMemberAccess {
        ty: String,
        member: String,
        #[label("receiver may be `nil`")]
        span: miette::SourceSpan,
    },

    #[error("default value for parameter `{param}` is incompatible with declared type `{ty}`")]
    #[diagnostic(help(
        "the default expression must produce a value of type `{ty}`"
    ))]
    DefaultParamTypeMismatch {
        param: String,
        ty: String,
        #[label("default here")]
        span: miette::SourceSpan,
    },

    #[error("return value is incompatible with declared return type `{ty}`")]
    #[diagnostic(help("this function must return a `{ty}`"))]
    WrongReturnType {
        ty: String,
        #[label("returned here")]
        span: miette::SourceSpan,
    },

    #[error("cannot access private member `{member}` of class `{class}` from outside the class")]
    #[diagnostic(help(
        "`local` fields and methods are only accessible from within `{class}`"
    ))]
    PrivateMemberAccess {
        class: String,
        member: String,
        #[label("private")]
        span: miette::SourceSpan,
    },

    #[error("table value of type `{found}` is incompatible with declared element type `{expected}`")]
    #[diagnostic(help(
        "every value stored in this table must be a `{expected}`"
    ))]
    TableElementTypeMismatch {
        expected: String,
        found: String,
        #[label("wrong value type")]
        span: miette::SourceSpan,
    },

    #[error("table key of type `{found}` is incompatible with declared key type `{expected}`")]
    #[diagnostic(help(
        "this table is declared with key type `{expected}` — pass an index of that type"
    ))]
    TableKeyTypeMismatch {
        expected: String,
        found: String,
        #[label("wrong key type")]
        span: miette::SourceSpan,
    },

    #[error("cannot initialise `table<{key}, {value}>` with an array-style literal")]
    #[diagnostic(help(
        "array-style `{{ ... }}` literals can only fill `table<T>` (integer-keyed); start from `{{}}` and assign by key instead"
    ))]
    TableArrayLiteralForMap {
        key: String,
        value: String,
        #[label("array literal not allowed for a map-typed table")]
        span: miette::SourceSpan,
    },

    #[error("cannot iterate over a `{class}` — it does not implement `Iterable` or `Iterable2`")]
    #[diagnostic(help(
        "add `implements Iterable<T>` (or `Iterable2<K, V>`) to `{class}` and define `fn iter() -> fn(): T?` returning a step closure"
    ))]
    NotIterable {
        class: String,
        #[label("class is not iterable")]
        span: miette::SourceSpan,
    },

    #[error("argument {arg} of `{callee}` expects `{expected}`, got `{found}`")]
    #[diagnostic(help(
        "pass a value of type `{expected}` here — check the signature of `{callee}`"
    ))]
    NativeArgTypeMismatch {
        callee: String,
        arg: usize,
        expected: String,
        found: String,
        #[label("wrong argument type")]
        span: miette::SourceSpan,
    },

    #[error("`{callee}` expects {expected} argument(s), got {found}")]
    #[diagnostic(help("check the signature of `{callee}`"))]
    NativeArity {
        callee: String,
        expected: usize,
        found: usize,
        #[label("wrong number of arguments")]
        span: miette::SourceSpan,
    },

    #[error("`{construct}` condition must be a `boolean`, got `{found}`")]
    #[diagnostic(help(
        "compare with `==`, `!=`, `<`, `>` etc., or use `?? false` / `!= nil` to coerce a nullable to a `boolean`"
    ))]
    NonBooleanCondition {
        construct: &'static str,
        found: String,
        #[label("not a boolean")]
        span: miette::SourceSpan,
    },

    #[error("non-exhaustive `match`: {reason}")]
    #[diagnostic(help(
        "add a wildcard arm `case _ then ...` or cover the remaining cases explicitly"
    ))]
    MatchNonExhaustive {
        reason: String,
        #[label("not all cases covered")]
        span: miette::SourceSpan,
    },

    #[error("`match` arms produce incompatible types: `{expected}` vs `{found}`")]
    #[diagnostic(help("every arm of a `match` expression must evaluate to the same type"))]
    MatchArmTypeMismatch {
        expected: String,
        found: String,
        #[label("arms disagree on result type")]
        span: miette::SourceSpan,
    },

    #[error("pattern of type `{found}` cannot match scrutinee of type `{expected}`")]
    #[diagnostic(help("change the pattern to match the scrutinee's type"))]
    MatchPatternTypeMismatch {
        expected: String,
        found: String,
        #[label("incompatible pattern")]
        span: miette::SourceSpan,
    },

    #[error("enum `{enum_name}` has no variant `{variant}`")]
    #[diagnostic(help("check the spelling of the variant or add it to the enum"))]
    MatchUnknownVariant {
        enum_name: String,
        variant: String,
        #[label("unknown variant")]
        span: miette::SourceSpan,
    },

    #[error("variant `{variant}` expects {expected} field(s), got {found}")]
    #[diagnostic(help("supply a sub-pattern for every payload field of the variant"))]
    MatchVariantArityMismatch {
        variant: String,
        expected: usize,
        found: usize,
        #[label("wrong number of sub-patterns")]
        span: miette::SourceSpan,
    },
}
