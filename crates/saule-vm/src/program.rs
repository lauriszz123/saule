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
use std::path::{Path, PathBuf};
use std::rc::Rc;

use saule_ast::{Decl, ImportNames, Module, Stmt};

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
#[derive(Debug, Clone, Copy)]
pub enum Export {
    Class(ClassIdx),
    Enum(EnumIdx),
    Interface(InterfaceIdx),
    /// An exported `fn` or module variable, as a slot in the program's flat
    /// slot space — already rebased, so an importer can read it directly.
    Value { slot: u16 },
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
}

/// One `import` statement, with its target already resolved.
pub struct Edge {
    pub target: Target,
    pub names: ImportNames,
    pub span: std::ops::Range<usize>,
}

/// What an `import` points at.
pub enum Target {
    /// Another Saule module, by index into the unit list.
    Module(usize),
    /// A native package: Rust-built values with no Saule source behind
    /// them. Nothing to compile and nothing to run, so its exports are
    /// resolved at compile time like prelude names.
    Native(std::collections::HashMap<String, saule_interpreter::Value>),
}

/// Anything that can go wrong turning a file tree into a [`Program`].
#[derive(Debug, thiserror::Error, miette::Diagnostic)]
pub enum ProgramError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    Compile(#[from] CompileError),

    /// A module could not be read, parsed, or resolved.
    ///
    /// Deliberately *not* a hard error at the CLI: the tree-walker resolves
    /// imports its own way and may well succeed where this does. Treated the
    /// same as `Unsupported` — fall back and let the oracle produce the
    /// user-facing diagnostic, so the two engines never disagree about
    /// whether a program is valid.
    #[error("cannot build a program from `{path}`: {detail}")]
    Unreadable { path: String, detail: String },

    /// An import cycle. The tree-walker reports one at run time; refusing
    /// here means the VM never tries to lay out a class whose parent is
    /// still being laid out.
    #[error("circular import involving `{path}`")]
    Circular { path: String },
}

impl ProgramError {
    /// Whether the CLI should fall back to the tree-walker rather than
    /// surface this. True for everything except a genuine compiler bug.
    pub fn is_fallback(&self) -> bool {
        match self {
            ProgramError::Compile(CompileError::Unsupported { .. }) => true,
            ProgramError::Compile(_) => false,
            ProgramError::Unreadable { .. } | ProgramError::Circular { .. } => true,
        }
    }
}

/// Compile the whole program rooted at `entry`.
///
/// Modules are compiled in post-order, accumulating one shared type world,
/// so a class declared in one module and extended in another has exactly one
/// layout (§24.2).
pub fn compile(entry: &Path) -> Result<Program, ProgramError> {
    let (units, entry_idx) = load_units(entry)?;

    let mut tables = crate::compile::Tables::default();
    let mut exports: Vec<HashMap<String, Export>> = vec![HashMap::new(); units.len()];
    let mut chunks: Vec<Chunk> = Vec::with_capacity(units.len());

    for (i, unit) in units.iter().enumerate() {
        let dir = unit
            .path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));

        // The front end runs **per module**, and its two side tables cannot
        // be shared: `NodeId`s are per module and every module numbers from
        // zero, so one module's `TypeTable` entry would answer another
        // module's question. Diagnostics are discarded — this function's
        // precondition is that the program already checked clean, and the
        // tree-walker is the engine that reports.
        let seed = saule_interpreter::module::collect_import_seed(&unit.ast, &dir);
        let (_, bindings) = saule_semantic::analyze_with_bindings(&unit.ast, seed);
        let (_, types) = saule_typeck::check_with_types(&unit.ast);

        let (imported, import_bindings, natives) = imported_layouts(unit, &exports, &bindings)?;
        let (chunk, layouts) = crate::compile::compile_into(
            &unit.ast,
            &unit.label,
            &unit.source,
            &bindings,
            &types,
            &imported,
            &mut tables,
            true,
            import_bindings,
            i,
            natives,
        )?;
        exports[i] = collect_exports(unit, chunk.module_slot_base, &layouts, &bindings);
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
fn imported_layouts(
    unit: &Unit,
    exports: &[HashMap<String, Export>],
    bindings: &saule_semantic::Bindings,
) -> Result<
    (
        crate::compile::layout::Layouts,
        Vec<ImportBinding>,
        HashMap<String, saule_interpreter::Value>,
    ),
    ProgramError,
