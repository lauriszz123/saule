//! Compiling a whole program — several modules over one shared type world
//! (`VM_DESIGN.md` §14).
//!
//! ## Why a program is not just a chunk
//!
//! `saule-interpreter`'s loader resolves an `import` at *run* time: it lexes,
//! parses, checks and executes the imported file on first use, then caches
//! the resulting `Value`s. That works because the tree-walker looks names up
//! by string.
//!
//! This compiler resolves names to **indices**, and two of those index spaces
//! cannot be per-module:
//!
//! * a class's `ClassIdx`, because a subclass in one module extends a parent
//!   in another and its field slots and vtable slots are prefix-extensions of
//!   the parent's real ones. Computing the parent's layout twice — once where
//!   it is declared and once where it is extended — is precisely the
//!   divergence §24.2 calls the worst bug this project could ship;
//! * an enum's `EnumIdx` and an interface's `InterfaceIdx`, for the same
//!   reason at one remove.
//!
//! So the import graph is resolved **at compile time**, every module is laid
//! out into one shared table, and the resulting `ClassIdx` is program-global.
//! Each module still gets its own [`Chunk`] — its own protos, constants and
//! module slots — which is what §14 asks for so a per-module bytecode cache
//! stays possible.
//!
//! ## Order
//!
//! Modules are compiled and executed in **post-order**: every module appears
//! after the ones it imports. That is not only a compilation requirement (a
//! parent class must be laid out before the subclass that extends it) — it is
//! observable behaviour. The tree-walker runs an imported module's top level
//! on first import, so a module that prints at the top level prints in
//! post-order, and the two engines have to agree.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use saule_ast::{Decl, ImportNames, Module, Stmt};
use saule_runtime::RuntimeError;
use saule_runtime::error::ImportedDiagnostic;

use crate::chunk::{Chunk, ClassIdx, EnumIdx, InterfaceIdx};
use crate::compile::CompileError;
use crate::compile::ctx::ImportBinding;

/// A compiled program: the module chunks plus which one is the entry.
///
/// Every chunk's `classes` / `enums` / `interfaces` are the **same** `Rc`, so
/// a `ClassIdx` means the same thing in any of them.
#[derive(Debug)]
pub struct Program {
    /// Post-order: every module appears after the ones it imports.
    pub modules: Vec<Rc<Chunk>>,
    /// Index into `modules` of the file the user asked to run.
    pub entry: usize,
    /// Per module, the span of the entry file's `import` that first reached
    /// it — `None` for the entry itself. An error raised while a module's
    /// top level runs is reported against that `import`.
    pub via: Vec<Option<Range<usize>>>,
}

impl Program {
    pub fn entry_chunk(&self) -> &Rc<Chunk> {
        &self.modules[self.entry]
    }
}

/// What an exported name denotes, once its module has been compiled.
///
/// Types resolve entirely at compile time and cost nothing at run time — an
/// imported `Button` becomes the same `ClassIdx` the declaring module got,
/// and `Button(...)` compiles to a plain `NEW`. Values are different: they
/// need the exporting module to have *run*, so they travel through module
/// slots (§14, "exported names land in the importing chunk's module slots").
#[derive(Debug, Clone)]
pub enum Export {
    Class(ClassIdx),
    Enum(EnumIdx),
    Interface(InterfaceIdx),
    /// An exported `fn` or module variable, as a slot in the program's flat
    /// slot space — already rebased, so an importer can read it directly.
    Value {
        slot: u16,
    },
    /// A native package's export that a barrel re-publishes. Native exports
    /// are Rust-built values fixed before the program runs, so it travels
    /// as the value itself and folds into a constant wherever it lands.
    Native(saule_runtime::Value),
}

/// One module on its way from a path to a chunk.
pub struct Unit {
    pub path: PathBuf,
    /// The label diagnostics use — a pretty relative path.
    pub label: String,
    pub source: String,
    pub ast: Module,
    /// Resolved import edges, in source order.
    pub imports: Vec<Edge>,
    /// The entry file's `import` that first reached this module; `None` for
    /// the entry. See [`Program::via`].
    pub via: Option<Range<usize>>,
}

