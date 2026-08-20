//! AST → [`Chunk`] compiler (`VM_DESIGN.md` §17).
//!
//! ```text
//! Module (AST)
//!   ├─ Pass 1: layout   — class layouts, vtables, itables, enum tags,
//!   │                     module slot assignment
//!   ├─ Pass 2: codegen  — recursive walk; register allocation is a stack
//!   │                     discipline (§18); consults the TypeTable for
//!   │                     opcode selection and the ResolveTable for names
//!   ├─ Pass 3: patch    — forward jump labels, match jump tables, lines
//!   └─ Pass 4: verify   — debug builds only (§17 Pass 4)
//! ```
//!
//! ## What lives where
//!
//! | file | pass | holds |
//! |---|---|---|
//! | [`layout`] | 1 | class layouts, vtables, itables, enum tags, module slots |
//! | [`class`] | 1-2 | compiling a class body against the layout it was given |
//! | [`ctx`] | 2 | the compiler's own state: registers, scopes, emission |
//! | [`expr`] | 2 | expression codegen |
//! | [`stmt`] | 2 | statement codegen |
//! | [`match_`] | 2 | `match`, its jump table, and pattern binding |
//! | [`verify`] | 4 | the debug-build check that a chunk is well formed |
//!
//! ## The `Unsupported` contract
//!
//! Anything the compiler cannot handle is a [`CompileError::Unsupported`],
//! which the CLI reads as "fall back to the tree-walker" rather than as a
//! hard failure. That is what made `--vm` usable long before it was
//! complete (§21.3), and it is still what keeps a gap from becoming a
//! crash: a construct nobody has taught the compiler yet runs under the
//! other engine and prints a note, rather than miscompiling.

pub mod class;
pub mod ctx;
pub mod expr;
pub mod layout;
pub mod match_;
pub mod stmt;
pub mod verify;

/// Register allocation is part of the compiler's state, so it lives in
/// [`ctx`]; this keeps the `compile::regalloc` path it had before.
pub use ctx::regalloc;

use std::ops::Range;

use saule_ast::Module;

use crate::chunk::{Chunk, Proto};
use crate::op::{Instruction, Op};


/// A construct the compiler does not handle yet, or a program it cannot
/// represent. Never a panic — §24.4 is explicit that even "function too
/// complex" must be a clean diagnostic.
#[derive(Debug, thiserror::Error, miette::Diagnostic)]
pub enum CompileError {
    /// The compiler has no codegen for this construct yet. The CLI treats
    /// this as "run it on the tree-walker instead".
    #[error("`{thing}` is not supported by the bytecode compiler yet")]
    #[diagnostic(help(
        "the bytecode compiler is still under construction; \
         the tree-walking interpreter runs this construct today"
    ))]
    Unsupported {
        thing: &'static str,
        #[label("not yet compiled")]
        span: Range<usize>,
    },

    /// A function body needed more than 256 registers (§5.2, §24.4).
    #[error("function `{name}` is too complex: it needs {needed} registers, the limit is 256")]
    #[diagnostic(help("split this function into smaller ones"))]
    TooManyRegisters {
        name: String,
        needed: usize,
        #[label("this function")]
        span: Range<usize>,
    },

    /// The verifier rejected a chunk this compiler produced.
    ///
    /// Always a compiler bug, never a user error — surfaced rather than
    /// swallowed because the alternative is a program that runs and computes
    /// the wrong answer.
    #[error("internal compiler error: {detail}")]
    #[diagnostic(help("this is a bug in the Saule bytecode compiler, not in your program"))]
    MalformedChunk {
        detail: String,
        #[label("while compiling this")]
        span: Range<usize>,
    },

    /// A jump displacement did not fit in `sBx` and no trampoline was
    /// emitted. Vanishingly rare; must not be a panic (§5.2).
    #[error("jump target is too far away ({distance} instructions)")]
    JumpTooFar {
        distance: i64,
        #[label("from here")]
        span: Range<usize>,
    },
}

impl CompileError {
    pub fn unsupported(thing: &'static str, span: Range<usize>) -> CompileError {
        CompileError::Unsupported { thing, span }
    }
}

