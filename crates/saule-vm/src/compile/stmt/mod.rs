//! Statement codegen (`VM_DESIGN.md` §17 Pass 2, §11).
//!
//! ## What lives where
//!
//! | file | holds |
//! |---|---|
//! | `mod.rs` | the `block`/`stmt` dispatch every other file is reached from |
//! | [`decl`] | `fn`, `class`, `enum`, `export` — declarations |
//! | [`assign`] | `local`, assignment, parallel binding, compound assignment |
//! | [`control`] | `if`, `while`, `repeat`, and the jump helpers they share |
//! | [`loops`] | the `for` family, numeric and `for … in` |
//! | [`ret`] | `return` |
//! | [`try_catch`] | `try`/`catch`/`throw` and the type tests a `catch` needs |

pub mod assign;
pub mod control;
pub mod decl;
pub mod loops;
pub mod ret;
pub mod try_catch;

use saule_ast::{Expr, Spanned, Stmt};

use super::CompileError;
use super::ctx::Compiler;
use super::expr::results::Want;
use crate::op::{Instruction, Op};

/// What an assignment stores.
///
/// A parallel assignment evaluates its **whole** right-hand side before it
/// writes any target — that is what makes `a, b = b, a` a swap — so by the
/// time a target is written the value is already sitting in a register.
/// Every other assignment still hands over the expression, which is what
/// lets it be evaluated straight into the target's own register with no
/// temporary in between.
#[derive(Clone, Copy)]
pub(crate) enum Rhs<'a> {
    Expr(&'a Spanned<Expr>),
    Reg(u16),
}