/// One `import` statement, with its target already resolved.
pub struct Edge {
    pub target: Target,
    pub names: ImportNames,
    pub span: std::ops::Range<usize>,
    /// The path as the `import` wrote it, for naming it in a diagnostic.
    pub raw: String,
}

/// What an `import` points at.
pub enum Target {
    /// Another Saule module, by index into the unit list.
    Module(usize),
    /// A native package: Rust-built values with no Saule source behind
    /// them. Nothing to compile and nothing to run, so its exports are
    /// resolved at compile time like prelude names.
    Native(std::collections::HashMap<String, saule_runtime::Value>),
    /// A dynamic native package: a TOML manifest plus a shared library.
    ///
    /// Folded at compile time like [`Native`](Target::Native), because the
    /// manifest — already parsed, and parsed without touching the binary —
    /// carries every name, symbol and arity the compiler needs. What the
    /// manifest cannot give is the code, so each folded method defers its
    /// symbol lookup, and the `dlopen` happens at run time from
    /// [`Chunk::dynamic_imports`].
    Dynamic {
        /// Import name, e.g. `engine`.
        package: String,
        exports: std::collections::HashMap<String, saule_runtime::Value>,
    },
}

/// Anything that can go wrong turning a file tree into a [`Program`].
#[derive(Debug, thiserror::Error, miette::Diagnostic)]
pub enum ProgramError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    Compile(#[from] CompileError),

    /// A module the entry file imports is wrong — it could not be found,
    /// read or parsed, it failed the front end, it names something another
    /// module does not export, or it is part of an import cycle.
    ///
    /// Reported the way the language has always reported an import problem:
    /// against the `import` in the entry file that led to it
    /// (`RuntimeError::ImportFailed`), carrying the offending module's own
    /// snippet inside — or, for a problem with one of the entry file's own
    /// `import`s, as a plain `RuntimeError::ImportError` at that `import`.
    #[error(transparent)]
    #[diagnostic(transparent)]
    Import(#[from] RuntimeError),

    /// The entry file itself failed the front end. Only reachable through
    /// [`compile`] on a file nobody checked first — `saule run` checks the
    /// entry before it gets here — and carries its own snippet.
    #[error(transparent)]
    #[diagnostic(transparent)]
    Entry(Box<ImportedDiagnostic>),
}

/// A module that failed to load, on its way up the import graph.
///
/// Each level re-anchors it at the `import` that named the module below, so
/// by the time it reaches the entry file it points at the entry's own
/// `import` — the one line the user can see from where they are.
enum Failure {
    /// Wrong in the module that returned it, with that module's snippet
    /// already attached.
    Local {
        diag: Box<ImportedDiagnostic>,
        label: String,
    },
    /// Already an `ImportFailed` about a module further down, anchored at
    /// an `import` in the module that returned it.
    Nested(RuntimeError),
    /// The file behind an `import` could not be read. Reported at that
    /// `import`, so it belongs to the module that has one.
    Unreadable(String),
}

impl Failure {
    /// The failure as the module whose `import` at `span` named the failing
    /// one sees it.
    fn at_import(self, span: Range<usize>) -> Failure {
        match self {
            Failure::Local { diag, label } => Failure::Nested(RuntimeError::ImportFailed {
                module_label: label,
                import_span: span,
                inner: diag,
            }),
            Failure::Nested(RuntimeError::ImportFailed {
                module_label,
                inner,
                ..
            }) => Failure::Nested(RuntimeError::ImportFailed {
                module_label,
                import_span: span,
                inner,
            }),
            Failure::Nested(other) => Failure::Nested(other),
            Failure::Unreadable(message) => {
                Failure::Nested(RuntimeError::ImportError { message, span })
            }
        }
    }

    /// The failure as the whole program's result.
    fn into_error(self) -> ProgramError {
        match self {
            Failure::Local { diag, .. } => ProgramError::Entry(diag),
            Failure::Nested(e) => ProgramError::Import(e),
            Failure::Unreadable(message) => ProgramError::Import(RuntimeError::ImportError {
                message,
                span: 0..0,
            }),
        }
    }
}

/// `diag` as a problem in `unit`: plain for the entry file, whose snippet
/// the caller already has, and otherwise anchored at the entry's `import`
/// that reached `unit`.
fn in_unit(unit: &Unit, diag: &dyn miette::Diagnostic) -> ProgramError {
    let local = ImportedDiagnostic::from_inner(diag, unit.label.clone(), unit.source.clone());
    match &unit.via {
        None => ProgramError::Entry(Box::new(local)),
        Some(span) => ProgramError::Import(RuntimeError::ImportFailed {
            module_label: unit.label.clone(),
            import_span: span.clone(),
            inner: Box::new(local),
        }),
    }
}

/// Compile the whole program rooted at `entry`.
///
/// Modules are compiled in post-order, accumulating one shared type world,
/// so a class declared in one module and extended in another has exactly one
/// layout (§24.2).
pub fn compile(entry: &Path) -> Result<Program, ProgramError> {
    let (mut units, entry_idx) = load_units(entry)?;

    let mut tables = crate::compile::Tables::default();
    let mut exports: Vec<HashMap<String, Export>> = vec![HashMap::new(); units.len()];
    let mut chunks: Vec<Chunk> = Vec::with_capacity(units.len());
    // For naming the module an `import` asked for, in a diagnostic.
    let paths: Vec<PathBuf> = units.iter().map(|u| u.path.clone()).collect();
    let via: Vec<Option<Range<usize>>> = units.iter().map(|u| u.via.clone()).collect();

    for (i, unit) in units.iter_mut().enumerate() {
        let dir = unit
            .path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));

        // The front end runs **per module**, and its two side tables cannot
        // be shared: `NodeId`s are per module and every module numbers from
        // zero, so one module's `TypeTable` entry would answer another
        // module's question.
        //
        // Its diagnostics are the only report an imported module gets:
        // `saule run` checks the entry file, and nothing else reads the
        // files it imports. The first one is reported, semantic before
        // type, because the type pass assumes a structurally valid module.
        let seed = saule_runtime::module::collect_import_seed(&unit.ast, &dir);
        let (errors, bindings) = saule_semantic::analyze_with_bindings(&unit.ast, seed);
        if let Some(e) = errors.first() {
            return Err(in_unit(unit, e));
        }
        // Resolving, not just checking: these units were parsed here, from
        // disk, so their `as` nodes have never met a typechecker. Compiling
        // them unresolved would emit `CASTCHK` where the program means a
        // conversion.
        let (errors, types) = saule_typeck::check_and_resolve_with_types(&mut unit.ast);
        if let Some(e) = errors.first() {
            return Err(in_unit(unit, e));
        }

        let imported = imported_layouts(unit, &paths, &exports, &bindings)?;
        let (mut chunk, layouts) = crate::compile::compile_into(
            &unit.ast,
            &unit.label,
            &unit.source,
            &bindings,
            &types,
            &imported.layouts,
            &mut tables,
            true,
            imported.values,
            i,
            imported.natives,
        )?;
        chunk.dynamic_imports = imported.dynamic;
        // An imported class or enum named as a value reads this module's
        // own slot for it — see `Chunk::type_slots`.
        for (local, ty) in imported.type_slots {
            if let Ok(global) = u16::try_from(chunk.module_slot_base + local as usize) {
                chunk.type_slots.push((global, ty));
            }
        }
        let exported =
            collect_exports(unit, chunk.module_slot_base, &layouts, &bindings, &exports)?;
        // A re-exported `fn` answers about its parameters at its new slot
        // too — see `Exported::value_aliases`.
        for (dst, src) in &exported.value_aliases {
            if let Some(params) = tables.fn_params_by_slot.get(src).cloned() {
                tables.fn_params_by_slot.insert(*dst, params);
            }
        }
        exports[i] = exported.names;
        chunks.push(chunk);
    }

    // Only now do the tables become shared. Up to this point exactly one
    // `Rc` existed, which is what let every module mutate through
    // `Chunk::classes_mut`'s `Rc::get_mut`.
    let classes = Rc::new(tables.classes);
    let interfaces = Rc::new(tables.interfaces);
    let enums = Rc::new(tables.enums);
    let modules = chunks
        .into_iter()
        .map(|mut c| {
            c.classes = Rc::clone(&classes);
            c.interfaces = Rc::clone(&interfaces);
            c.enums = Rc::clone(&enums);
            Rc::new(c)
        })
        .collect();

    Ok(Program {
        modules,
        entry: entry_idx,
        via,
    })
}