/// The runtime value of a literal expression, for the places a chunk needs a
/// constant rather than code — enum variant values, and eventually constant
/// field templates.
pub(crate) fn literal_value(e: &saule_ast::Expr) -> Option<saule_interpreter::Value> {
    use saule_ast::Expr;
    use saule_interpreter::Value;
    Some(match e {
        Expr::Int(n) => Value::Int(*n),
        Expr::Float(f) => Value::Float(*f),
        Expr::Bool(b) => Value::Bool(*b),
        Expr::Str(s) => Value::Str(std::rc::Rc::new(s.clone())),
        Expr::Nil => Value::Nil,
        _ => return None,
    })
}

/// Compile a module that has already passed `saule_semantic::analyze` and
/// `saule_typeck::check`.
///
/// Runs both front-end passes again in their *publishing* form
/// (`analyze_with_bindings`, `check_with_types`) to get the two side tables
/// codegen reads. Their diagnostics are discarded here on purpose: this
/// function's precondition is that the module already checked clean, and
/// re-reporting would duplicate what the caller printed.
pub fn compile(module: &Module, name: &str, source: &str) -> Result<Chunk, CompileError> {
    let (_, bindings) =
        saule_semantic::analyze_with_bindings(module, saule_semantic::ModuleSeed::default());
    let (_, types) = saule_typeck::check_with_types(module);
    compile_with(module, name, source, &bindings, &types)
}

/// Collects the receiver names a module assigns through — the `X` of any
/// `X.field = value`.
///
/// Position is why this needs the visitor rather than `visit_exprs`: a
/// flattened expression walk cannot tell `Math.pi` being read from
/// `Math.pi` being written.
struct MutatedReceivers(std::collections::HashSet<String>);

impl saule_ast::Visitor for MutatedReceivers {
    fn assign_target(&mut self, e: &saule_ast::Spanned<saule_ast::Expr>) {
        if let saule_ast::Expr::Member { obj, .. } = &e.value
            && let saule_ast::Expr::Ident(n) = &obj.value
        {
            self.0.insert(n.clone());
        }
    }
}

/// Compile against side tables the caller already has, so a driver that ran
/// the front end does not pay for it twice.
pub fn compile_with(
    module: &Module,
    name: &str,
    source: &str,
    bindings: &saule_semantic::Bindings,
    types: &saule_typeck::TypeTable,
) -> Result<Chunk, CompileError> {
    let mut tables = Tables::default();
    let (mut chunk, _) = compile_into(
        module,
        name,
        source,
        bindings,
        types,
        &Default::default(),
        &mut tables,
        false,
        Vec::new(),
        0,
        Default::default(),
    )?;
    // A one-module program: the type world is this chunk's, so hand back
    // what `compile_into` took for the driver's benefit.
    *chunk.classes_mut() = tables.classes;
    *chunk.interfaces_mut() = tables.interfaces;
    *chunk.enums_mut() = tables.enums;
    Ok(chunk)
}

/// The program's shared type world, accumulated across modules.
///
/// Held by the driver rather than by any one chunk, and *moved* into the
/// chunk being compiled so the thousand `self.chunk.classes[…]` reads in
/// codegen keep working unchanged. Moving rather than sharing is what keeps
/// the refcount at one, so [`Chunk::classes_mut`]'s `Rc::get_mut` never
/// fails and no table is ever mutated while someone else can see it.
#[derive(Default)]
pub struct Tables {
    pub classes: Vec<crate::chunk::ClassProto>,
    pub interfaces: Vec<crate::chunk::InterfaceProto>,
    pub enums: Vec<crate::chunk::EnumProto>,
    /// Running total of module slots claimed so far — the next module's
    /// base in the program's flat slot space.
    pub module_slots: usize,
    /// Declared parameters of every class method compiled so far, for §19's
    /// call-site argument binding, keyed the way [`ctx::CalleeKey::Method`]
    /// is: by **program-global** `ClassIdx` and method name.
    ///
    /// This accumulates across modules, and it is the one part of
    /// `callee_params` that safely can. A `ClassIdx` means the same thing in
    /// every chunk of a program, so an entry written by the module that
    /// *declared* the class answers correctly for any module that imports
    /// it. `CalleeKey::Function`, by contrast, is keyed on a bare name and
    /// stays strictly per module: two modules may each declare `fn helper`,
    /// and letting one answer for the other is the same class of bug as the
    /// shadowing family in trap 1.
    ///
    /// Modules arrive in post-order — every module after the ones it imports
    /// — so a class's entry is always present before any importer needs it.
    pub method_params:
        std::collections::HashMap<(crate::chunk::ClassIdx, String), Vec<saule_ast::Param>>,
    /// Declared parameters of every top-level `fn` compiled so far, keyed by
    /// its **program-global module slot**.
    ///
    /// A slot, unlike a name, is unique across the program, which is what
    /// makes this safe to accumulate where a name-keyed map would not be.
    /// The call site cannot look a name up here directly — an importer holds
    /// its *own* slot for an imported name, not the exporter's — so the seed
    /// below goes through `ImportBinding`, which carries exactly that
    /// mapping. Only names a module actually imports are seeded, under the
    /// alias that module used.
    pub fn_params_by_slot: std::collections::HashMap<u16, Vec<saule_ast::Param>>,
}

