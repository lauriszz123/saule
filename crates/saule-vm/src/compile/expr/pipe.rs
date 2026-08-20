//! The pipe operator.
//!
//! `x |> f |> g` is compiled as a chain of ordinary calls, each stage
//! feeding the next — the piped value becomes the first argument, and the
//! stage's own arguments follow it.

use std::ops::Range;

use saule_ast::{Expr, Spanned};

use super::CompileError;
use super::super::ctx::Compiler;
use crate::op::{Instruction, Op};

impl Compiler<'_> {

    /// A call leaves its single result in the first register of its window;
    /// move it where the caller wanted it.
    /// `when(source):a(x):b(y)` — the colon pipeline (§21.4 item 9).
    ///
    /// Lowered to a chain of ordinary calls, each threading the upstream
    /// value in as argument 0, which is exactly what `eval`'s `Expr::Pipe`
    /// arm does. The value lives in one register for the whole chain and
    /// every stage writes its result back there.
    pub(crate) fn pipe_to(
        &mut self,
        source: &Spanned<Expr>,
        stages: &[saule_ast::PipeStage],
        dst: u16,
        span: &Range<usize>,
    ) -> Result<(), CompileError> {
        let m = self.mark();
        let cur = self.alloc(span)?;
        self.expr_to(source, cur)?;
        for stage in stages {
            self.pipe_stage(cur, stage)?;
        }
        self.move_result(cur, dst, span)?;
        self.free_to(m);
        Ok(())
    }

    /// One `:name(args)` step. Reads `cur` and writes its result back there.
    ///
    /// The stage's callee is resolved **by name**, not through the binding
    /// table: a `PipeStage` holds a `String` and has no `NodeId`, so there is
    /// nothing for `Bindings` to have keyed an answer on — the same shape as
    /// the enum-method gap in §0.6. The lookup order below is therefore
    /// written out by hand, and it has to match the resolver's: a local
    /// shadows a top-level `fn`, which shadows a module slot, which shadows
    /// the prelude. Getting that order wrong is the `local String = {…}`
    /// bug this compiler has already shipped once.
    fn pipe_stage(
        &mut self,
        cur: u16,
        stage: &saule_ast::PipeStage,
    ) -> Result<(), CompileError> {
        let span = &stage.span;
        let name = stage.name.as_str();

        let mut positional = Vec::with_capacity(stage.args.len());
        for a in &stage.args {
            match a {
                saule_ast::CallArg::Positional(v) => positional.push(v),
                saule_ast::CallArg::Named { .. } => {
                    return Err(CompileError::unsupported(
                        "a named argument in a pipeline stage",
                        span.clone(),
                    ));
                }
            }
        }
        // The piped value plus whatever the stage wrote.
        let n_args = positional.len() as u16 + 1;

        // A top-level `fn` of this module: the target is known outright, and
        // `CALLK`'s window starts at the *arguments* — there is no callee
        // register, because the callee is the operand.
        if self.f.lookup(name).is_none()
            && self.not_shadowed(name)
            && let Some(&proto) = self.fn_protos.get(name)
        {
            // Same guard as the ordinary call path: a pipe into a `fn` the
            // module body has not reached yet is the same forward call.
            if !self.callk_resolvable(name) {
                return Err(CompileError::unsupported(
                    "a module-level call to a function declared further down",
                    span.clone(),
                ));
            }
            if self.reaches_undeclared(name) {
                return Err(CompileError::unsupported(
                    "a module-level call whose callee reaches a declaration further down",
                    span.clone(),
                ));
            }
            let m = self.mark();
            let base = self.alloc_n(n_args, span)?;
            self.move_result(cur, base, span)?;
            for (i, arg) in positional.iter().enumerate() {
                self.expr_to(arg, base + 1 + i as u16)?;
            }
            let a = self.reg8(base, span)?;
            let t = self.own_call_target(proto, span)?;
            self.emit(Instruction::abc(Op::CALLK, a, n_args as u8 + 1, 2), span);
            self.emit(Instruction::ax_of(Op::EXTRAARG, t), span);
            self.move_result(base, cur, span)?;
            self.free_to(m);
            return Ok(());
        }

        // A name an `import` bound to a native package's export, resolved to
        // a constant now so nothing is looked up at run time.
        //
        // The *prelude* is deliberately not consulted here: `saule-typeck`
        // rejects `when(x):tostring()` with `UnknownPipeStage`, so a stage
        // naming a built-in never reaches a valid program, and a branch for
        // it would be unreachable code pretending to be a feature.
        if self.f.lookup(name).is_none()
            && self.not_shadowed(name)
            && self.module_slot_of(name).is_none()
            && let Some(v) = self.native_imports.get(name).cloned()
        {
            let m = self.mark();
            let k = self.constant(v, span)?;
            // `CALLNAT` reads its arguments from `A+1`, mirroring `CALL`.
            let base = self.alloc_n(n_args + 1, span)?;
            self.move_result(cur, base + 1, span)?;
            for (i, arg) in positional.iter().enumerate() {
                self.expr_to(arg, base + 2 + i as u16)?;
            }
            let a = self.reg8(base, span)?;
            self.emit(Instruction::abc(Op::CALLNAT, a, n_args as u8 + 1, 2), span);
            self.emit(Instruction::ax_of(Op::EXTRAARG, k as u32), span);
            self.move_result(base, cur, span)?;
            self.free_to(m);
            return Ok(());
        }

        // Anything else callable is a *value*: a local holding a lambda, or
        // a module slot — which is also where a top-level `fn` from another
        // module ends up. `CALL` dispatches on what it finds.
        let m = self.mark();
        let base = self.alloc_n(n_args + 1, span)?;
        let a = self.reg8(base, span)?;
        if let Some(reg) = self.f.lookup(name) {
            self.move_result(reg, base, span)?;
        } else if let Some(slot) = self.module_slot_of(name) {
            let g = self.mod_slot(slot, span)?;
            self.emit(Instruction::abx(Op::GETMOD, a, g), span);
        } else {
            return Err(CompileError::unsupported(
                "a pipeline stage the compiler could not resolve",
                span.clone(),
            ));
        }
        self.move_result(cur, base + 1, span)?;
        for (i, arg) in positional.iter().enumerate() {
            self.expr_to(arg, base + 2 + i as u16)?;
        }
        self.emit(Instruction::abc(Op::CALL, a, n_args as u8 + 1, 2), span);
        self.move_result(base, cur, span)?;
        self.free_to(m);
        Ok(())
    }
}