/// What a module's `import` statements bring into scope, as program-global
/// indices.
///
/// A type resolves entirely here and costs nothing at run time. An imported
/// **value** — an exported `fn` or module variable — lives in the exporting
/// module's slot, so it comes back as an [`ImportBinding`] for the prologue
/// to copy across.
///
/// A name that resolves to *neither* must be refused rather than skipped:
/// `collect_module_scope` gives every imported name a module slot, so
/// compiling on would emit a `GETMOD` against a slot nothing ever writes —
/// `nil`, silently.
/// What a module's `import`s resolve to.
struct Imported {
    /// The layouts the imported types bring into scope.
    layouts: crate::compile::layout::Layouts,
    /// Value bindings to copy in before the body runs.
    values: Vec<ImportBinding>,
    /// Native-package exports, folded at compile time.
    natives: HashMap<String, saule_runtime::Value>,
    /// Dynamic packages this module imports, in source order — see
    /// [`Chunk::dynamic_imports`], which this becomes.
    dynamic: Vec<(String, std::ops::Range<usize>)>,
    /// Imported classes and enums, by this module's *local* slot for the
    /// name — see [`Chunk::type_slots`].
    type_slots: Vec<(u16, crate::chunk::TypeSlot)>,
}

/// The language's error for `import { name } from x` when `x` exports no
/// `name` — the tree-walker's wording, reported in the importing module.
fn not_exported(unit: &Unit, name: &str, from: &str, span: &Range<usize>) -> ProgramError {
    let e = RuntimeError::ImportError {
        message: format!("`{name}` is not exported from `{from}`"),
        span: span.clone(),
    };
    // In the entry file it stays plain, so it renders against the source
    // and name the caller already has.
    match unit.via {
        None => ProgramError::Import(e),
        Some(_) => in_unit(unit, &e),
    }
}