/// Compile one module of a program, appending its types to `tables`.
///
/// `imported` is what this module's `import` statements bring into scope,
/// already resolved to program-global indices — see `program::compile`.
/// Returns the chunk and the module's own view of the type world, which the
/// driver reads to work out what this module *exports*.
pub(crate) fn compile_into(
    module: &Module,
    name: &str,
    source: &str,
    bindings: &saule_semantic::Bindings,
    types: &saule_typeck::TypeTable,
    imported: &layout::Layouts,
    tables: &mut Tables,
    // Whether a program driver bound this module's imports already; see
    // `Compiler::imports_bound`.
    imports_bound: bool,
    // Imported values to copy in before the body runs.
    import_bindings: Vec<ctx::ImportBinding>,
    // This module's position in its program.
    module_index: usize,
    // Names bound to a native package's exports, folded at compile time.
    native_imports: std::collections::HashMap<String, saule_interpreter::Value>,
) -> Result<(Chunk, layout::Layouts), CompileError> {
    let mut c = ctx::Compiler::new(name, source, bindings, types);
    c.imports_bound = imports_bound;
    c.import_bindings = import_bindings;
    c.native_imports = native_imports;
    c.module_slot_base = tables.module_slots;
    c.chunk.module_slot_base = tables.module_slots;
    c.chunk.module_index = module_index;
    let c_module_index = module_index;

    // Which receivers this module writes through, so the stdlib-constant
    // fold below knows what it must not freeze. One walk, before anything
    // is emitted, because a write can appear after the read it invalidates.
    let mut mutated = MutatedReceivers(Default::default());
    saule_ast::visit(module, &mut mutated);
    c.mutated_receivers = mutated.0;

    // Top-level `local`s, which become module slots rather than frame
    // locals — see `Compiler::shadowed_names`. Only the top level needs
    // collecting; a `local` in any inner block is a real frame local and
    // `FuncCtx::lookup` finds it.
    for s in &module.stmts {
        match &s.value {
            saule_ast::Stmt::Local { name, .. } => {
                c.shadowed_names.insert(name.clone());
            }
            saule_ast::Stmt::LocalMulti { names, .. } => {
                c.shadowed_names.extend(names.iter().map(|(n, _, _)| n.clone()));
            }
            saule_ast::Stmt::Decl(d) => {
                if let saule_ast::Decl::Variable { name, .. } = &d.value {
                    c.shadowed_names.insert(name.clone());
                }
            }
            _ => {}
        }
    }

    // Pass 1: class layouts. Runs before anything is compiled so a method
    // can reference a class declared further down the file, and so a
    // constructor call knows the slot its `init` occupies.
    // The class table arrives holding every module compiled before this one,
    // so the indices this pass assigns continue theirs — which is what makes
    // `ClassIdx` mean the same thing in every chunk of the program.
    let first_new_class = tables.classes.len();
    let mut layouts = layout::build(
        module,
        &mut tables.classes,
        &mut tables.interfaces,
        imported,
    )?;
    // Stamp this module on the classes it just declared. `vtable` and
    // `static_methods` hold proto indices, and those are per chunk — without
    // this a `CALLM` on a class from another module would load the running
    // module's proto of the same number.
    for c in &mut tables.classes[first_new_class..] {
        c.module = c_module_index;
    }
    // Moved in, not cloned: codegen reads the tables off the chunk, and the
    // driver takes them back at the end.
    *c.chunk.interfaces_mut() = std::mem::take(&mut tables.interfaces);
    *c.chunk.classes_mut() = std::mem::take(&mut tables.classes);
    *c.chunk.enums_mut() = std::mem::take(&mut tables.enums);
    // `build_enums` numbers from `chunk.enums.len()`, which is already the
    // program-global count — so enum indices become global for free.
    layout::build_enums(module, &mut c.chunk, &mut layouts)?;
    c.layouts = layouts.clone();
    c.check_interface_conformance(module)?;

    // Pass 1a: reserve a proto index for every top-level `fn` before any
    // body is compiled, so a forward call resolves — `fn a() b() end`
    // written above `fn b()` is ordinary Saule. The placeholder is replaced
    // when the real body is compiled; nothing can execute in between.
    for s in &module.stmts {
        if let saule_ast::Stmt::Decl(d) = &s.value
            && let saule_ast::Decl::Function { name, .. } = &d.value
        {
            let placeholder = Proto::new(Some(name), 0, 1, vec![Instruction::abc(Op::RET0, 0, 0, 0)]);
            let idx = c.chunk.add_proto(placeholder);
            c.fn_protos.insert(name.clone(), idx);
        }
    }

    // Pass 1b: every callee's declared parameters, for §19's call-site
    // argument binding. One pass before any body, because a method may call
    // another declared further down the file.
    //
    // Seeded first with the methods of every class compiled so far, so a
    // constructor or method call on an **imported** class can be bound by
    // name. Without this the class resolved fine — `layouts` has been
    // program-global since the imports slice — while its parameter list did
    // not exist, and `Panel(title: "…")` on an imported `Panel` refused with
    // `a named argument to a callee the compiler cannot identify`.
    for ((class, method), params) in &tables.method_params {
        c.callee_params.insert(
            ctx::CalleeKey::Method(*class, method.clone()),
            params.clone(),
        );
    }
    // Imported top-level `fn`s, by the slot they were exported from. Done
    // before the module's own declarations below, so a local `fn` of the same
    // name overwrites the import rather than the other way round — which is
    // the shadowing order the resolver uses.
    for ib in &c.import_bindings {
        if let Some(params) = tables.fn_params_by_slot.get(&ib.from)
            && let Some(local_name) = bindings.module_slots.get(ib.local as usize)
        {
            c.callee_params.insert(
                ctx::CalleeKey::Function(local_name.to_string()),
                params.clone(),
            );
        }
    }
    for s in &module.stmts {
        let saule_ast::Stmt::Decl(d) = &s.value else { continue };
        match &d.value {
            saule_ast::Decl::Function { name, params, .. } => {
                c.callee_params
                    .insert(ctx::CalleeKey::Function(name.clone()), params.clone());
                // Published for importers, keyed by the slot the program
                // gave it. `module_slot_base` is this module's offset into
                // the flat slot space, so this is the same number an
                // importer's `ImportBinding::from` will carry.
                if let Some(local) = c.module_slot_of(name)
                    && let Ok(global) = u16::try_from(c.module_slot_base + local as usize)
                {
                    tables.fn_params_by_slot.insert(global, params.clone());
                }
            }
            saule_ast::Decl::Class { name, members, .. } => {
                let Some(idx) = c.layouts.get(name) else { continue };
                for m in members {
                    if let saule_ast::ClassMember::Method(me) = &m.value {
                        c.callee_params.insert(
                            ctx::CalleeKey::Method(idx, me.name.clone()),
                            me.params.clone(),
                        );
                        // Published program-wide for the modules that import
                        // this class. Keyed on the global `ClassIdx`, so it
                        // cannot be answered by the wrong class.
                        tables
                            .method_params
                            .insert((idx, me.name.clone()), me.params.clone());
                    }
                }
            }
            _ => {}
        }
    }

    // Class bodies are compiled before the module body runs, so a
    // constructor called on line 1 finds a filled-in vtable.
    for s in &module.stmts {
        if let saule_ast::Stmt::Decl(d) = &s.value
            && matches!(&d.value, saule_ast::Decl::Class { .. })
        {
            c.class_decl(d)?;
        }
    }

    // Enum method bodies, for the same reason and at the same point: an
    // enum is a compile-time table too, and a method on it must exist before
    // the module body can call one.
    for s in &module.stmts {
        if let saule_ast::Stmt::Decl(d) = &s.value
            && matches!(&d.value, saule_ast::Decl::Enum { .. })
        {
            c.enum_decl(d)?;
        }
    }

    // Pass 2a: inherit the vtable slots a subclass did not override.
    //
    // Pass 1 copies the parent's vtable so the *slot numbering* extends it —
    // that is what makes a slot resolved against a static type correct for
    // any subclass. But at that point no body has been compiled, so what it
    // copies is a row of `u32::MAX` placeholders, and `class_decl` fills in
    // only the slots a class declares *itself*. An inherited, non-overridden
    // method was therefore left unfilled: `Circle.describe` resolved to a
    // slot holding `u32::MAX`.
    //
    // Resolved here rather than by making Pass 1 order-dependent on codegen:
    // one forward sweep, parents before children, which `order_by_depth`
    // already guarantees `chunk.classes` is in.
    // Only this module's classes: an earlier module's were swept when it was
    // compiled and are already filled. A child here whose parent is over
    // there still resolves — the sweep *reads* the whole table, it just does
    // not need to revisit rows another module already finished.
    for i in first_new_class..c.chunk.classes.len() {
        let Some(parent) = c.chunk.classes[i].parent else {
            continue;
        };
        debug_assert!(
            (parent as usize) < i,
            "layout must order parents before children for this sweep to be one pass"
        );
        for slot in 0..c.chunk.classes[i].vtable.len() {
            if c.chunk.classes[i].vtable[slot] != u32::MAX {
                continue;
            }
            if let Some(&inherited) = c.chunk.classes[parent as usize].vtable.get(slot) {
                c.chunk.classes_mut()[i].vtable[slot] = inherited;
            }
        }
    }

    // Import prologue: copy each imported *value* out of the exporting
    // module's slot and into this module's. Emitted before the body rather
    // than at each `import` statement because both slots are indices into
    // one flat vector, and post-order guarantees the exporting module has
    // already run — so there is nothing to sequence against.
    let prologue_span = 0..0;
    for b in std::mem::take(&mut c.import_bindings) {
        let m = c.mark();
        let r = c.alloc(&prologue_span)?;
        let a = c.reg8(r, &prologue_span)?;
        let dst = c.mod_slot(b.local, &prologue_span)?;
        c.emit(Instruction::abx(Op::GETMOD, a, b.from), &prologue_span);
        c.emit(Instruction::abx(Op::SETMOD, a, dst), &prologue_span);
        c.free_to(m);
    }

    // The module body's value is the last expression statement's — the same
    // rule `saule_interpreter::run_in` follows, which is what lets a
    // differential test compare the two engines by value.
    // Every distinct name the module declares at top level. The body is
    // walked below in the same order it will *run*, so the count and the
    // running set together say whether a call made here could still reach a
    // name that does not exist yet.
    for s in &module.stmts {
        if let saule_ast::Stmt::Decl(d) = &s.value {
            match &d.value {
                saule_ast::Decl::Function { name, .. }
                | saule_ast::Decl::Class { name, .. }
                | saule_ast::Decl::Interface { name, .. }
                | saule_ast::Decl::Enum { name, .. } => {
                    c.module_type_decls.insert(name.clone());
                }
                _ => {}
            }
        }
    }

    // One edge per *mention*: for each top-level declaration, the set of
    // names its body names. `reaches_undeclared` closes it transitively at
    // the call site, which is what catches a call whose callee reaches a
    // declaration further down the file (§ "Forward references").
    for s in &module.stmts {
        if let saule_ast::Stmt::Decl(d) = &s.value {
            let (name, body): (&String, Vec<saule_ast::Spanned<saule_ast::Stmt>>) = match &d.value {
                saule_ast::Decl::Function { name, body, .. } => (name, body.clone()),
                saule_ast::Decl::Class { name, members, .. } => {
                    let mut stmts = Vec::new();
                    for m in members {
                        if let saule_ast::ClassMember::Method(meth) = &m.value {
                            stmts.extend(meth.body.iter().cloned());
                        }
                    }
                    (name, stmts)
                }
                _ => continue,
            };
            let mut names = NameRefs::default();
            saule_ast::visit_stmts(&body, &mut names);
            c.module_refs
                .entry(name.clone())
                .or_default()
                .extend(names.0);
        }
    }

    let mut last = None;
    for s in &module.stmts {
        last = c.stmt(s)?.or(last);
        // *After* the statement, matching straight-line execution: a
        // declaration is not in scope until its own initialiser has run.
        for n in top_level_declared_names(s) {
            c.module_decls_seen.insert(n);
        }
    }

    let span = module.stmts.last().map(|s| s.span.clone()).unwrap_or(0..0);
    let mut chunk = c.finish(last, &span)?;

    // Pass 4. Debug builds only: this catches *compiler* bugs, and in a
    // release build of a chunk this compiler just produced there is nothing
    // new to learn. A chunk read back from a cache would be another matter —
    // that one is untrusted and must always be verified (§17).
    // Verified *before* the tables are taken back, since the verifier checks
    // class and enum indices against them.
    #[cfg(debug_assertions)]
    if let Err(e) = verify::verify(&chunk) {
        return Err(CompileError::MalformedChunk {
            detail: e.to_string(),
            span,
        });
    }

    // Hand the type world back to the driver for the next module. The chunk
    // is left with empty tables; the driver fills every chunk with the final
    // shared `Rc` once the last module is compiled.
    tables.classes = std::mem::take(chunk.classes_mut());
    tables.interfaces = std::mem::take(chunk.interfaces_mut());
    tables.enums = std::mem::take(chunk.enums_mut());
    tables.module_slots += chunk.module_slots;

    Ok((chunk, layouts))
}