> {
    let mut out = crate::compile::layout::Layouts::default();
    let mut values: Vec<ImportBinding> = Vec::new();
    let mut natives: HashMap<String, saule_interpreter::Value> = HashMap::new();
    for edge in &unit.imports {
        // A native package's exports fold at compile time, so they never
        // reach a module slot at all.
        let from = match &edge.target {
            Target::Native(vals) => {
                match &edge.names {
                    ImportNames::All => natives.extend(
                        vals.iter().map(|(k, v)| (k.clone(), v.clone())),
                    ),
                    ImportNames::List(items) => {
                        for (orig, alias) in items {
                            let Some(v) = vals.get(orig) else {
                                return Err(ProgramError::Compile(CompileError::unsupported(
                                    "an import of a name the package does not export",
                                    edge.span.clone(),
                                )));
                            };
                            natives.insert(
                                alias.clone().unwrap_or_else(|| orig.clone()),
                                v.clone(),
                            );
                        }
                    }
                }
                continue;
            }
            Target::Module(i) => &exports[*i],
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
                    out.index.insert(local, *i);
                }
                Some(Export::Enum(i)) => {
                    out.enums.insert(local, *i);
                }
                Some(Export::Interface(i)) => {
                    out.interfaces.insert(local, *i);
                }
                Some(Export::Value { slot: from }) => {
                    let from = *from;
                    let Some(local) = bindings
                        .module_slots
                        .iter()
                        .position(|s| s.as_ref() == local.as_str())
                    else {
                        // No slot for a name the resolver did bind is a
                        // compiler bug, not a user error — refuse rather
                        // than drop the copy on the floor.
                        return Err(ProgramError::Compile(CompileError::unsupported(
                            "an imported name with no module slot",
                            edge.span.clone(),
                        )));
                    };
                    let Ok(local) = u16::try_from(local) else {
                        return Err(ProgramError::Compile(CompileError::unsupported(
                            "a program with over 65536 top-level names",
                            edge.span.clone(),
                        )));
                    };
                    values.push(ImportBinding { local, from });
                }
                // A named import of something the target does not export.
                // The tree-walker reports this at run time; refusing keeps
                // the two engines agreeing about which programs are valid.
                None => {
                    return Err(ProgramError::Compile(CompileError::unsupported(
                        "an import of a name the module does not export",
                        edge.span.clone(),
                    )));
                }
            }
        }
    }
    Ok((out, values, natives))
}

/// What this module publishes to its importers.
///
/// Only `export`ed declarations, matching `module::collect_exports` in the
/// tree-walker — a name without `export` stays private.
fn collect_exports(
    unit: &Unit,
    slot_base: usize,
    layouts: &crate::compile::layout::Layouts,
    bindings: &saule_semantic::Bindings,
) -> HashMap<String, Export> {
    let mut out = HashMap::new();
    for stmt in &unit.ast.stmts {
        let Stmt::Decl(d) = &stmt.value else { continue };
        let name = match &d.value {
            Decl::Class { exported: true, name, .. }
            | Decl::Interface { exported: true, name, .. }
            | Decl::Enum { exported: true, name, .. }
            | Decl::Function { exported: true, name, .. }
            | Decl::Variable { exported: true, name, .. } => name,
            _ => continue,
        };
        // Types first: a class and a function cannot share a name, so the
        // order only decides which lookup answers, not which is right.
        let export = if let Some(i) = layouts.get(name) {
            Export::Class(i)
        } else if let Some(i) = layouts.enum_of(name) {
            Export::Enum(i)
        } else if let Some(i) = layouts.interface_of(name) {
            Export::Interface(i)
        } else {
            match bindings.module_slots.iter().position(|s| s.as_ref() == name.as_str()) {
                // Rebased on the way out, so an importer never has to know
                // which module a value came from.
                Some(slot) => match u16::try_from(slot_base + slot) {
                    Ok(slot) => Export::Value { slot },
                    Err(_) => continue,
                },
                // No slot and no type: nothing to publish. The tree-walker
                // would find nothing either.
                None => continue,
            }
        };
        out.insert(name.clone(), export);
    }
    out
}