/// Bind a native package's exports into `natives` under the names the
/// `import` asks for. Shared by the static and dynamic cases: once the
/// exports exist, the two are the same thing to the compiler — a fixed set
/// of values, resolved before the program runs.
fn fold_native(
    unit: &Unit,
    edge: &Edge,
    vals: &HashMap<String, saule_runtime::Value>,
    natives: &mut HashMap<String, saule_runtime::Value>,
) -> Result<(), ProgramError> {
    match &edge.names {
        ImportNames::All => natives.extend(vals.iter().map(|(k, v)| (k.clone(), v.clone()))),
        ImportNames::List(items) => {
            for (orig, alias) in items {
                let Some(v) = vals.get(orig) else {
                    return Err(not_exported(unit, orig, &edge.raw, &edge.span));
                };
                natives.insert(alias.clone().unwrap_or_else(|| orig.clone()), v.clone());
            }
        }
    }
    Ok(())
}

fn imported_layouts(
    unit: &Unit,
    paths: &[PathBuf],
    exports: &[HashMap<String, Export>],
    bindings: &saule_semantic::Bindings,
) -> Result<Imported, ProgramError> {
    let mut out = crate::compile::layout::Layouts::default();
    let mut values: Vec<ImportBinding> = Vec::new();
    let mut natives: HashMap<String, saule_runtime::Value> = HashMap::new();
    let mut dynamic: Vec<(String, std::ops::Range<usize>)> = Vec::new();
    let mut type_slots = Vec::new();
    let slot_of = |name: &str| {
        bindings
            .module_slots
            .iter()
            .position(|s| s.as_ref() == name)
            .and_then(|i| u16::try_from(i).ok())
    };
    for edge in &unit.imports {
        // A native package's exports fold at compile time, so they never
        // reach a module slot at all.
        let (from, from_path) = match &edge.target {
            Target::Native(vals) => {
                fold_native(unit, edge, vals, &mut natives)?;
                continue;
            }
            // The same fold, plus a note to load the library at run time.
            // Recorded per `import` rather than per package: two modules
            // importing one package each load it where *they* would have,
            // and the cache in `load_library` makes the second a no-op.
            Target::Dynamic { package, exports } => {
                fold_native(unit, edge, exports, &mut natives)?;
                dynamic.push((package.clone(), edge.span.clone()));
                continue;
            }
            Target::Module(i) => (&exports[*i], &paths[*i]),
        };
        // `import * from x` binds every export; a named list binds the ones
        // it asks for, under the alias when there is one.
        let wanted: Vec<(&String, String)> = match &edge.names {
            ImportNames::All => from.keys().map(|k| (k, k.clone())).collect(),
            ImportNames::List(items) => items
                .iter()
                .map(|(orig, alias)| (orig, alias.clone().unwrap_or_else(|| orig.clone())))
                .collect(),
        };
        for (orig, local) in wanted {
            match from.get(orig) {
                Some(Export::Class(i)) => {
                    if let Some(s) = slot_of(&local) {
                        type_slots.push((s, crate::chunk::TypeSlot::Class(*i)));
                    }
                    out.index.insert(local, *i);
                }
                Some(Export::Enum(i)) => {
                    if let Some(s) = slot_of(&local) {
                        type_slots.push((s, crate::chunk::TypeSlot::Enum(*i)));
                    }
                    out.enums.insert(local, *i);
                }
                Some(Export::Interface(i)) => {
                    out.interfaces.insert(local, *i);
                }
                // A native package's export, re-published by a barrel: a
                // constant, folded like any other native import.
                Some(Export::Native(v)) => {
                    natives.insert(local, v.clone());
                }
                Some(Export::Value { slot: from }) => {
                    let from = *from;
                    let Some(local) = slot_of(&local) else {
                        // No slot for a name the resolver did bind is a
                        // compiler bug, not a user error — refuse rather
                        // than drop the copy on the floor.
                        return Err(ProgramError::Compile(CompileError::unsupported(
                            "an imported name with no module slot",
                            edge.span.clone(),
                        )));
                    };
                    values.push(ImportBinding { local, from });
                }
                None => {
                    let from = from_path.display().to_string();
                    return Err(not_exported(unit, orig, &from, &edge.span));
                }
            }
        }
    }
    Ok(Imported {
        layouts: out,
        values,
        natives,
        dynamic,
        type_slots,
    })
}