impl Compiler<'_> {
    fn rhs_to(
        &mut self,
        rhs: Rhs<'_>,
        dst: u16,
        span: &std::ops::Range<usize>,
    ) -> Result<(), CompileError> {
        match rhs {
            Rhs::Expr(e) => self.expr_to(e, dst),
            Rhs::Reg(r) => self.move_result(r, dst, span),
        }
    }

    /// The register holding the value, allocating a temporary only when the
    /// right-hand side is still an expression.
    fn rhs_tmp(&mut self, rhs: Rhs<'_>) -> Result<u16, CompileError> {
        match rhs {
            Rhs::Expr(e) => self.expr_tmp(e),
            Rhs::Reg(r) => Ok(r),
        }
    }

    /// [`Self::rhs_tmp`], but reading a value already in a register where it
    /// is.
    ///
    /// `in_place` is the caller's promise that nothing runs between this
    /// read and the instruction that consumes it — the same contract
    /// [`Compiler::operand_to_reg`] takes, and the caller has usually
    /// already evaluated it to decide about the *receiver*.
    fn rhs_operand(&mut self, rhs: Rhs<'_>, in_place: bool) -> Result<u16, CompileError> {
        match rhs {
            Rhs::Expr(e) => self.operand_to_reg(e, in_place),
            Rhs::Reg(r) => Ok(r),
        }
    }

    /// Compile a block in its own scope.
    pub fn block(&mut self, stmts: &[Spanned<Stmt>]) -> Result<(), CompileError> {
        self.f.enter_scope();
        for s in stmts {
            match &s.value {
                // A call in statement position inside a block. **Nothing
                // reads its value** — only the module body and a
                // block-bodied `match` arm keep a statement's value, and
                // neither of them comes through here — so asking for one
                // result was two wasted instructions per call: `pop_frame`
                // copying a value down out of the callee, and the `MOVE`
                // `finish_call` emits to land it in a register the program
                // never looks at again. `p.move(1.0, 2.0)` in a loop was
                // one sixth `MOVE`.
                Stmt::Expr(e @ Spanned { value: Expr::Call { callee, args, .. }, .. }) => {
                    let m = self.mark();
                    let dst = self.alloc(&s.span)?;
                    self.call_to_want(e, callee, args, dst, Want::Fixed(0))?;
                    self.free_to(m);
                }
                _ => {
                    self.stmt(s)?;
                }
            }
        }
        // A block that something captured needs its registers closed before
        // they are reused — otherwise the next iteration of a loop would
        // overwrite the value a closure from the previous one points at
        // (§7.2). Closures are Phase 2's next slice; the hook is here so the
        // allocator and the emitter agree from the start.
        if let Some(reg) = self.f.leave_scope() {
            let a = self.reg8(reg, &stmts.last().map(|s| s.span.clone()).unwrap_or(0..0))?;
            self.emit(Instruction::abc(Op::CLOSEUP, a, 0, 0), &(0..0));
        }
        Ok(())
    }

    /// Compile one statement. Returns the register holding its value when
    /// the statement is a bare expression — the module body's result is the
    /// last of those, which is what `run_in` returns and therefore what a
    /// differential test compares.
    pub fn stmt(&mut self, s: &Spanned<Stmt>) -> Result<Option<u16>, CompileError> {
        let span = &s.span;
        match &s.value {
            Stmt::Local { name, value, ty, .. } => {
                self.local(name, value.as_ref(), ty.as_ref(), span)?;
                Ok(None)
            }

            Stmt::LocalMulti { names, values } => {
                self.local_multi(names, values, span)?;
                Ok(None)
            }

            Stmt::Assign { target, value } => {
                self.assign(target, Rhs::Expr(value))?;
                Ok(None)
            }

            Stmt::AssignMulti { targets, values } => {
                self.assign_multi(targets, values, span)?;
                Ok(None)
            }

            Stmt::Expr(e) => {
                // Kept live rather than released: the module body's value is
                // the last expression statement's.
                let r = self.expr_tmp(e)?;
                Ok(Some(r))
            }

            Stmt::If {
                cond,
                then_block,
                elseifs,
                else_block,
            } => {
                self.if_chain(cond, then_block, elseifs, else_block.as_deref())?;
                Ok(None)
            }

            Stmt::While { cond, body } => {
                self.while_loop(cond, body)?;
                Ok(None)
            }

            Stmt::Repeat { body, cond } => {
                self.repeat_loop(body, cond)?;
                Ok(None)
            }

            Stmt::CompoundAssign { target, op, value } => {
                self.compound_assign(target, *op, value, span)?;
                Ok(None)
            }

            Stmt::ForNumeric {
                var,
                from,
                to,
                step,
                body,
                ..
            } => {
                self.for_numeric(var, from, to, step.as_ref(), body, span)?;
                Ok(None)
            }

            Stmt::ForIn { vars, iter, body } => {
                self.for_in(vars, iter, body, span)?;
                Ok(None)
            }

            Stmt::Try {
                body,
                catch_var,
                catch_ty,
                catch_body,
            } => {
                self.try_catch(body, catch_var, catch_ty, catch_body, span)?;
                Ok(None)
            }

            Stmt::Throw(e) => {
                let m = self.mark();
                let r = self.expr_tmp(e)?;
                let a = self.reg8(r, span)?;
                self.emit(Instruction::abc(Op::THROW, a, 0, 0), span);
                self.free_to(m);
                Ok(None)
            }

            Stmt::Return(values) => {
                self.ret(values, span)?;
                Ok(None)
            }

            Stmt::Break | Stmt::Continue => {
                if self.loops.is_empty() {
                    // `saule-semantic`'s control-flow pass already rejects
                    // this, so reaching it means analysis was skipped.
                    return Err(CompileError::unsupported(
                        "`break` or `continue` outside a loop",
                        span.clone(),
                    ));
                }
                let is_break = matches!(s.value, Stmt::Break);
                let label = self.emit_jump(Op::JMP, 0, span);
                let l = self.loops.last_mut().expect("checked");
                if is_break {
                    l.breaks.push(label);
                } else {
                    l.continues.push(label);
                }
                Ok(None)
            }

            Stmt::Decl(d) => {
                self.decl(d)?;
                Ok(None)
            }

            other => Err(CompileError::unsupported(stmt_label(other), span.clone())),
        }
    }


    /// Whether a declaration here is a module slot rather than a register:
    /// the module body, outside any block.
    fn at_module_top(&self) -> bool {
        self.f.name.as_deref() == Some("main") && self.f.regs.block_depth() == 0
    }

}

fn stmt_label(s: &Stmt) -> &'static str {
    match s {
        Stmt::Error => "an unparsable statement",
        _ => "this statement",
    }
}
