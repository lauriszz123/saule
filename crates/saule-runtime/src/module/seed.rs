//! Collecting the names an import brings into scope, without
//! executing the module. Used by the checker and the LSP so a
//! not-yet-run import still resolves.

use saule_ast::{Decl, ImportNames, Module, Stmt};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::*;

/// Walk every `import ... from "path"` statement in `module`, resolve the
/// target file, parse it, and harvest its exported class / interface /
/// enum metadata into a [`saule_semantic::ModuleSeed`]. Returned to the
/// caller so they can hand it to [`saule_semantic::analyze_with_seed`] —
/// the result lets the typechecker know the return types of imported
/// methods like `Json.decode(...)`.
///
/// Best-effort: any import that fails to resolve, read, or parse is
/// silently skipped — semantic/typeck will surface the user-facing error
/// (or, in the import-fails case, the program driver will).
pub fn collect_import_seed(module: &Module, dir: &Path) -> saule_semantic::ModuleSeed {
    collect_import_seed_with(module, dir, &|_| None)
}

/// Where an imported module's source text comes from.
///
/// Returning `Some(text)` supplies that file's contents in place of what is
/// on disk; `None` means "read the file normally".
///
/// This exists for the language server. Its authoritative copy of an open
/// file is the buffer the editor last sent, which for an unsaved file is *not*
/// what `read_to_string` returns — so without an overlay, `main.sau`'s
/// diagnostics, hover, and completions were all computed against a stale
/// on-disk version of every file it imports. Editing a module and watching its
/// importer's errors fail to update was the visible symptom.
pub type SourceOverlay<'a> = &'a dyn Fn(&Path) -> Option<String>;

/// Where an imported module's parse *tree* comes from.
///
/// Returning `Some(tree)` skips reading and parsing that file entirely;
/// `None` means "read and parse it as usual".
///
/// This exists because the walk is the same walk for every file in a
/// project, and the modules near the root of the import graph are therefore
/// parsed once per importer. A caller that already keeps parse trees — the
/// language server, `saule check` — hands them over instead, and the
/// duplicated work disappears. Only a *clean* tree may be supplied: the
/// walk's own parse is strict and skips a module it cannot parse, and a
/// recovered tree would smuggle declarations out of a file that does not
/// compile.
pub type ModuleSource<'a> = &'a dyn Fn(&Path) -> Option<std::sync::Arc<Module>>;

/// Where the walk gets what it needs to read a module.
#[derive(Clone, Copy)]
pub struct SeedIo<'a> {
    /// Consulted before the filesystem for a module's text.
    pub overlay: SourceOverlay<'a>,
    /// Consulted before either, for a module's tree.
    pub modules: Option<ModuleSource<'a>>,
}

