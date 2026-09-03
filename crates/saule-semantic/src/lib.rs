//! Semantic analysis for Saule — the pass that runs after parsing and
//! before type-checking. It catches everything that can be decided from
//! the structure of the program alone, without inferring types:
//!
//! * **Class/interface/enum registry build** — exposed publicly so later
//!   passes (the typechecker, future name-resolution work) consult one
//!   source of truth for what's declared. See [`registry`].
//! * **Definite-assignment** — every non-nullable instance field must be
//!   assigned `self.field = ...` inside the class's constructor, and every
//!   non-nullable `static local` field must carry a value in its
//!   declaration. See [`field_init`].
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

pub mod binding;
pub mod builtins;
mod control_flow;
mod error;
mod field_init;
pub mod prelude;
pub mod registry;
mod resolve;
mod return_check;
mod return_infer;

pub use binding::{Binding, Bindings, FunctionInfo, FunctionTable, ResolveTable, UpvalRef};
pub use error::SemanticError;
pub use return_infer::infer_missing_returns;
pub use registry::{
    ClassInfo, ClassRegistry, EnumInfo, EnumRegistry, FunctionRegistry, FunctionSig,
    InterfaceMethodRegistry, InterfaceRegistry, InterfaceTypeParamRegistry, MethodSig,
    VariableRegistry, VariantInfo, build_function_registry, build_registry,
    build_variable_registry, class_implements, class_implements_iterable, clear_registries,
    install_functions, install_registries, install_variables, interface_extends,
    interface_type_params, is_interface, is_subtype_named, lookup_field_type, lookup_function,
    lookup_interface_method, lookup_member, lookup_method, super_init_target, with_classes,
    with_enums, with_functions, with_interfaces, with_variables,
};

/// Shared span helper. Submodules emit `miette::SourceSpan`s through this
/// so the conversion only lives in one place.
pub(crate) fn to_source_span(r: Range<usize>) -> miette::SourceSpan {
    (r.start, r.end.saturating_sub(r.start)).into()
}

/// Comparing two seeds for the thing that actually matters: whether an
/// importer would analyse any differently against one than the other.
///
/// Not `PartialEq`, because that is not the question. A seed carries
/// [`saule_ast::Param`]s, and a `Param` carries the span it occupied in the
/// file it was declared in — so editing a function body shifts the spans of
/// every declaration below it and makes two otherwise identical seeds
/// compare unequal. Nothing reads those spans through a registry (they point
/// into a *different* file's text than the one being analysed), and the
/// difference between "the same" and "the same but three characters further
/// down" is the difference between an editor that re-checks one file per
/// keystroke and one that re-checks twenty.
///
/// Deliberately conservative wherever it cannot be exact: a variant's
/// discriminant expression is compared with its span included, so an edit
/// above a valued enum reports a change that isn't one. That costs work;
/// the other kind of mistake would show stale diagnostics.
impl ModuleSeed {
    pub fn same_surface(&self, other: &ModuleSeed) -> bool {
        same_map(&self.classes, &other.classes, same_class)
            && self.interfaces == other.interfaces
            && same_map(&self.interface_methods, &other.interface_methods, |a, b| {
                same_map(a, b, same_method)
            })
            && same_map(&self.enums, &other.enums, same_enum)
            && same_map(&self.functions, &other.functions, same_function)
            && self.variables == other.variables
            && self.wildcard_names == other.wildcard_names
    }
}

fn same_map<V>(
    a: &std::collections::HashMap<String, V>,
    b: &std::collections::HashMap<String, V>,
    eq: impl Fn(&V, &V) -> bool,
) -> bool {
    a.len() == b.len() && a.iter().all(|(k, v)| b.get(k).is_some_and(|w| eq(v, w)))
}

/// A parameter as an importer sees it: its name, its type, whether it is
/// variadic, and whether it may be omitted. The default's *value* is the
/// declaring file's business — every cross-file reader asks only whether
/// there is one.
fn same_params(a: &[saule_ast::Param], b: &[saule_ast::Param]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            x.name == y.name
                && x.ty == y.ty
                && x.variadic == y.variadic
                && x.default.is_some() == y.default.is_some()
        })
}

fn same_method(a: &MethodSig, b: &MethodSig) -> bool {
    a.is_static == b.is_static
        && a.is_private == b.is_private
        && a.type_params == b.type_params
        && a.return_ty == b.return_ty
        && same_params(&a.params, &b.params)
}

fn same_function(a: &FunctionSig, b: &FunctionSig) -> bool {
    a.type_params == b.type_params
        && a.return_ty == b.return_ty
        && same_params(&a.params, &b.params)
}

fn same_class(a: &ClassInfo, b: &ClassInfo) -> bool {
    a.type_params == b.type_params
        && a.parent == b.parent
        && a.parent_args == b.parent_args
        && a.implements == b.implements
        && a.implements_args == b.implements_args
        && a.members == b.members
        && a.field_types == b.field_types
        && same_map(&a.methods, &b.methods, same_method)
}

