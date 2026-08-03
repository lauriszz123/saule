//! Expression and parameter-list parsing: precedence ladder, postfix chains,
//! primary expressions (including match, lambdas, table literals), and
//! function-parameter declarations.

mod binary;
mod lambda;
mod matching;
mod pipe;
mod postfix;
mod primary;

/// Whether a parameter list requires `: T` on every parameter.
///
/// Declarations do — a `fn`'s signature is the contract its callers see, and
/// there is nothing to infer it from. Lambdas don't: an omitted type parses
/// as `any` and the typechecker refines it from the lambda's target type,
/// which is what makes `local f: fn(integer) -> integer = fn(x) ... end`
/// type `x` as `integer`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParamTypes {
    Required,
    Optional,
}
