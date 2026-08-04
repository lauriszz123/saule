//! Expression typing rules shared by the LSP's two walkers.
//!
//! Hover ([`crate::hover::walker`]) and inlay hints ([`crate::server::inlay`])
//! both answer "what type does this expression produce?", and both used to
//! answer it with their own copy of the rules. The copies drifted from each
//! other and from the checker: `a ?? b` kept a nullability the checker
//! strips, `a or b` reported `boolean` where the checker reports an operand
//! type, `-x` skipped the nullable strip, and neither consulted operator
//! overloads at all — so a class overloading `OpConcat` was labelled
//! `string` and a `Matrix * Vector -> Vector` was labelled `Matrix`.
//!
//! These rules live here once, and both walkers reach them through
//! [`TypeSource`]. The authority is `saule_typeck`'s own `infer`: each
//! function below mirrors its corresponding arm, and the overload lookups
//! call straight into [`saule_typeck::ops`] rather than re-deriving it.
//!
//! One deliberate difference from the checker: where `infer` must return
//! *some* type and falls back to `any`, these return `None`. The checker
//! has to type every expression; a walker may decline, and no hint beats a
//! hint reading `: any` — the same "rather skip than mislead" rule the
//! inlay module documents.

use saule_ast::{BinOp, CallArg, Expr, PipeStage, Type, UnaryOp};
use saule_typeck::sigs::NativeSig;

/// What the shared rules need from a walker: the walker's own recursive
/// inference, plus the two lookups a pipeline stage resolves through.
pub(crate) trait TypeSource {
    /// The walker's own `infer` — recursion re-enters the caller so its
    /// local scope, class registry and lambda handling still apply.
    fn type_of(&self, expr: &Expr) -> Option<Type>;

    /// Signature for a pipeline stage's free function: the module's own
    /// top-level `fn`s first, then the native signature table.
    fn stage_sig(&self, name: &str) -> Option<NativeSig>;

    /// Inferred types of a call's positional arguments, in declaration
    /// order, with `None` where inference failed.
    fn arg_types(&self, args: &[CallArg]) -> Vec<Option<Type>>;
}

fn strip_nullable(ty: Type) -> Type {
    match ty {
        Type::Nullable(inner) => *inner,
        other => other,
    }
}

fn is_nullable(ty: &Type) -> bool {
    match ty {
        Type::Nullable(_) => true,
        Type::Named(n) => n == "nil",
        _ => false,
    }
}

fn named(n: &str) -> Option<Type> {
    Some(Type::Named(n.into()))
}

/// Type of `op rhs`.
///
/// An `OpNeg` / `OpLen` overload names its own result and wins; otherwise
/// `-x` keeps the operand's type (with any nullability stripped, as the
/// checker does), `#x` counts, and `not x` is a boolean.
pub(crate) fn unary_type<S: TypeSource + ?Sized>(cx: &S, op: UnaryOp, rhs: &Expr) -> Option<Type> {
    if let Some(ty) = cx
        .type_of(rhs)
        .and_then(|t| saule_typeck::ops::overload_unary_result(op, &t))
    {
        return Some(ty);
    }
    match op {
        UnaryOp::Neg => cx.type_of(rhs).map(strip_nullable),
        UnaryOp::Len => named("integer"),
        UnaryOp::Not => named("boolean"),
    }
}