/// What one name this module holds looks like to an importer.
///
/// `None` where there is nothing to publish — the tree-walker's
/// `env.get(name)` would come back empty too.
fn export_of(
    name: &str,
    slot_base: usize,
    layouts: &crate::compile::layout::Layouts,
    bindings: &saule_semantic::Bindings,
) -> Option<Export> {
    // Types first: a class and a function cannot share a name, so the order
    // only decides which lookup answers, not which is right.
    //
    // `layouts` here is this module's *whole* type scope, imports included —
    // `layout::build` seeds it with them so a subclass can extend an
    // imported parent — which is exactly what makes a barrel's re-export of
    // a type fall out with no extra lookup, and with the same program-global
    // index the declaring module assigned.
    if let Some(i) = layouts.get(name) {
        return Some(Export::Class(i));
    }
    if let Some(i) = layouts.enum_of(name) {
        return Some(Export::Enum(i));
    }
    if let Some(i) = layouts.interface_of(name) {
        return Some(Export::Interface(i));
    }
    let slot = bindings
        .module_slots
        .iter()
        .position(|s| s.as_ref() == name)?;
    // Rebased on the way out, so an importer never has to know which module
    // a value came from.
    u16::try_from(slot_base + slot)
        .ok()
        .map(|slot| Export::Value { slot })
}

