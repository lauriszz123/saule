//! `if`, `while`, `repeat`, and the branch helpers they share.
//!
//! A comparison feeding a branch is fused into one instruction rather than
//! materialising a boolean, which is what [`Compiler::fused_compare_jump`]
//! is for; [`Compiler::cond_jump_if_false`] is the general fallback.

use saule_ast::{Expr, Spanned, Stmt};

use super::super::CompileError;
use super::super::ctx::Compiler;
use crate::op::{Instruction, Op};

impl Compiler<'_> {

    /// `repeat … until cond`.
    ///
    /// Two things separate it from `while`. The body always runs once, and
    /// the condition is evaluated **inside the body's scope** — Lua-style,
    /// so `until` can read a local the body declared. That is why the scope
    /// is opened here rather than delegated to `block`.
    pub(crate) fn repeat_loop(
        &mut self,
        body: &[Spanned<Stmt>],
        cond: &Spanned<Expr>,
    ) -> Result<(), CompileError> {
        let top = self.f.label_here();
        self.loops.push(Default::default());
        self.f.enter_scope();
        for st in body {
            self.stmt(st)?;
        }

        // `continue` skips the rest of the body but still has to test the
        // condition, so it lands here.
        let test_at = self.f.label_here();
        let m = self.mark();
        let r = self.expr_tmp(cond)?;
        let a = self.reg8(r, &cond.span)?;
        // `C = 0` skips the next instruction when the condition is truthy —
        // and the next instruction is the back edge, so a true `until` exits.
        self.emit(Instruction::abc(Op::TEST, a, 0, 0), &cond.span);
        self.free_to(m);
        self.emit_jump_back(Op::JMP, 0, top, &cond.span)?;

        if let Some(reg) = self.f.leave_scope() {
            let a = self.reg8(reg, &cond.span)?;
            self.emit(Instruction::abc(Op::CLOSEUP, a, 0, 0), &cond.span);
        }
        let l = self.loops.pop().expect("pushed above");
        for c in l.continues {
            self.patch_to(c, test_at)?;
        }
        for b in l.breaks {
            self.patch_here(b)?;
        }
        Ok(())
    }


    pub(crate) fn if_chain(
        &mut self,
        cond: &Spanned<Expr>,
        then_block: &[Spanned<Stmt>],
        elseifs: &[(Spanned<Expr>, Vec<Spanned<Stmt>>)],
        else_block: Option<&[Spanned<Stmt>]>,
    ) -> Result<(), CompileError> {
        let mut to_end = Vec::new();

        let mut arms: Vec<(&Spanned<Expr>, &[Spanned<Stmt>])> = vec![(cond, then_block)];
        arms.extend(elseifs.iter().map(|(c, b)| (c, b.as_slice())));

        // "Only worth a jump to the end when something follows" is what this
        // loop's comment claimed, and it emitted one unconditionally — so a
        // plain `if c then … end` ended in a `JMP` to the very next
        // instruction. §17 lists dropping those, and `fib`'s listing showed
        // one on the line after the fused branch that this slice added.
        //
        // Decided here rather than by popping the instruction afterwards:
        // popping would have to reason about handler `pc` ranges and the
        // line table pointing into the code array, while *not emitting* has
        // nothing to undo. `patch_here` reads `code.len()` when it runs, so
        // the earlier arms' jumps land correctly without knowing this
        // happened.
        let n = arms.len();
        let else_runs = else_block.is_some_and(|b| !b.is_empty());
        for (i, (c, body)) in arms.into_iter().enumerate() {
            // Jump past this arm when the condition is false.
            let skip = self.cond_jump_if_false(c)?;
            self.block(body)?;
            let last = i + 1 == n && !else_runs;
            if !last {
                to_end.push(self.emit_jump(Op::JMP, 0, &c.span));
            }
            self.patch_here(skip)?;
        }

        if let Some(b) = else_block {
            self.block(b)?;
        }
        for l in to_end {
            self.patch_here(l)?;
        }
        Ok(())
    }

    pub(crate) fn while_loop(
        &mut self,
        cond: &Spanned<Expr>,
        body: &[Spanned<Stmt>],
    ) -> Result<(), CompileError> {
        let top = self.f.label_here();
        let exit = self.cond_jump_if_false(cond)?;
        self.loops.push(Default::default());
        self.block(body)?;
        let l = self.loops.pop().expect("pushed above");
        // `continue` re-tests the condition, so it lands where the back edge
        // goes.
        for c in l.continues {
            self.patch_to(c, top)?;
        }
        self.emit_jump_back(Op::JMP, 0, top, &cond.span)?;
        self.patch_here(exit)?;
        for b in l.breaks {
            self.patch_here(b)?;
        }
        Ok(())
    }

    /// A comparison feeding a branch, emitted as the **fused** form.
    ///
    /// `binary_opcode`'s comment has said "the fused branch forms (§15.7)
    /// are used where the value feeds an `if`, which `stmt::cond_jump`
    /// handles" since Phase 1. It did not. `JLTI` and its eleven siblings
    /// were implemented in the dispatch loop and **nothing ever emitted
    /// one**: every `if a < b` compiled to `LTI` + `TEST` + `JMP`, which
    /// materialises a `Value::Bool` into a register that is read once and
    /// discarded.
    ///
    /// Measured before writing it, per §16's rule: `--profile-bytecode` on
    /// `fib` shows `LTI TEST` as an adjacent pair 1,028,457 times — 8.7% of
    /// the program's instructions in the pair's first half alone. This
    /// removes that half outright.
    ///
    /// **Only when the operands are a proved numeric kind.** That is what
    /// rules out an `Op*` overload without re-deriving `binary_to`'s
    /// contract lookup: an `integer` or `float` operand cannot be a class
    /// instance, so there is no `compare` or `equals` method to dispatch to.
    /// An unproved `==` keeps the `EQV` + `TEST` path, which is correct for
    /// every value including overloaded ones.
    ///
    /// Returns `None` when the shape does not apply, so the caller falls
    /// through to the general path rather than this having to reproduce it.
    fn fused_compare_jump(
        &mut self,
        cond: &Spanned<Expr>,
    ) -> Result<Option<crate::compile::ctx::Label>, CompileError> {
        use saule_ast::BinOp::*;
        let Expr::Binary { op, lhs, rhs } = &cond.value else {
            return Ok(None);
        };
        if !matches!(op, Lt | LtEq | Gt | GtEq | Eq | NotEq) {
            return Ok(None);
        }
        // Both sides must agree on a numeric kind, the same test
        // `binary_to` makes before it picks a typed opcode.
        let kind = match (self.num_of_node(lhs), self.num_of_node(rhs)) {
            (Some(l), Some(r)) if l == r => l,
            _ => return Ok(None),
        };
        // Float `==` / `!=` have no fused form; `JEQ`/`JNE` are
        // `values_equal`, which is right but is not what `EQF` does for a
        // proved float pair. Leave those on the materialising path rather
        // than changing which predicate answers them.
        use crate::compile::ctx::Num;
        let o = match (op, kind) {
            (Lt, Num::Int) => Op::JLTI,
            (LtEq, Num::Int) => Op::JLEI,
            (Gt, Num::Int) => Op::JGTI,
            (GtEq, Num::Int) => Op::JGEI,
            (Eq, Num::Int) => Op::JEQI,
            (NotEq, Num::Int) => Op::JNEI,
            (Lt, Num::Float) => Op::JLTF,
            (LtEq, Num::Float) => Op::JLEF,
            (Gt, Num::Float) => Op::JGTF,
            (GtEq, Num::Float) => Op::JGEF,
            _ => return Ok(None),
        };

        let in_place = self.operand_is_pure(lhs) && self.operand_is_pure(rhs);
        let m = self.mark();
        let l = self.operand_to_reg(lhs, in_place)?;
        let r = self.operand_to_reg(rhs, in_place)?;
        let (la, rb) = (self.reg8(l, &cond.span)?, self.reg8(r, &cond.span)?);
        // Every `J*` skips the next instruction when the comparison holds,
        // and the next instruction is the jump to the false branch — the
        // same convention `TEST` follows, which is why no operand order
        // changes here.
        self.emit(Instruction::abc(o, la, rb, 0), &cond.span);
        let label = self.emit_jump(Op::JMP, 0, &cond.span);
        self.free_to(m);
        Ok(Some(label))
    }

    /// Emit a test of `cond` plus a jump taken when it is **false**.
    ///
    /// `TEST` skips the following instruction when truthiness matches, and
    /// by convention that following instruction is the jump — which is why
    /// the comparison opcodes carry no jump operand at all (§15.7).
    fn cond_jump_if_false(
        &mut self,
        cond: &Spanned<Expr>,
    ) -> Result<crate::compile::ctx::Label, CompileError> {
        if let Some(label) = self.fused_compare_jump(cond)? {
            return Ok(label);
        }
        let m = self.mark();
        let r = self.expr_tmp(cond)?;
        let a = self.reg8(r, &cond.span)?;
        // `TEST` skips the next instruction when truthiness *matches* `C`.
        // The next instruction is the jump-to-else, so `C = 0` means "skip
        // the jump when the condition is true" — fall through into the
        // then-branch. Getting this polarity backwards inverts every `if` in
        // the language and is invisible in the disassembly, which is why the
        // differential test caught it and reading the listing did not.
        self.emit(Instruction::abc(Op::TEST, a, 0, 0), &cond.span);
        let label = self.emit_jump(Op::JMP, 0, &cond.span);
        self.free_to(m);
        Ok(label)
    }

}
