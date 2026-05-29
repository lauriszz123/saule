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

    #[error("`??` fallback of type `{found}` is incompatible with left-hand side base type `{expected}`")]
    #[diagnostic(help(
        "the fallback expression must produce a `{expected}` so the whole `??` expression has a consistent type"
    ))]
    CoalesceFallbackTypeMismatch {
        expected: String,
        found: String,
        #[label("incompatible fallback")]
        span: miette::SourceSpan,
    },

    #[error("comparison between unrelated types `{left}` and `{right}` is always {result}")]
    #[diagnostic(help(
        "these types can never be equal — did you mean to compare against `nil` instead?"
    ))]
    DisjointEquality {
        left: String,
        right: String,
        result: &'static str,
        #[label("types can never match")]
        span: miette::SourceSpan,
    },

    #[error("operator `{op}` cannot be applied to type `{found}` (expected {expected})")]
    #[diagnostic(help("change the operand so it has a compatible type"))]
    BinaryOperandTypeMismatch {
        op: &'static str,
        expected: &'static str,
        found: String,
        #[label("incompatible operand")]
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

    #[error("no member `{member}` on `{receiver}`")]
    #[diagnostic(help(
        "check the spelling, or add `{member}` as a field / method to `{receiver}`"
    ))]
    UnknownMember {
        receiver: String,
        member: String,
        #[label("unknown member")]
        span: miette::SourceSpan,
    },

    #[error("cannot assign field `{member}` on value of type `{receiver}`")]
    #[diagnostic(help(
        "only class instances and class statics support `obj.{member} = ...`; for tables use `t[\"{member}\"] = ...`"
    ))]
    InvalidFieldAssign {
        receiver: String,
        member: String,
        #[label("not a class or instance")]
        span: miette::SourceSpan,
    },

    #[error("enum `{enum_name}` has no variant `{variant}`")]
    #[diagnostic(help("check the spelling of the variant or add it to the enum"))]
    UnknownEnumVariant {
        enum_name: String,
        variant: String,
        #[label("unknown variant")]
        span: miette::SourceSpan,
    },

    #[error("class `{name}` cannot extend `{parent}` — no class with that name is in scope")]
    #[diagnostic(help("define `class {parent}` first, or import it from another module"))]
    UnknownParentClass {
        name: String,
        parent: String,
        #[label("unknown parent class")]
        span: miette::SourceSpan,
    },

    #[error("class `{name}` cannot implement `{iface}` — no interface with that name is in scope")]
    #[diagnostic(help(
        "define `interface {iface}` first, import it, or remove it from the `implements` list"
    ))]
    UnknownInterface {
        name: String,
        iface: String,
        #[label("unknown interface")]
        span: miette::SourceSpan,
    },

    #[error("`{callee}` expects {expected} argument(s), got {found}")]
    #[diagnostic(help(
        "check the signature of `{callee}` — pass exactly the right number of arguments (or rely on declared defaults)"
    ))]
    FunctionArity {
        callee: String,
        expected: usize,
        found: usize,
        #[label("wrong number of arguments")]
        span: miette::SourceSpan,
    },

    #[error("enum variant `{enum_name}.{variant}` expects {expected} field(s), got {found}")]
    #[diagnostic(help(
        "construct the variant with exactly the declared positional fields"
    ))]
    EnumVariantArity {
        enum_name: String,
        variant: String,
        expected: usize,
        found: usize,
        #[label("wrong number of fields")]
        span: miette::SourceSpan,
    },

    #[error("cannot mix `integer` and `float` in arithmetic — type mismatch")]
    #[diagnostic(help(
        "Saule never auto-promotes numeric types; wrap one operand in `int(...)` or `float(...)` to make the kinds match"
    ))]
    NumericMix {
        #[label("incompatible numeric kinds")]
        span: miette::SourceSpan,
    },

    #[error("`nil` is not a valid type annotation — it is only a value")]
    #[diagnostic(help(
        "to allow `nil`, mark the surrounding type as nullable (e.g. `string?`); `nil` may only appear as a value or as the unit return type (`-> nil`)"
    ))]
    NilTypeAnnotation {
        #[label("`nil` cannot be used as a binding type")]
        span: miette::SourceSpan,
    },

    #[error("pipeline stage `{stage}` expects `{expected}` as first argument, got `{found}`")]
    #[diagnostic(help(
        "the value flowing through `when(...):{stage}(...)` must match the function's first parameter — adjust the upstream stage or call `{stage}` directly"
    ))]
    PipeStageTypeMismatch {
        stage: String,
        expected: String,
        found: String,
        #[label("piped value has the wrong type for this stage")]
        span: miette::SourceSpan,
    },

    #[error("pipeline stage `{stage}` takes {expected} argument(s) (one of them is the piped value), got {found}")]
    #[diagnostic(help(
        "the upstream value counts as the first argument; pass the rest in the parentheses, e.g. `:fn(a, b)`"
    ))]
    PipeStageArity {
        stage: String,
        expected: usize,
        found: usize,
        #[label("wrong number of arguments in this pipeline stage")]
        span: miette::SourceSpan,
    },

    #[error("pipeline stage `{stage}` is not a known function — `when` chains only call free functions")]
    #[diagnostic(help(
        "declare `fn {stage}(first: T, …) -> U` at the top level (or import it), then re-run; class methods and locally-bound lambdas are not currently pipeable"
    ))]
    UnknownPipeStage {
        stage: String,
        #[label("no top-level function with this name")]
        span: miette::SourceSpan,
    },
}