/// What this module publishes to its importers.
///
/// Normally only `export`ed declarations, matching `module::collect_exports`
/// in the tree-walker — a name without `export` stays private.
///
/// ## Barrels
///
/// An `init.sau` is a **barrel**: it also publishes everything its `import`
/// statements brought in, which is what lets a folder of files be consumed
/// as one module. `examples/UI Project` is built on it — `UIKit/init.sau`
/// re-exports two dozen siblings and declares nothing of its own — and until
/// this existed here, a class declared behind the barrel was invisible to
/// the modules that extend it, which surfaced as the wrong diagnostic
/// entirely: `a class extending one the compiler cannot see`.
///
/// The rule is `module::is_init_module`, called rather than restated: which
/// modules re-export is a language rule, and two engines each with their own
/// copy of it is how they drift.
///
/// **A re-exported value publishes the barrel's own slot, not the original
/// module's.** The barrel's prologue copies the value into that slot when
/// the barrel runs, and the tree-walker's barrel snapshots
/// `env.get(name)` at exactly the same moment — so forwarding the source
/// slot instead would hand a later importer a *fresher* value than the
/// tree-walker gives it, in the one case where something mutated the
/// original in between. Types have no such question: the index is
/// program-global and there is only one of it.
///
/// Statements are walked in source order, imports included, because that is
/// the order the tree-walker resolves collisions in — a barrel that both
/// declares `X` and imports one publishes whichever came last.
struct Exported {
    names: HashMap<String, Export>,
    /// `(destination, source)` global slot pairs for values a barrel
    /// re-published under a slot of its own.
    ///
    /// The two slots hold the same function, so whatever the program knows
    /// about the one it knows about the other — `Tables::fn_params_by_slot`
    /// in particular, which §19's call-site binding reads and which is keyed
    /// on the slot the *declaring* module exported from. Without carrying it
    /// across, a `fn` reached through a barrel had no parameter list, and
    /// every named argument or trailing block on it refused with
    /// `a named argument to a callee the compiler cannot identify`.
    ///
    /// Classes need no equivalent: `Tables::method_params` is keyed on a
    /// program-global `ClassIdx`, which a re-export does not change.
    value_aliases: Vec<(u16, u16)>,
}