/// Read, parse and topologically order every module reachable from `entry`.
///
/// Post-order by construction: a unit is pushed only after everything it
/// imports has been pushed, so `units[i]` never depends on `units[j]` for
/// `j > i`.
pub fn load_units(entry: &Path) -> Result<(Vec<Unit>, usize), ProgramError> {
    let mut units: Vec<Unit> = Vec::new();
    let mut index: HashMap<PathBuf, usize> = HashMap::new();
    let mut in_flight: HashSet<PathBuf> = HashSet::new();
    let entry_idx = load_one(entry, &mut units, &mut index, &mut in_flight)?;
    Ok((units, entry_idx))
}

fn load_one(
    path: &Path,
    units: &mut Vec<Unit>,
    index: &mut HashMap<PathBuf, usize>,
    in_flight: &mut HashSet<PathBuf>,
) -> Result<usize, ProgramError> {
    let abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if let Some(&i) = index.get(&abs) {
        return Ok(i);
    }
    if !in_flight.insert(abs.clone()) {
        return Err(ProgramError::Circular {
            path: abs.display().to_string(),
        });
    }

    let unreadable = |detail: String| ProgramError::Unreadable {
        path: abs.display().to_string(),
        detail,
    };

    let source = std::fs::read_to_string(&abs).map_err(|e| unreadable(e.to_string()))?;
    let tokens = saule_lexer::Lexer::new(&source)
        .tokenize()
        .map_err(|e| unreadable(e.to_string()))?;
    let ast = saule_parser::parse(tokens).map_err(|e| unreadable(e.to_string()))?;

    let dir = abs
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    // Resolve every import *before* this unit is pushed, so the vector ends
    // up in post-order without a second sorting pass.
    let mut imports = Vec::new();
    for stmt in &ast.stmts {
        let Stmt::Decl(d) = &stmt.value else { continue };
        let Decl::Import { names, path: raw, .. } = &d.value else {
            continue;
        };
        let Some(target_path) = saule_interpreter::module::resolve_import_path(&dir, raw) else {
            return Err(unreadable(format!("could not resolve import `{raw}`")));
        };
        // A native package is a bag of Rust-built values with no Saule
        // source behind it, so there is nothing to compile: its exports are
        // known before the program starts, exactly like the prelude.
        let target = if let Some(pkg) =
            saule_interpreter::native_packages::name_from_sentinel(&target_path)
                .and_then(saule_interpreter::native_packages::lookup)
        {
            Target::Native(saule_interpreter::native_packages::build_exports(pkg).values)
        } else if saule_interpreter::dynamic_packages::name_from_sentinel(&target_path).is_some() {
            // A manifest-described shared library. Loading one is a runtime
            // side effect — `dlopen` — that compiling must not perform, so
            // this still refuses and falls back.
            return Err(ProgramError::Compile(CompileError::unsupported(
                "an import of a dynamic native package",
                d.span.clone(),
            )));
        } else {
            Target::Module(load_one(&target_path, units, index, in_flight)?)
        };
        imports.push(Edge {
            target,
            names: names.clone(),
            span: d.span.clone(),
        });
    }

    in_flight.remove(&abs);

    let label = saule_interpreter::project::pretty_path(&abs);
    units.push(Unit {
        path: abs.clone(),
        label,
        source,
        ast,
        imports,
    });
    let i = units.len() - 1;
    index.insert(abs, i);
    Ok(i)
}
