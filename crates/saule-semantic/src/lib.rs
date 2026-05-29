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
pub mod prelude;
pub mod registry;
mod resolve;
mod return_check;

pub use error::SemanticError;
pub use registry::{
    ClassInfo, ClassRegistry, EnumInfo, EnumRegistry, InterfaceRegistry, MethodSig, build_registry,
    class_implements, class_implements_iterable, clear_registries, install_registries,
    interface_extends, is_interface, is_subtype_named, lookup_member, lookup_method, with_classes,
    with_enums, with_interfaces,
};

/// Shared span helper. Submodules emit `miette::SourceSpan`s through this
/// so the conversion only lives in one place.
pub(crate) fn to_source_span(r: Range<usize>) -> miette::SourceSpan {
    (r.start, r.end.saturating_sub(r.start)).into()
}

/// Pre-built class / interface / enum metadata to splice into the
/// registry before analysing the current module. Used by embedders that
/// can resolve `import` statements (the interpreter's module loader)
/// to make imported classes' method signatures visible to the
/// typechecker.
///
/// Locally-declared symbols always win on a name collision: if a module
/// declares `class Json` *and* imports a `Json` from elsewhere, the
/// imported entry is ignored.
#[derive(Default, Debug)]
pub struct ModuleSeed {
    pub classes: ClassRegistry,
    pub interfaces: InterfaceRegistry,
    pub enums: EnumRegistry,
}

/// Run every semantic check on a parsed module. See [`analyze_with_seed`]
/// to also include metadata from imported modules.
pub fn analyze(module: &Module) -> Vec<SemanticError> {
    analyze_with_seed(module, ModuleSeed::default())
}

/// Like [`analyze`] but seeds the class / interface / enum registry with
/// metadata pre-collected from this module's imports. The seed augments
/// (but does not override) the metadata extracted from the current
/// module's own declarations.
pub fn analyze_with_seed(module: &Module, seed: ModuleSeed) -> Vec<SemanticError> {
    let (mut reg, mut ifaces, mut enums) = build_registry(module);
    for (name, info) in seed.classes {
        reg.entry(name).or_insert(info);
    }
    for (name, ext) in seed.interfaces {
        ifaces.entry(name).or_insert(ext);
    }
    for (name, info) in seed.enums {
        enums.entry(name).or_insert(info);
    }
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

    // Every function/method with a non-nullable return type must actually
    // return on every reachable path (catches the "fell off the end →
    // implicit nil" bug at compile time).
    return_check::check_module(module, &mut errors);

    // Name resolution + a bundle of structural checks
    // (self/super placement, variadic param shape, arg ordering, for-in
    // arity). Walks the AST once, sharing scope state across the checks.
    resolve::check(module, &mut errors);

    errors
}
