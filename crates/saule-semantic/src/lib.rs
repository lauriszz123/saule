//! Semantic analysis for Saule — the pass that runs after parsing and
//! before type-checking. It catches everything that can be decided from
//! the structure of the program alone, without inferring types:
//!
//! * **Class/interface/enum registry build** — exposed publicly so later
//!   passes (the typechecker, future name-resolution work) consult one
//!   source of truth for what's declared. See [`registry`].
//! * **Definite-assignment** — every non-nullable instance field of a class
//!   with a constructor must be assigned `self.field = ...` inside that
//!   constructor.
//! * **Control-flow validity** — `break` and `continue` are only valid
//!   inside loops; `return` is only valid inside functions.
//!
//! All diagnostics emitted live in [`SemanticError`]. The pipeline:
//!
//! ```text
//! lex → parse → semantic::analyze → typeck::check → interpret
//! ```
//!
//! makes [`analyze`] the gate that runs *first* once a `Module` is in hand;
//! a non-empty error list there means typechecking should be skipped (the
//! type pass assumes a structurally valid module).

use std::ops::Range;

use saule_ast::{Decl, Module, Stmt};

mod control_flow;
mod error;
mod field_init;
pub mod registry;

pub use error::SemanticError;
pub use registry::{
    ClassInfo, ClassRegistry, EnumInfo, EnumRegistry, InterfaceRegistry, build_registry,
    class_implements, class_implements_iterable, clear_registries, install_registries,
    interface_extends, is_interface, is_subtype_named, lookup_member, with_classes, with_enums,
    with_interfaces,
};

/// Shared span helper. Submodules emit `miette::SourceSpan`s through this
/// so the conversion only lives in one place.
pub(crate) fn to_source_span(r: Range<usize>) -> miette::SourceSpan {
    (r.start, r.end.saturating_sub(r.start)).into()
}

/// Run every semantic check on a parsed module.
///
/// As a side effect this installs the class/interface/enum registries into
/// thread-local slots so a subsequent `saule_typeck::check` call sees the
/// same metadata without re-walking the AST. Callers that don't intend to
/// typecheck can call [`clear_registries`] afterwards.
pub fn analyze(module: &Module) -> Vec<SemanticError> {
    let (reg, ifaces, enums) = build_registry(module);
    install_registries(reg, ifaces, enums);

    let mut errors = Vec::new();

    // Definite assignment on every class declaration.
    for stmt in &module.stmts {
        if let Stmt::Decl(d) = &stmt.value
            && let Decl::Class { name, members, .. } = &d.value
        {
            field_init::check_class(name, members, &mut errors);
        }
    }

    // Control-flow validity over every executable region.
    control_flow::check_module(module, &mut errors);

    errors
}