/// Type of `lhs op rhs`, mirroring `saule_typeck`'s `infer`.
pub(crate) fn binary_type<S: TypeSource + ?Sized>(
    cx: &S,
    op: BinOp,
    lhs: &Expr,
    rhs: &Expr,
) -> Option<Type> {
    match op {
        // `lhs ?? rhs` drops the left side's nullability — that is the
        // whole point of the operator — and is nullable again only when
        // the *fallback* is.
        BinOp::Coalesce => match (cx.type_of(lhs), cx.type_of(rhs)) {
            (Some(lt), Some(rt)) => {
                let base = strip_nullable(lt);
                Some(if is_nullable(&rt) {
                    Type::Nullable(Box::new(base))
                } else {
                    base
                })
            }
            (Some(lt), None) => Some(strip_nullable(lt)),
            (None, Some(rt)) => Some(rt),
            (None, None) => None,
        },

        BinOp::Eq | BinOp::NotEq | BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => {
            named("boolean")
        }

        // Lua semantics: `and` / `or` evaluate to one of their *operands*,
        // not to a boolean. `local name = maybe ?? nil or "default"` is a
        // string, and labelling it `boolean` contradicts the checker.
        // Operands that disagree are a genuine union — decline rather than
        // pick a side.
        BinOp::And | BinOp::Or => match (cx.type_of(lhs), cx.type_of(rhs)) {
            (Some(lt), Some(rt)) => {
                let (lb, rb) = (strip_nullable(lt), strip_nullable(rt));
                (lb == rb).then_some(lb)
            }
            _ => None,
        },

        // `..` yields whatever an `OpConcat` overload returns, and a plain
        // `string` otherwise.
        BinOp::Concat => overload(cx, op, lhs).or_else(|| named("string")),

        // Arithmetic dispatches on the left operand's overload, else takes
        // whichever side we can type.
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod | BinOp::Pow => {
            overload(cx, op, lhs)
                .or_else(|| cx.type_of(lhs))
                .or_else(|| cx.type_of(rhs))
        }
    }
}

fn overload<S: TypeSource + ?Sized>(cx: &S, op: BinOp, lhs: &Expr) -> Option<Type> {
    saule_typeck::ops::overload_binary_result(op, &cx.type_of(lhs)?)
}

/// Type produced by a `when(source):a():b()` chain.
///
/// Each stage is an ordinary call with the piped value spliced in as
/// argument 0, so threading the value type through the chain is what binds
/// each stage's type parameters from what actually reaches it. Reading the
/// last stage's declared return alone reports its own parameter name
/// (`table<U>`), which is not a type the reader can act on.
///
/// `None` at any stage — an unresolvable stage name, or a type parameter
/// nothing pinned down — makes the whole chain unknown.
pub(crate) fn pipe_type<S: TypeSource + ?Sized>(
    cx: &S,
    source: &Expr,
    stages: &[PipeStage],
) -> Option<Type> {
    let mut current = cx.type_of(source);
    for stage in stages {
        let sig = cx.stage_sig(&stage.name)?;
        let arg_types = cx.arg_types(&stage.args);
        current = saule_typeck::sigs::instantiate_pipe_stage(&sig, current, &arg_types);
    }
    current
}

/// The type each stage's *explicit* arguments are expected to have, one
/// list per stage of the chain.
///
/// Threading the value type through the chain is what makes these
/// concrete: `when(nums):filter(x => …)` binds `T := integer` from the
/// piped `table<integer>`, so the predicate slot expects
/// `fn(integer) -> boolean` — and that is where the untyped `x` gets its
/// type. Mirrors what the checker does in `check_pipe`, so hover reports
/// the type the body is actually checked against.
pub(crate) fn pipe_stage_expectations<S: TypeSource + ?Sized>(
    cx: &S,
    source: &Expr,
    stages: &[PipeStage],
) -> Vec<Vec<Option<Type>>> {
    let mut current = cx.type_of(source);
    let mut out = Vec::with_capacity(stages.len());
    for stage in stages {
        let Some(sig) = cx.stage_sig(&stage.name) else {
            // Chain broken — no expectations from here on.
            out.resize(stages.len(), Vec::new());
            return out;
        };
        let arg_types = cx.arg_types(&stage.args);
        out.push(saule_typeck::sigs::instantiate_pipe_stage_params(
            &sig,
            current.clone(),
            &arg_types,
        ));
        current = saule_typeck::sigs::instantiate_pipe_stage(&sig, current, &arg_types);
    }
    out
}

/// Fill in a lambda's untyped parameters from the function type its slot
/// expects.
///
/// An omitted parameter type parses as `any`, and the target's function
/// type is the only place the real one can come from. An explicit
/// annotation on the lambda always wins — same rule the checker applies.
pub(crate) fn refine_lambda_params(
    params: &[saule_ast::Param],
    expected: Option<&Type>,
) -> Vec<saule_ast::Param> {
    let Some(Type::Function { params: want, .. }) = expected else {
        return params.to_vec();
    };
    params
        .iter()
        .enumerate()
        .map(|(i, p)| match want.get(i) {
            Some(t) if matches!(&p.ty, Type::Named(n) if n == "any") => saule_ast::Param {
                ty: t.clone(),
                ..p.clone()
            },
            _ => p.clone(),
        })
        .collect()
}