/// The module-scope names one top-level statement declares.
///
/// Must recognise exactly the statements `saule_semantic`'s
/// `collect_module_scope` does — it is the same question asked
/// per-statement instead of per-module. A statement missed here would leave
/// the module body looking permanently under-declared, which costs a
/// fallback on every call it makes; one added here that the resolver does
/// not treat as a module slot would do the reverse and let a forward
/// reference through.
/// Every bare identifier a body mentions.
///
/// Deliberately crude: it does not distinguish a local named `later` from
/// the top-level `fn later`, so a body with a local of the same name is
/// counted as reaching it. The caller filters against
/// `module_type_decls`, and an over-count costs a fallback rather than a
/// wrong answer — which is the right side to err on for a guard whose whole
/// job is to refuse programs the tree-walker rejects.
#[derive(Default)]
struct NameRefs(std::collections::HashSet<String>);

impl saule_ast::Visitor for NameRefs {
    fn expr(&mut self, e: &saule_ast::Spanned<saule_ast::Expr>) {
        match &e.value {
            saule_ast::Expr::Ident(n) => {
                self.0.insert(n.clone());
            }
            // `C.go()` names `C`, which the flattened walk would otherwise
            // only see as the receiver expression it already visits — but a
            // pipe stage is a bare `String` with no expression node at all.
            saule_ast::Expr::Pipe { stages, .. } => {
                for st in stages {
                    self.0.insert(st.name.clone());
                }
            }
            _ => {}
        }
    }
}

fn top_level_declared_names(stmt: &saule_ast::Spanned<saule_ast::Stmt>) -> Vec<String> {
    use saule_ast::{Decl, ImportNames, Stmt};
    let mut out = Vec::new();
    match &stmt.value {
        Stmt::Local { name, .. } => out.push(name.clone()),
        Stmt::LocalMulti { names, .. } => {
            for (n, _, _) in names {
                out.push(n.clone());
            }
        }
        Stmt::Decl(d) => match &d.value {
            Decl::Function { name, .. }
            | Decl::Class { name, .. }
            | Decl::Interface { name, .. }
            | Decl::Enum { name, .. }
            | Decl::Variable { name, .. } => out.push(name.clone()),
            Decl::Import { names, .. } => match names {
                ImportNames::All => {}
                ImportNames::List(items) => {
                    for (orig, alias) in items {
                        out.push(alias.clone().unwrap_or_else(|| orig.clone()));
                    }
                }
            },
        },
        _ => {}
    }
    out
}