fn collect_exports(
    unit: &Unit,
    slot_base: usize,
    layouts: &crate::compile::layout::Layouts,
    bindings: &saule_semantic::Bindings,
    exports: &[HashMap<String, Export>],
) -> Result<Exported, ProgramError> {
    let barrel = saule_runtime::module::is_init_module(&unit.path);
    let mut out = HashMap::new();
    let mut value_aliases = Vec::new();
    // `unit.imports` is one edge per `Decl::Import`, in source order, so a
    // running index keeps the two in step without re-resolving anything.
    let mut edge = 0usize;
    for stmt in &unit.ast.stmts {
        let Stmt::Decl(d) = &stmt.value else { continue };
        match &d.value {
            Decl::Class {
                exported: true,
                name,
                ..
            }
            | Decl::Interface {
                exported: true,
                name,
                ..
            }
            | Decl::Enum {
                exported: true,
                name,
                ..
            }
            | Decl::Function {
                exported: true,
                name,
                ..
            }
            | Decl::Variable {
                exported: true,
                name,
                ..
            } => {
                if let Some(e) = export_of(name, slot_base, layouts, bindings) {
                    out.insert(name.clone(), e);
                }
            }
            Decl::Import { names, .. } => {
                let Some(this) = unit.imports.get(edge) else {
                    continue;
                };
                edge += 1;
                if !barrel {
                    continue;
                }
                let from = match &this.target {
                    Target::Module(i) => &exports[*i],
                    // A native package's exports are folded into constants
                    // at compile time — they reach no module slot and no
                    // layout table — so the barrel republishes the values
                    // themselves, and each importer folds them in turn. A
                    // dynamic package's library is loaded when the barrel
                    // runs, which post-order puts before any importer.
                    Target::Native(vals) | Target::Dynamic { exports: vals, .. } => {
                        let pairs: Vec<(String, &String)> = match names {
                            ImportNames::All => vals.keys().map(|k| (k.clone(), k)).collect(),
                            ImportNames::List(items) => items
                                .iter()
                                .map(|(orig, alias)| {
                                    (alias.clone().unwrap_or_else(|| orig.clone()), orig)
                                })
                                .collect(),
                        };
                        for (local, orig) in pairs {
                            if let Some(v) = vals.get(orig) {
                                out.insert(local, Export::Native(v.clone()));
                            }
                        }
                        continue;
                    }
                };
                // The names this import bound *locally* — under their
                // aliases, since that is what the barrel published them as.
                let locals: Vec<String> = match names {
                    ImportNames::All => from.keys().cloned().collect(),
                    ImportNames::List(items) => items
                        .iter()
                        .map(|(orig, alias)| alias.clone().unwrap_or_else(|| orig.clone()))
                        .collect(),
                };
                for local in locals {
                    let Some(e) = export_of(&local, slot_base, layouts, bindings) else {
                        continue;
                    };
                    // A value moved to a slot of the barrel's own; note the
                    // pair so what the program knows about the source slot
                    // follows it there.
                    if let (Export::Value { slot: dst }, Some(Export::Value { slot: src })) =
                        (&e, from.get(&local))
                    {
                        value_aliases.push((*dst, *src));
                    }
                    out.insert(local, e);
                }
            }
            _ => {}
        }
    }
    Ok(Exported {
        names: out,
        value_aliases,
    })
}

/// Read, parse and topologically order every module reachable from `entry`.
///
/// Post-order by construction: a unit is pushed only after everything it
/// imports has been pushed, so `units[i]` never depends on `units[j]` for
/// `j > i`.
pub fn load_units(entry: &Path) -> Result<(Vec<Unit>, usize), ProgramError> {
    load_units_inner(entry).map_err(Failure::into_error)
}

fn load_units_inner(entry: &Path) -> Result<(Vec<Unit>, usize), Failure> {
    let mut units: Vec<Unit> = Vec::new();
    let mut index: HashMap<PathBuf, usize> = HashMap::new();
    let mut in_flight: HashSet<PathBuf> = HashSet::new();
    let entry_idx = load_one(entry, None, &mut units, &mut index, &mut in_flight)?;
    Ok((units, entry_idx))
}

