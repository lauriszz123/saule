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
//! **Status: not implemented — but no longer blocked.** Phase 0 has landed,
//! so the two inputs codegen needs now exist: `saule_typeck::check_with_types`
//! publishes a `TypeTable` (which is what lets the compiler select `ADDI`
//! over `ARITHX`), and `saule_semantic::analyze_with_bindings` publishes a
//! `Bindings` with a slot for every local and an exact upvalue list for every
//! closure (which is what lets a name become a register index). Writing
//! codegen before those existed would have meant rewriting it afterwards,
//! which is the whole argument of §24.6.
//!
//! Passes 1–4 are Phase 2 and 3 work.
//!
//! Until then this module exists to fix the *interface*: everything the
//! compiler cannot handle is a [`CompileError::Unsupported`], which the CLI
//! reads as "fall back to the tree-walker" rather than as a hard failure.
//! That is what makes `--vm` usable long before it is complete (§21.3).

pub mod class;
pub mod ctx;
pub mod expr;
pub mod layout;
pub mod match_;
pub mod regalloc;
pub mod stmt;
pub mod verify;

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

/// Compile against side tables the caller already has, so a driver that ran
/// the front end does not pay for it twice.
pub fn compile_with(
    module: &Module,
    name: &str,
    source: &str,
    bindings: &saule_semantic::Bindings,
    types: &saule_typeck::TypeTable,
) -> Result<Chunk, CompileError> {
    let mut c = ctx::Compiler::new(name, source, bindings, types);

    // Pass 1: class layouts. Runs before anything is compiled so a method
    // can reference a class declared further down the file, and so a
    // constructor call knows the slot its `init` occupies.
    let (classes, mut layouts) = layout::build(module)?;
    c.chunk.interfaces = layout::build_interfaces(module).0;
    c.chunk.classes = classes;
    layout::build_enums(module, &mut c.chunk, &mut layouts)?;
    c.layouts = layouts;
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

    // Class bodies are compiled before the module body runs, so a
    // constructor called on line 1 finds a filled-in vtable.
    for s in &module.stmts {
        if let saule_ast::Stmt::Decl(d) = &s.value
            && matches!(&d.value, saule_ast::Decl::Class { .. })
        {
            c.class_decl(d)?;
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
    for i in 0..c.chunk.classes.len() {
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
                c.chunk.classes[i].vtable[slot] = inherited;
            }
        }
    }

    // The module body's value is the last expression statement's — the same
    // rule `saule_interpreter::run_in` follows, which is what lets a
    // differential test compare the two engines by value.
    let mut last = None;
    for s in &module.stmts {
        last = c.stmt(s)?.or(last);
    }

    let span = module.stmts.last().map(|s| s.span.clone()).unwrap_or(0..0);
    let chunk = c.finish(last, &span)?;

    // Pass 4. Debug builds only: this catches *compiler* bugs, and in a
    // release build of a chunk this compiler just produced there is nothing
    // new to learn. A chunk read back from a cache would be another matter —
    // that one is untrusted and must always be verified (§17).
    #[cfg(debug_assertions)]
    if let Err(e) = verify::verify(&chunk) {
        return Err(CompileError::MalformedChunk {
            detail: e.to_string(),
            span,
        });
    }

    Ok(chunk)
}