impl<'a> SeedIo<'a> {
    /// Text from `overlay` or the filesystem; nothing pre-parsed.
    pub fn from_overlay(overlay: SourceOverlay<'a>) -> SeedIo<'a> {
        SeedIo {
            overlay,
            modules: None,
        }
    }
}

/// [`collect_import_seed`], but consulting `overlay` before the filesystem for
/// every module it reads — transitively, including through re-exports.
pub fn collect_import_seed_with(
    module: &Module,
    dir: &Path,
    overlay: SourceOverlay<'_>,
) -> saule_semantic::ModuleSeed {
    collect_import_seed_io(module, dir, SeedIo::from_overlay(overlay))
}

/// [`collect_import_seed_with`], with the option of being handed trees the
/// caller has already parsed. See [`ModuleSource`].
pub fn collect_import_seed_io(
    module: &Module,
    dir: &Path,
    io: SeedIo<'_>,
) -> saule_semantic::ModuleSeed {
    let mut visited = HashSet::new();
    let mut seed = collect_import_seed_inner(module, dir, &mut visited, 0, io);
    seed.wildcard_names = collect_wildcard_names(module, dir, io);
    seed
}

/// A module's tree: from the caller's cache if it has one, otherwise read
/// (through the overlay) and parsed here.
fn parsed_module(abs: &Path, io: SeedIo<'_>) -> Option<std::sync::Arc<Module>> {
    if let Some(provider) = io.modules
        && let Some(tree) = provider(abs)
    {
        return Some(tree);
    }
    let source = match (io.overlay)(abs) {
        Some(text) => text,
        None => std::fs::read_to_string(abs).ok()?,
    };
    // The same text parses to the same tree, so the last tree read from it
    // is handed back. Every file's seed reaches the same modules, and
    // without this a run parsed each of them once per file that imports
    // anything leading to it. Keyed on the text itself, so an edit is a
    // miss.
    let cached = PARSED.with(|c| {
        c.borrow()
            .get(abs)
            .filter(|(text, _)| *text == source)
            .map(|(_, tree)| tree.clone())
    });
    if let Some(tree) = cached {
        return Some(tree);
    }
    let tokens = saule_lexer::Lexer::new(&source).tokenize().ok()?;
    let tree = std::sync::Arc::new(saule_parser::parse(tokens).ok()?);
    PARSED.with(|c| {
        c.borrow_mut()
            .insert(abs.to_path_buf(), (source, tree.clone()))
    });
    Some(tree)
}

thread_local! {
    /// Trees [`parsed_module`] parsed itself, with the text they came from.
    static PARSED: std::cell::RefCell<HashMap<PathBuf, (String, Arc<Module>)>> =
        std::cell::RefCell::new(HashMap::new());
}

/// Union of the local names every `from "..." import *` in `module` binds.
///
/// `None` when at least one wildcard target couldn't be enumerated — the
/// name resolver reads that as "unknown names may be in scope here" and
/// stops reporting undefined names for the whole module. Enumerating them
/// is what lets a typo inside a file that globs a module still be caught.
pub(crate) fn collect_wildcard_names(
    module: &Module,
    dir: &Path,
    io: SeedIo<'_>,
) -> Option<HashSet<String>> {
    let mut out = HashSet::new();
    for stmt in &module.stmts {
        let Stmt::Decl(d) = &stmt.value else { continue };
        let Decl::Import {
            names: names @ (ImportNames::All | ImportNames::Namespace(_)),
            path,
            ..
        } = &d.value
        else {
            continue;
        };
        out.extend(static_import_names(dir, path, names, 0, io)?);
    }
    Some(out)
}

/// The local names one `import` statement binds, resolved statically —
/// from a package manifest or by parsing the target file, never by
/// executing anything.
///
/// `None` means the target couldn't be enumerated: an unresolvable path,
/// an unreadable or unparseable file, or a chain of re-exports nested deeper
/// than [`MAX_REEXPORT_DEPTH`].
pub(crate) fn static_import_names(
    dir: &Path,
    path: &str,
    names: &ImportNames,
    depth: usize,
    io: SeedIo<'_>,
) -> Option<Vec<String>> {
    // A name list is its own answer — no need to look at the target.
    if let ImportNames::List(_) = names {
        return Some(names.bind([]).into_iter().map(|(_, local)| local).collect());
    }
    let exports = static_export_names(dir, path, depth, io)?;
    Some(
        names
            .bind(exports.iter().map(String::as_str))
            .into_iter()
            .map(|(_, local)| local)
            .collect(),
    )
}

/// Every name the module at `path` exports: its own `export`ed declarations
/// and whatever its `export from … import …` statements pass on.
pub(crate) fn static_export_names(
    dir: &Path,
    path: &str,
    depth: usize,
    io: SeedIo<'_>,
) -> Option<Vec<String>> {
    if let Some(pkg) = crate::native_packages::lookup(path) {
        return Some(pkg.exports.iter().map(|n| (*n).to_string()).collect());
    }
    if crate::dynamic_packages::is_dynamic_package(path) {
        return Some(crate::dynamic_packages::export_names(path));
    }
    let abs = resolve_import_path(dir, path)?;
    let imported = parsed_module(&abs, io)?;
    let mut out: Vec<String> = imported
        .stmts
        .iter()
        .filter_map(|s| match &s.value {
            Stmt::Decl(d) => exported_name(&d.value).map(str::to_string),
            _ => None,
        })
        .collect();
    // The depth bound doubles as the cycle guard: a module that
    // (transitively) re-exports itself bottoms out here and reports "can't
    // enumerate" rather than recursing forever.
    let sub_dir = abs
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    for stmt in &imported.stmts {
        let Stmt::Decl(d) = &stmt.value else { continue };
        let Decl::Import {
            names,
            path,
            exported: true,
            ..
        } = &d.value
        else {
            continue;
        };
        if depth >= MAX_REEXPORT_DEPTH {
            return None;
        }
        out.extend(static_import_names(&sub_dir, path, names, depth + 1, io)?);
    }
    Some(out)
}

/// How many nested re-exports we will follow when gathering type
/// metadata. Deep enough for any sane module tree, bounded so a pathological
/// (or cyclic) layout can't hang the typechecker.
pub(crate) const MAX_REEXPORT_DEPTH: usize = 8;

pub(crate) fn collect_import_seed_inner(
    module: &Module,
    dir: &Path,
    visited: &mut HashSet<PathBuf>,
    depth: usize,
    io: SeedIo<'_>,
) -> saule_semantic::ModuleSeed {
    collect_seed(module, dir, visited, depth, io, false)
}

/// What `module`'s imports bring into scope, or with `only_exported` what its
/// `export from … import …` statements pass on to whoever imports it.
fn collect_seed(
    module: &Module,
    dir: &Path,
    visited: &mut HashSet<PathBuf>,
    depth: usize,
    io: SeedIo<'_>,
    only_exported: bool,
) -> saule_semantic::ModuleSeed {
    let mut seed = saule_semantic::ModuleSeed::default();
    // Native packages this module imports from, for the types-only pass at
    // the end.
    let mut packages: Vec<&str> = Vec::new();
    // Saule modules it imports from, for the same pass.
    let mut imported_modules: Vec<PathBuf> = Vec::new();

    for stmt in &module.stmts {
        let Stmt::Decl(d) = &stmt.value else { continue };
        let Decl::Import {
            names,
            path,
            exported,
            ..
        } = &d.value
        else {
            continue;
        };
        if only_exported && !exported {
            continue;
        }
        if crate::native_packages::lookup(path).is_some()
            || crate::dynamic_packages::is_dynamic_package(path)
        {
            packages.push(path);
        }

        // Native package seed — fold the package's synthetic
        // class/interface/enum metadata directly into the seed; no need
        // to parse anything from disk.
        if let Some(pkg) = crate::native_packages::lookup(path) {
            let built = (pkg.builtins)();
            let aliases = collect_native_aliases(pkg, names);
            for (orig, alias) in aliases {
                if let Some(info) = built.classes.get(&orig).cloned() {
                    seed.classes.entry(alias.clone()).or_insert(info);
                }
                if let Some(ext) = built.interfaces.get(&orig).cloned() {
                    seed.interfaces.entry(alias.clone()).or_insert(ext);
                }
                if let Some(info) = built.enums.get(&orig).cloned() {
                    seed.enums.entry(alias).or_insert(info);
                }
            }
            continue;
        }

        // Dynamic (shared-library) native package seed — synthesize a
        // semantic `ClassInfo` per exported class and an `EnumInfo` per enum,
        // so `Window.create(...)`, `img.width` and `BlendMode.Add` resolve.
        // Without this the loop below would try to read the synthetic
        // sentinel path as a file and silently skip the package, leaving
        // its types undefined.
        if crate::dynamic_packages::is_dynamic_package(path) {
            for (alias, info) in crate::dynamic_packages::seed_classes(path, names) {
                seed.classes.entry(alias).or_insert(info);
            }
            for (alias, info) in crate::dynamic_packages::seed_enums(path, names) {
                seed.enums.entry(alias).or_insert(info);
            }
            for (alias, extends) in crate::dynamic_packages::seed_interfaces(path, names) {
                seed.interfaces.entry(alias).or_insert(extends);
            }
            continue;
        }

        let Some(abs) = resolve_import_path(dir, path) else {
            continue;
        };
        let Some(imported) = parsed_module(&abs, io) else {
            continue;
        };
        imported_modules.push(abs.clone());

        let (mut reg, ifaces, enums, iface_methods, _iface_params) =
            saule_semantic::build_registry(&imported);
        let mut funcs = saule_semantic::build_function_registry(&imported);
        // An imported accessor's return type is inferred here, on the way
        // in. Doing it only in `analyze_inner` would infer it for the file
        // that declares the class and not for the files that use it, which
        // is the wrong way round: the declaring file can read the body.
        saule_semantic::infer_missing_returns(&imported, &mut reg, &ifaces, &enums, &mut funcs);
        let vars = saule_semantic::build_variable_registry(&imported);

        // For each top-level decl in the imported module, decide which
        // (local) alias to register it under. Wildcard imports adopt the
        // original name; named imports rename per `as`-clause.
        let aliases = collect_import_aliases(&imported, names);

        for (orig, alias) in aliases {
            if let Some(info) = reg.get(&orig).cloned() {
                seed.classes.entry(alias.clone()).or_insert(info);
            }
            if let Some(ext) = ifaces.get(&orig).cloned() {
                seed.interfaces.entry(alias.clone()).or_insert(ext);
            }
            // Carried beside `interfaces` so an *imported* interface's
            // method return types are as knowable as a local one's —
            // otherwise the fix would work only in single-file programs,
            // which is not where interfaces are used.
            if let Some(sigs) = iface_methods.get(&orig).cloned() {
                seed.interface_methods.entry(alias.clone()).or_insert(sigs);
            }
            if let Some(info) = enums.get(&orig).cloned() {
                seed.enums.entry(alias.clone()).or_insert(info);
            }
            if let Some(sig) = funcs.get(&orig).cloned() {
                seed.functions.entry(alias.clone()).or_insert(sig);
            }
            if let Some(ty) = vars.get(&orig).cloned() {
                seed.variables.entry(alias).or_insert(ty);
            }
        }

        // What the imported module passes on from its own imports lives one
        // level deeper. `visited` holds the modules on the current path, so a
        // cycle of re-exports stops instead of recursing.
        if depth < MAX_REEXPORT_DEPTH && visited.insert(abs.clone()) {
            let sub_dir = abs
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."));
            let sub = collect_seed(&imported, &sub_dir, visited, depth + 1, io, true);
            visited.remove(&abs);
            merge_reexported_seed(&mut seed, sub, names);
        }
    }

    if !only_exported {
        seed_package_types(&mut seed, &packages);
        seed_reachable_types(&mut seed, &imported_modules, io);
    }

    seed
}

/// The types one module declares itself — not those it imports. See
/// [`seed_reachable_types`].
#[derive(Default)]
struct DeclaredTypes {
    classes: saule_semantic::ClassRegistry,
    enums: saule_semantic::EnumRegistry,
    interfaces: saule_semantic::InterfaceRegistry,
    interface_methods: saule_semantic::InterfaceMethodRegistry,
    /// The modules and native packages it imports from, in the order
    /// written.
    imports: Vec<Reached>,
}

/// Where one `import` leads.
enum Reached {
    Module(PathBuf),
    Package(String),
}

/// A module's tree, and what it declares.
type DeclaredEntry = (Arc<Module>, Arc<DeclaredTypes>);

thread_local! {
    /// Each module's [`DeclaredTypes`], with the tree they were read from.
    /// Every file's seed walks the same modules, so without this a project
    /// of a hundred files built each module's registry a hundred times. An
    /// entry is used only while the module's tree is the very same one
    /// (`Arc::ptr_eq`): an edited module is parsed into a new tree, which
    /// misses and is read again.
    static DECLARED: std::cell::RefCell<HashMap<PathBuf, DeclaredEntry>> =
        std::cell::RefCell::new(HashMap::new());
}

/// Make the types a module's imports lead to known to the checker,
/// imported by name or not: the Saule counterpart of
/// [`seed_package_types`].
///
/// A class's fields and methods name the types of *its* module: `Outer`
/// has an `inner: Inner`, and `Inner` is imported in `Outer.sau`. A module
/// that imports only `Outer` used to know nothing about `Inner`, so
/// `o.inner.weight` had no type at all — and an untyped value goes
/// unchecked, so `1.0 * o.inner.weight` passed `saule check` and failed at
/// run time with "cannot mix an integer and a float". The fix the language
/// asked of the program was to import a name it never writes.
///
/// So the types the seed holds are followed: every class, enum or
/// interface one of them mentions — a field's type, a method's parameter
/// or result, a parent — is looked up in the modules the imports reach,
/// added, and followed in turn. Only what is mentioned is added, so a seed
/// grows by what its types use rather than by the whole program.
///
/// Only types, and only gaps: a name the module's imports bind keeps its
/// meaning, as does one it declares (decided where the seed meets the
/// module's own declarations), and its functions and variables are not
/// touched — so nothing becomes *callable* by a name the module didn't
/// import. A name several modules declare means the nearest: modules are
/// searched breadth first, imports in the order written, then the native
/// packages they use.
fn seed_reachable_types(
    seed: &mut saule_semantic::ModuleSeed,
    imported: &[PathBuf],
    io: SeedIo<'_>,
) {
    let (modules, packages) = reachable(imported, io);
    let mut from_packages = saule_semantic::ModuleSeed::default();
    let packages: Vec<&str> = packages.iter().map(String::as_str).collect();
    seed_package_types(&mut from_packages, &packages);

    let mut pending: Vec<String> = Vec::new();
    mentions_of_seed(seed, &mut pending);
    let mut tried: HashSet<String> = HashSet::new();

    while let Some(name) = pending.pop() {
        let known = seed.classes.contains_key(&name)
            || seed.enums.contains_key(&name)
            || seed.interfaces.contains_key(&name);
        if known || !tried.insert(name.clone()) {
            continue;
        }

        let sources = modules
            .iter()
            .map(|m| (&m.classes, &m.enums, &m.interfaces, &m.interface_methods))
            .chain(std::iter::once((
                &from_packages.classes,
                &from_packages.enums,
                &from_packages.interfaces,
                &from_packages.interface_methods,
            )));
        for (classes, enums, interfaces, interface_methods) in sources {
            if let Some(info) = classes.get(&name) {
                class_mentions(info, &mut pending);
                seed.classes.insert(name.clone(), info.clone());
                break;
            }
            if let Some(info) = enums.get(&name) {
                enum_mentions(info, &mut pending);
                seed.enums.insert(name.clone(), info.clone());
                break;
            }
            if let Some(extends) = interfaces.get(&name) {
                pending.extend(extends.iter().cloned());
                seed.interfaces.insert(name.clone(), extends.clone());
                if let Some(sigs) = interface_methods.get(&name) {
                    for sig in sigs.values() {
                        method_mentions(sig, &mut pending);
                    }
                    seed.interface_methods
                        .entry(name.clone())
                        .or_insert_with(|| sigs.clone());
                }
                break;
            }
        }
    }
}

/// Every module `imported` leads to, nearest first (breadth first, imports
/// in the order written), and the native packages they import from.
fn reachable(imported: &[PathBuf], io: SeedIo<'_>) -> (Vec<Arc<DeclaredTypes>>, Vec<String>) {
    let mut queue: std::collections::VecDeque<PathBuf> = imported.iter().cloned().collect();
    let mut seen: HashSet<PathBuf> = imported.iter().cloned().collect();
    let mut modules = Vec::new();
    let mut packages: Vec<String> = Vec::new();

    while let Some(abs) = queue.pop_front() {
        let Some(declared) = declared_types(&abs, io) else {
            continue;
        };
        for reached in &declared.imports {
            match reached {
                Reached::Module(next) => {
                    if seen.insert(next.clone()) {
                        queue.push_back(next.clone());
                    }
                }
                Reached::Package(name) => {
                    if !packages.contains(name) {
                        packages.push(name.clone());
                    }
                }
            }
        }
        modules.push(declared);
    }
    (modules, packages)
}

/// The type names everything in `seed` mentions.
fn mentions_of_seed(seed: &saule_semantic::ModuleSeed, out: &mut Vec<String>) {
    for info in seed.classes.values() {
        class_mentions(info, out);
    }
    for info in seed.enums.values() {
        enum_mentions(info, out);
    }
    for extends in seed.interfaces.values() {
        out.extend(extends.iter().cloned());
    }
    for sigs in seed.interface_methods.values() {
        for sig in sigs.values() {
            method_mentions(sig, out);
        }
    }
    for sig in seed.functions.values() {
        for param in &sig.params {
            type_mentions(&param.ty, out);
        }
        if let Some(ty) = &sig.return_ty {
            type_mentions(ty, out);
        }
    }
    for ty in seed.variables.values() {
        type_mentions(ty, out);
    }
}

fn class_mentions(info: &saule_semantic::ClassInfo, out: &mut Vec<String>) {
    out.extend(info.parent.iter().cloned());
    out.extend(info.implements.iter().cloned());
    for ty in info
        .parent_args
        .iter()
        .chain(info.implements_args.iter().flatten())
    {
        type_mentions(ty, out);
    }
    for ty in info.field_types.values() {
        type_mentions(ty, out);
    }
    for sig in info.methods.values() {
        method_mentions(sig, out);
    }
}

fn enum_mentions(info: &saule_semantic::EnumInfo, out: &mut Vec<String>) {
    for variant in info.variants.values() {
        for field in &variant.fields {
            type_mentions(&field.ty, out);
        }
    }
}

fn method_mentions(sig: &saule_semantic::MethodSig, out: &mut Vec<String>) {
    for param in &sig.params {
        type_mentions(&param.ty, out);
    }
    if let Some(ty) = &sig.return_ty {
        type_mentions(ty, out);
    }
}

fn type_mentions(ty: &saule_ast::Type, out: &mut Vec<String>) {
    use saule_ast::Type;
    match ty {
        Type::Named(name) => out.push(name.clone()),
        Type::Nullable(inner) => type_mentions(inner, out),
        Type::Table { key, value } => {
            if let Some(key) = key {
                type_mentions(key, out);
            }
            type_mentions(value, out);
        }
        Type::Tuple(items) => items.iter().for_each(|t| type_mentions(t, out)),
        Type::Function { params, ret } => {
            params.iter().for_each(|t| type_mentions(t, out));
            type_mentions(ret, out);
        }
        Type::Generic(g) => {
            out.push(g.name.clone());
            g.args.iter().for_each(|t| type_mentions(t, out));
        }
    }
}

/// What the module at `abs` declares, from [`DECLARED`] when its tree has
/// not changed.
fn declared_types(abs: &Path, io: SeedIo<'_>) -> Option<Arc<DeclaredTypes>> {
    let module = parsed_module(abs, io)?;
    let cached = DECLARED.with(|c| {
        c.borrow()
            .get(abs)
            .filter(|(tree, _)| Arc::ptr_eq(tree, &module))
            .map(|(_, declared)| declared.clone())
    });
    if let Some(declared) = cached {
        return Some(declared);
    }

    // With accessors' return types inferred, as they are for a direct
    // import.
    let (mut classes, interfaces, enums, interface_methods, _params) =
        saule_semantic::build_registry(&module);
    let mut funcs = saule_semantic::build_function_registry(&module);
    saule_semantic::infer_missing_returns(&module, &mut classes, &interfaces, &enums, &mut funcs);

    let dir = abs.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut imports = Vec::new();
    for stmt in &module.stmts {
        let Stmt::Decl(d) = &stmt.value else { continue };
        let Decl::Import { path, .. } = &d.value else {
            continue;
        };
        if crate::native_packages::lookup(path).is_some()
            || crate::dynamic_packages::is_dynamic_package(path)
        {
            imports.push(Reached::Package(path.clone()));
        } else if let Some(next) = resolve_import_path(&dir, path) {
            imports.push(Reached::Module(next));
        }
    }

    let declared = Arc::new(DeclaredTypes {
        classes,
        enums,
        interfaces,
        interface_methods,
        imports,
    });
    DECLARED.with(|c| {
        c.borrow_mut()
            .insert(abs.to_path_buf(), (module, declared.clone()))
    });
    Some(declared)
}

/// Make every class and enum of the native packages a module imports from
/// known to the checker, imported by name or not.
///
/// A package's functions return its own types: `Compositor.poll()` returns
/// `table<WindowEvent>`. The checker reads a value's members from the seed,
/// so with only `Compositor` imported, `ev.id` on such a value had no type
/// at all, and assigning it anywhere was an error the program couldn't fix
/// short of importing a name it never writes.
///
/// Runs after every import and only fills gaps: a name any import actually
/// binds still means that. A name the module declares itself wins too, but
/// that is decided where the seed meets the module's own declarations
/// (`saule_semantic::analyze_inner`): the seed is built from the module's
/// imports alone, and cached on them.
fn seed_package_types(seed: &mut saule_semantic::ModuleSeed, packages: &[&str]) {
    let all = ImportNames::All;
    let mut classes = Vec::new();
    let mut enums = Vec::new();
    let mut interfaces = Vec::new();
    for path in packages {
        if let Some(pkg) = crate::native_packages::lookup(path) {
            let built = (pkg.builtins)();
            classes.extend(built.classes);
            enums.extend(built.enums);
            interfaces.extend(built.interfaces);
        } else {
            classes.extend(crate::dynamic_packages::seed_classes(path, &all));
            enums.extend(crate::dynamic_packages::seed_enums(path, &all));
            interfaces.extend(crate::dynamic_packages::seed_interfaces(path, &all));
        }
    }
    let free = |seed: &saule_semantic::ModuleSeed, name: &str| {
        !seed.classes.contains_key(name)
            && !seed.enums.contains_key(name)
            && !seed.interfaces.contains_key(name)
    };
    for (name, info) in classes {
        if free(seed, &name) {
            seed.classes.insert(name, info);
        }
    }
    for (name, info) in enums {
        if free(seed, &name) {
            seed.enums.insert(name, info);
        }
    }
    for (name, extends) in interfaces {
        if free(seed, &name) {
            seed.interfaces.insert(name, extends);
        }
    }
}

/// The names `key`, exported by the imported module, is known by here: a
/// namespace import binds it both bare and qualified.
fn local_names(names: &ImportNames, key: &str) -> Vec<String> {
    match names {
        ImportNames::All => vec![key.to_string()],
        ImportNames::Namespace(ns) => vec![format!("{ns}.{key}"), key.to_string()],
        ImportNames::List(items) => items
            .iter()
            .filter(|(orig, _)| orig == key)
            .map(|(orig, alias)| alias.clone().unwrap_or_else(|| orig.clone()))
            .collect(),
    }
}

fn merge_into<V: Clone>(
    to: &mut std::collections::HashMap<String, V>,
    from: std::collections::HashMap<String, V>,
    names: &ImportNames,
) {
    for (k, v) in from {
        for local in local_names(names, &k) {
            to.entry(local).or_insert_with(|| v.clone());
        }
    }
}

/// Fold what an imported module re-exports into `seed`, under the names this
/// import gives them.
pub(crate) fn merge_reexported_seed(
    seed: &mut saule_semantic::ModuleSeed,
    sub: saule_semantic::ModuleSeed,
    names: &ImportNames,
) {
    merge_into(&mut seed.classes, sub.classes, names);
    merge_into(&mut seed.interfaces, sub.interfaces, names);
    // Beside `interfaces` for the same reason the direct-import path carries
    // them: an interface whose method signatures were dropped registers as a
    // type, but every call through it resolves to an unknown return.
    merge_into(&mut seed.interface_methods, sub.interface_methods, names);
    merge_into(&mut seed.enums, sub.enums, names);
    merge_into(&mut seed.functions, sub.functions, names);
    merge_into(&mut seed.variables, sub.variables, names);
}

/// Resolve which `(original_name, local_alias)` pairs come into the
/// importing module's scope from one `import` statement.
pub(crate) fn collect_import_aliases(
    imported: &Module,
    names: &ImportNames,
) -> Vec<(String, String)> {
    let exports: Vec<&str> = imported
        .stmts
        .iter()
        .filter_map(|s| match &s.value {
            Stmt::Decl(d) => exported_name(&d.value),
            _ => None,
        })
        .collect();
    names.bind(exports)
}

/// Native-package counterpart of [`collect_import_aliases`]: wildcards
/// pull in every name the package declares via
/// [`crate::native_packages::NativePackage::exports`].
pub(crate) fn collect_native_aliases(
    pkg: &crate::native_packages::NativePackage,
    names: &ImportNames,
) -> Vec<(String, String)> {
    names.bind(pkg.exports.iter().copied())
}