fn same_enum(a: &EnumInfo, b: &EnumInfo) -> bool {
    a.type_params == b.type_params
        && same_map(&a.variants, &b.variants, |x, y| {
            same_params(&x.fields, &y.fields) && x.discriminant == y.discriminant
        })
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
/// `Clone` so embedders can memoise a seed across requests: building one
/// means reading and parsing every reachable module, which the language
/// server was doing on every keystroke.
/// `PartialEq` so the database can tell a rebuilt seed that came out the
/// same from one that actually changed. Editing a function body in a
/// widely-imported module rebuilds every importer's seed to an identical
/// value; without the comparison every one of those importers would be
/// re-analysed on every keystroke.
#[derive(Default, Debug, Clone, PartialEq)]
pub struct ModuleSeed {
    pub classes: ClassRegistry,
    pub interfaces: InterfaceRegistry,
    /// Method signatures of the interfaces the imports bring in, keyed by
    /// the local name. Carried separately from `interfaces` for the same
    /// reason the registry is: that map's value type is the `extends` list
    /// and six LSP call sites destructure it.
    pub interface_methods: InterfaceMethodRegistry,
    pub enums: EnumRegistry,
    /// Signatures of the top-level `fn`s the imports bring in, keyed by the
    /// name they are bound to locally.
    pub functions: FunctionRegistry,
    /// Declared types of the `export name: T = value` module variables the
    /// imports bring in, keyed by their local name.
    pub variables: VariableRegistry,
    /// Local names this module's `import * from "..."` statements bind,
    /// as enumerated by the embedder.
    ///
    /// `None` — the default — means "not enumerated", and the resolver
    /// falls back to suppressing undefined-name diagnostics in any module
    /// that globs. Embedders that can resolve every wildcard target (the
    /// interpreter's module loader, and through it the CLI and the LSP)
    /// pass `Some`, which keeps those diagnostics live.
    pub wildcard_names: Option<std::collections::HashSet<String>>,
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
    analyze_inner(module, seed, false).0
}

/// [`analyze_with_seed`], but also hands back what the resolver learned:
/// where every identifier binds, the slot each local occupies, and the exact
/// upvalue list of every closure.
///
/// The resolver already decides all of this in order to report
/// `UndefinedName`; it simply discarded the answer. Recovering it is what
/// lets the bytecode compiler turn a name into a register index, and what
/// lets closure capture be *exact* rather than the over-approximation
/// `saule-interpreter`'s `capture.rs` performs
/// (`VM_DESIGN.md` §7.1, §21.1 item 0.6).
///
/// **Precondition**: `module` has been through `saule_ast::assign_ids`.
/// Every module the parser produces has.
pub fn analyze_with_bindings(module: &Module, seed: ModuleSeed) -> (Vec<SemanticError>, Bindings) {
    let (errors, bindings) = analyze_inner(module, seed, true);
    (errors, bindings.unwrap_or_default())
}

fn analyze_inner(
    module: &Module,
    seed: ModuleSeed,
    collect_bindings: bool,
) -> (Vec<SemanticError>, Option<Bindings>) {
    let wildcard_names = seed.wildcard_names;
    let (mut reg, mut ifaces, mut enums, mut iface_methods, iface_params) = build_registry(module);
    for (name, info) in seed.classes {
        reg.entry(name).or_insert(info);
    }
    for (name, ext) in seed.interfaces {
        ifaces.entry(name).or_insert(ext);
    }
    for (name, sigs) in seed.interface_methods {
        iface_methods.entry(name).or_insert(sigs);
    }
    for (name, info) in seed.enums {
        enums.entry(name).or_insert(info);
    }
    // Embedder-provided builtins (e.g. stdlib value types). User
    // declarations and seed entries take precedence.
    let built = builtins::snapshot();
    for (name, info) in built.classes {
        reg.entry(name).or_insert(info);
    }
    for (name, ext) in built.interfaces {
        ifaces.entry(name).or_insert(ext);
    }
    for (name, info) in built.enums {
        enums.entry(name).or_insert(info);
    }
    let mut funcs = build_function_registry(module);
    for (name, sig) in seed.functions {
        funcs.entry(name).or_insert(sig);
    }
    let mut vars = build_variable_registry(module);
    for (name, ty) in seed.variables {
        vars.entry(name).or_insert(ty);
    }
    // Everything a body could read is in `reg` by now — this module's own
    // declarations, the seed's imports, the embedder's builtins — so this
    // is the first point at which an unannotated `return self.field` can be
    // typed, and the last before the registries are handed downstream.
    return_infer::infer_missing_returns(module, &mut reg, &mut funcs);

    install_registries(reg, ifaces, enums, iface_methods, iface_params);
    install_functions(funcs);
    install_variables(vars);

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
    let bindings = resolve::check(
        module,
        wildcard_names.as_ref(),
        &mut errors,
        collect_bindings,
    );

    (errors, bindings)
}
