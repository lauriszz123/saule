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

use std::ops::Range;

use saule_ast::Module;

mod error;
mod expr;
mod matches;
mod state;
mod stmt;

pub use error::TypeCheckError;

/// Convert a byte-range span into a `miette::SourceSpan`. Shared by every
/// submodule when emitting diagnostics.
pub(crate) fn to_source_span(r: Range<usize>) -> miette::SourceSpan {
    (r.start, r.end.saturating_sub(r.start)).into()
}

/// Run the static checks on a parsed module. Returns *all* errors found so
/// the user sees everything in one pass.
pub fn check(module: &Module) -> Vec<TypeCheckError> {
    let (reg, ifaces, enums) = state::build_registry(module);
    state::install_registries(reg, ifaces, enums);
    let _restore = state::set_current_class(None);

    let mut errors = Vec::new();
    let mut scope = state::Scope::default();
    for s in &module.stmts {
        stmt::check_stmt(s, &mut scope, &mut errors);
    }

    state::clear_registries();
    errors
}