/// Load the module at `path` and, first, everything it imports.
///
/// `via` is the span of the entry file's `import` this module was reached
/// through — `None` for the entry. A failure comes back anchored in the
/// module that has the problem (see [`Failure`]); each caller re-anchors it
/// at its own `import` on the way up.
fn load_one(
    path: &Path,
    via: Option<Range<usize>>,
    units: &mut Vec<Unit>,
    index: &mut HashMap<PathBuf, usize>,
    in_flight: &mut HashSet<PathBuf>,
) -> Result<usize, Failure> {
    let abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if let Some(&i) = index.get(&abs) {
        return Ok(i);
    }
    in_flight.insert(abs.clone());

    let source = std::fs::read_to_string(&abs)
        .map_err(|e| Failure::Unreadable(format!("could not read `{}`: {e}", abs.display())))?;
    let label = saule_runtime::project::pretty_path(&abs);
    // A problem in this module, reported with its own snippet.
    let here = |e: &dyn miette::Diagnostic| Failure::Local {
        diag: Box::new(ImportedDiagnostic::from_inner(
            e,
            label.clone(),
            source.clone(),
        )),
        label: label.clone(),
    };
    // A problem with one of this module's own `import`s. In the entry file
    // it stays plain, so it renders against the source and name the caller
    // already has.
    let import_error = |message: String, span: &Range<usize>| {
        let e = RuntimeError::ImportError {
            message,
            span: span.clone(),
        };
        match via {
            None => Failure::Nested(e),
            Some(_) => here(&e),
        }
    };

    let tokens = saule_lexer::Lexer::new(&source)
        .tokenize()
        .map_err(|e| here(&e))?;
    let ast = saule_parser::parse(tokens).map_err(|e| here(&e))?;

    let dir = abs
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    // Resolve every import *before* this unit is pushed, so the vector ends
    // up in post-order without a second sorting pass.
    let mut imports = Vec::new();
    for stmt in &ast.stmts {
        let Stmt::Decl(d) = &stmt.value else { continue };
        let Decl::Import {
            names, path: raw, ..
        } = &d.value
        else {
            continue;
        };
        let Some(target_path) = saule_runtime::module::resolve_import_path(&dir, raw) else {
            return Err(import_error(
                saule_runtime::module::unresolved_import_message(raw),
                &d.span,
            ));
        };
        // A native package is a bag of Rust-built values with no Saule
        // source behind it, so there is nothing to compile: its exports are
        // known before the program starts, exactly like the prelude.
        let target = if let Some(pkg) =
            saule_runtime::native_packages::name_from_sentinel(&target_path)
                .and_then(saule_runtime::native_packages::lookup)
        {
            Target::Native(saule_runtime::native_packages::build_exports(pkg).values)
        } else if let Some(pkg) = saule_runtime::dynamic_packages::name_from_sentinel(&target_path)
        {
            // A manifest-described shared library. The manifest is the part
            // the compiler needs — class names, method names, parameter
            // names, arities — and it is already parsed, from TOML, with the
            // binary untouched. So the exports fold here exactly like a
            // static package's, each method carrying a deferred symbol
            // lookup rather than a resolved pointer.
            //
            // The `dlopen` remains a runtime side effect that compiling must
            // not perform. It is recorded on the chunk instead and performed
            // by `run_program` just before this module's body runs.
            match saule_runtime::dynamic_packages::build_exports_deferred(pkg) {
                Some(e) => Target::Dynamic {
                    package: pkg.to_string(),
                    exports: e.values,
                },
                // No manifest behind the sentinel, or a build without
                // dynamic loading at all (wasm). The eager loader says which
                // — without loading anything, since both cases fail before
                // it reaches a library.
                None => {
                    let e = saule_runtime::dynamic_packages::build_exports(pkg, d.span.clone())
                        .err()
                        .unwrap_or_else(|| RuntimeError::ImportError {
                            message: format!("native package `{pkg}` could not be loaded"),
                            span: d.span.clone(),
                        });
                    return Err(here(&e));
                }
            }
        } else {
            let child = target_path
                .canonicalize()
                .unwrap_or_else(|_| target_path.clone());
            // Still loading: this `import` closes a cycle, and it is this
            // module's `import` that does it.
            if in_flight.contains(&child) && !index.contains_key(&child) {
                return Err(import_error(
                    format!("circular import detected at `{}`", child.display()),
                    &d.span,
                ));
            }
            // The entry file's own `import`s are where a failure below it is
            // reported; deeper modules inherit the one that reached them.
            let child_via = via.clone().or_else(|| Some(d.span.clone()));
            let i = load_one(&target_path, child_via, units, index, in_flight)
                .map_err(|f| f.at_import(d.span.clone()))?;
            Target::Module(i)
        };
        imports.push(Edge {
            target,
            names: names.clone(),
            span: d.span.clone(),
            raw: raw.clone(),
        });
    }

    in_flight.remove(&abs);

    units.push(Unit {
        path: abs.clone(),
        label,
        source,
        ast,
        imports,
        via,
    });
    let i = units.len() - 1;
    index.insert(abs, i);
    Ok(i)
}
