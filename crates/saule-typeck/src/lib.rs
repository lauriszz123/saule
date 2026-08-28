//! Static checks performed between parsing and evaluation.
//!
//! Two flavours of checks live here:
//!
//! 1. **Field initialization** — every non-nullable instance field of a class
//!    with a constructor must be assigned `self.field = ...` somewhere inside
//!    that constructor's body. Fields with defaults are exempt; nullable
//!    fields (`name: string?`) are exempt by design.
//!
//! 2. **Null safety** — values whose static type is nullable cannot be
//!    silently assigned to non-nullable bindings, and members cannot be read
//!    off a nullable receiver without `?.` / `!` / a prior nil-guard. A
//!    lightweight flow-narrowing pass treats `if x != nil then ... end` as
//!    proving `x` is non-nullable inside the then-block (and symmetrically
//!    for `if x == nil ... else ...`).
//!
//! The expression type inference here is intentionally partial: when the
//! checker can't prove a type (e.g. calls, member reads on unknown classes),
//! it returns `None` and conservatively skips the check rather than producing
//! a false positive.
//!
//! ## Module layout
//!
//! | File | Contents |
//! |------|----------|
//! | [`error`]   | The [`TypeCheckError`] diagnostic enum |
//! | [`state`]   | Per-scope `Scope`, thread-local class/interface/enum registries, lookup helpers |
//! | [`stmt`]    | Statement & declaration walker, class field-init checks, return-type checks |
//! | [`expr`]    | Expression walker, type inference, native-call args, flow narrowing |
//! | [`matches`] | `match` exhaustiveness, pattern/scrutinee compat, arm-type unification |
//! | [`ops`]     | Operator overloading via the built-in `Op*` interfaces |
//! | [`sigs`]    | Native-function signature registry (consumed by `expr`, populated by embedders) |

use saule_ast::Module;
pub(crate) use saule_ast::to_source_span;

pub mod casts;
mod coerce_sites;
pub mod coverage;
mod error;
pub(crate) mod expr;
mod funcs;
mod matches;
pub mod ops;
pub mod sigs;
mod state;
mod stmt;
mod table;
mod vars;

pub use coverage::Coverage;
pub use error::TypeCheckError;
pub use table::TypeTable;

/// Run the static type checks on a parsed module. Returns *all* errors
/// found so the user sees everything in one pass.
///
/// **Precondition**: `saule_semantic::analyze` (or an equivalent) must have
/// installed the class / interface / enum registries already — typeck reads
/// them through `saule-semantic`'s thread-local accessors. The standard
/// pipeline (`saule_interpreter::pipeline` or the CLI) guarantees this.
pub fn check(module: &Module) -> Vec<TypeCheckError> {
    check_inner(module)
}

/// [`check`], but also hands back the types it proved along the way.
///
/// The checker already computes a type for most expressions and currently
/// discards it. The bytecode compiler needs precisely that: choosing `ADDI`
/// over the dynamic `ARITHX` *is* the question "did the checker prove both
/// operands are integers?" (`VM_DESIGN.md` §2, §21.1 item 0.5).
///
/// **Precondition**: `module` has been through
/// [`saule_ast::assign_ids`](saule_ast::assign_ids) — the table is keyed by
/// `NodeId`, and an unnumbered tree yields an empty one. Every module the
/// parser produces satisfies this.
///
/// Coverage is partial on purpose; see [`coverage`] for measuring it, and
/// [`TypeTable`] for why a missing entry is always safe.
pub fn check_with_types(module: &Module) -> (Vec<TypeCheckError>, TypeTable) {
    let previous = table::begin();
    let errors = check_inner(module);
    (errors, table::end(previous))
}

/// [`check`], then stamp each `as` in `module` with the reading the check
/// decided on (see [`saule_ast::CastKind`]).
///
/// **Execution paths should call this rather than [`check`].** A cast left
/// unresolved runs as the checked type test, which is the conservative
/// reading but the wrong one for `10f as integer` — the checker will have
/// typed that as `integer` while the runtime hands back `nil`.
///
/// Run it *before* anything keys off the identity of a lambda body (in the
/// standard pipeline, `saule_interpreter::prepare_captures`): resolving a
/// cast inside a lambda body reallocates that body if its `Arc` is already
/// shared.
pub fn check_and_resolve(module: &mut Module) -> Vec<TypeCheckError> {
    let previous = casts::begin();
    let errors = check_inner(module);
    stamp_casts(module, casts::end(previous));
    errors
}

/// [`check_and_resolve`] and [`check_with_types`] in one pass, for the
/// bytecode path — which needs both the stamped tree and the type table,
/// and would otherwise typecheck every module twice to get them.
pub fn check_and_resolve_with_types(module: &mut Module) -> (Vec<TypeCheckError>, TypeTable) {
    let prev_casts = casts::begin();
    let prev_types = table::begin();
    let errors = check_inner(module);
    let types = table::end(prev_types);
    stamp_casts(module, casts::end(prev_casts));
    (errors, types)
}

/// Write the decided [`CastKind`](saule_ast::CastKind)s back into the tree.
///
/// Skipped outright when nothing was recorded, which is most modules: the
/// walk is `&mut`, and reaching a lambda body through one reallocates that
/// body when its `Arc` is shared. A program with no `as` in it should not
/// pay for a feature it does not use.
fn stamp_casts(
    module: &mut Module,
    kinds: std::collections::HashMap<saule_ast::NodeId, saule_ast::CastKind>,
) {
    if kinds.is_empty() {
        return;
    }
    saule_ast::visit_exprs_mut(module, |e| {
        if let saule_ast::Expr::Cast { kind, .. } = &mut e.value
            && let Some(k) = kinds.get(&e.id)
        {
            *kind = *k;
        }
    });
}

fn check_inner(module: &Module) -> Vec<TypeCheckError> {
    let _restore = state::set_current_class(None);
    funcs::install(module);
    vars::install(module);

    let mut errors = Vec::new();
    let mut scope = state::Scope::default();
    for s in &module.stmts {
        stmt::check_stmt(s, &mut scope, &mut errors);
    }

    funcs::clear();
    vars::clear();
    errors
}

/// Type-check `module` and report how much of it the resulting table covers.
/// Backs the CLI's `--dump-type-coverage`.
pub fn type_coverage(module: &Module) -> Coverage {
    let (_, table) = check_with_types(module);
    coverage::measure(module, &table)
}
