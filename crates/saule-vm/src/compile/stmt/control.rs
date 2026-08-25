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
            // Jump past this arm when the condition is false. Plural: an
            // `and` chain tests each conjunct separately and every one of
            // them jumps here.
            let skips = self.cond_jumps_if_false(c)?;
            self.block(body)?;
            let last = i + 1 == n && !else_runs;
            if !last {
                to_end.push(self.emit_jump(Op::JMP, 0, &c.span));
            }
            for skip in skips {
                self.patch_here(skip)?;
            }
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
        let exits = self.cond_jumps_if_false(cond)?;
        self.loops.push(Default::default());
        self.block(body)?;
        let l = self.loops.pop().expect("pushed above");
        // `continue` re-tests the condition, so it lands where the back edge
        // goes.
        for c in l.continues {
            self.patch_to(c, top)?;
        }
        self.emit_jump_back(Op::JMP, 0, top, &cond.span)?;
        for exit in exits {
            self.patch_here(exit)?;
        }
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
        // `binary_to` makes before it picks a typed opcode. A mismatch is
        // no longer the end of the road: `JEQK` below compares a register
        // against a **constant** and needs nothing proved, which is what
        // `c == "{"` is made of.
        let kind = match (self.num_of_node(lhs), self.num_of_node(rhs)) {
            (Some(l), Some(r)) if l == r => Some(l),
            _ => None,
        };
        let Some(kind) = kind else {
            return self.constant_compare_jump(*op, lhs, rhs, &cond.span);
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

        // `n < 2` — the literal folded into the instruction, so the `LOADI`
        // that materialised it into a register read once disappears. Tried
        // before the register path for the same reason `immediate_arith` is:
        // it either applies or declines, and declining costs one `i8`
        // conversion.
        if let Some(label) = self.immediate_compare_jump(*op, kind, lhs, rhs, &cond.span)? {
            return Ok(Some(label));
        }

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

    /// [`fused_compare_jump`](Self::fused_compare_jump) against a **constant**
    /// — `JEQK`, for the `==` that no numeric kind covers.
    ///
    /// **`JEQK` has been in the instruction set since Phase 1, is checked by
    /// the verifier, has a verifier test of its own, and nothing has ever
    /// emitted one.** The same discovery `fused_compare_jump` itself was
    /// written for, one opcode over. `if c == "{"` — a string against a
    /// literal, which is what every scanner and every `elseif` chain in a
    /// parser is made of — compiled to `LOADK` + `EQV` + `TEST` + `JMP`,
    /// four words to ask one question.
    ///
    /// Measured first, per §16. `--profile-bytecode` on
    /// `benchmarks/sau/json.sau`, 110,843,923 instructions: `LOADK EQV`
    /// **6,234,024** (5.6%), `EQV TEST` **4,860,006** (4.4%), `TEST JMP`
    /// **4,680,003** (4.2%). That triple is about 14% of the program, and
    /// `json` is one of the two widest remaining gaps to Lua.
    ///
    /// **Substituting `JEQK` for `EQV` is exact, not merely close.** Both
    /// read `Value`'s own `PartialEq` — `EQV` between two registers, `JEQK`
    /// between a register and a constant — so this changes which
    /// instruction asks the question and not what the answer is.
    ///
    /// Three things make it decline:
    ///
    /// * **Anything but `==`.** `JEQK` skips *on equality* and has no `!=`
    ///   counterpart, so a `!=` would need a second jump to undo the skip:
    ///   three words where the materialising path takes four, for a saving
    ///   not worth a second opcode until a profile asks for one.
    /// * **An `equals` overload on the left operand's class.** Replicated
    ///   from `binary_to` rather than approximated, and tested against
    ///   `lhs` rather than against whichever side turned out to be the
    ///   constant, because that is the operand the dispatch rule names
    ///   (§8.7).
    ///
    ///   It **cannot fire today**, and that is worth knowing rather than
    ///   trusting: `saule-typeck` rejects `money == 2` outright
    ///   (`DisjointEquality`), so a class-typed operand never meets a
    ///   literal of another type, and an *optional* class type does not
    ///   resolve through `class_of_expr` — so `m == nil` dispatches no
    ///   overload here and none in `binary_to` either, the two agreeing
    ///   because they ask the same question. The guard stays because it is
    ///   the rule, not because a case reaches it;
    ///   `a_class_receiver_compared_to_a_constant_matches_the_materialising_form`
    ///   pins the reasoning so a `class_of_expr` that learns about
    ///   optionals has to come back here.
    /// * **A constant past the 8-bit `C` window.** The 257th constant in a
    ///   module keeps the materialising path.
    fn constant_compare_jump(
        &mut self,
        op: saule_ast::BinOp,
        lhs: &Spanned<Expr>,
        rhs: &Spanned<Expr>,
        span: &std::ops::Range<usize>,
    ) -> Result<Option<crate::compile::ctx::Label>, CompileError> {
        if op != saule_ast::BinOp::Eq {
            return Ok(None);
        }
        // The overload rule dispatches on the **left** operand, so this asks
        // about `lhs` whichever side the constant is on.
        if let Some(contract) = saule_ast::ops::binary_contract(op)
            && let Some(class) = self.class_of_expr(lhs)
            && self.chunk.classes[class as usize]
                .vindex
                .contains_key(contract.method)
        {
            return Ok(None);
        }
        // `==` commutes and `JEQK` reads `R[A] == K[C]`, so the constant
        // folds from either side with no mirrored opcode needed.
        let (value, k) = match crate::compile::literal_value(&rhs.value) {
            Some(v) => (lhs, v),
            None => match crate::compile::literal_value(&lhs.value) {
                Some(v) => (rhs, v),
                None => return Ok(None),
            },
        };
        let k = self.constant(k, span)?;
        let Ok(kc) = u8::try_from(k) else {
            return Ok(None);
        };

        let m = self.mark();
        let r = self.operand_to_reg(value, self.operand_is_pure(value))?;
        let a = self.reg8(r, span)?;
        // `B` is unused: one register operand and one constant.
        self.emit(Instruction::abc(Op::JEQK, a, 0, kc), span);
        let label = self.emit_jump(Op::JMP, 0, span);
        self.free_to(m);
        Ok(Some(label))
    }

    /// [`fused_compare_jump`](Self::fused_compare_jump) against a **signed
    /// 8-bit immediate** — the `JLTII` family (§15.7).
    ///
    /// `if n < 2` emitted `LOADI` then `JLTI`, one instruction and one
    /// register to hold a literal the next instruction reads and discards.
    /// `fib`'s guard is exactly that and it runs once per call.
    ///
    /// A literal on the **left** folds too, as the mirrored comparison
    /// against the right operand: `2 < n` is `n > 2`. That is why there is
    /// no second family of "immediate on the left" opcodes.
    ///
    /// An `Op*` overload cannot apply — the caller proved both sides
    /// `integer`, so neither is a class instance — which is what lets this
    /// skip the contract lookup `binary_to` has to make.
    ///
    /// Returns `None` when neither side is an `i8` literal, so the caller
    /// falls through to the register form. A literal too wide for `i8`
    /// declines rather than truncating: `sext(C)` is an `i64`, so a
    /// truncated operand would compare against a different number.
    fn immediate_compare_jump(
        &mut self,
        op: saule_ast::BinOp,
        kind: crate::compile::ctx::Num,
        lhs: &Spanned<Expr>,
        rhs: &Spanned<Expr>,
        span: &std::ops::Range<usize>,
    ) -> Result<Option<crate::compile::ctx::Label>, CompileError> {
        use crate::compile::ctx::Num;
        use saule_ast::BinOp::*;

        if kind != Num::Int {
            return Ok(None);
        }
        // Sees through a unary negation, which `literal_value` does not:
        // `-1` is `Unary { Neg, Int(1) }` in the AST, not `Int(-1)`, so
        // asking it directly would fold `n < 1` and decline `n < -1`. Only
        // an integer literal is unwrapped — `kind` already proved both sides
        // `integer`, so there is no `OpNeg` overload to run.
        let imm8 = |e: &Spanned<Expr>| {
            let v = match &e.value {
                Expr::Unary { op: saule_ast::UnaryOp::Neg, rhs } => {
                    match crate::compile::literal_value(&rhs.value) {
                        // `-i64::MIN` has no positive counterpart; it is far
                        // outside `i8` either way, so declining is the same
                        // answer as overflowing would have been.
                        Some(saule_interpreter::Value::Int(v)) => v.checked_neg()?,
                        _ => return None,
                    }
                }
                _ => match crate::compile::literal_value(&e.value) {
                    Some(saule_interpreter::Value::Int(v)) => v,
                    _ => return None,
                },
            };
            i8::try_from(v).ok()
        };
        // `R op K`.
        let direct = |op| {
            Some(match op {
                Lt => Op::JLTII,
                LtEq => Op::JLEII,
                Gt => Op::JGTII,
                GtEq => Op::JGEII,
                Eq => Op::JEQII,
                NotEq => Op::JNEII,
                _ => return None,
            })
        };
        // `K op R`, read as `R mirror(op) K`.
        let mirrored = |op| {
            Some(match op {
                Lt => Op::JGTII,
                LtEq => Op::JGEII,
                Gt => Op::JLTII,
                GtEq => Op::JLEII,
                Eq => Op::JEQII,
                NotEq => Op::JNEII,
                _ => return None,
            })
        };

        let (value, o, imm) = match (imm8(rhs), direct(op)) {
            (Some(i), Some(o)) => (lhs, o, i),
            _ => match (imm8(lhs), mirrored(op)) {
                (Some(i), Some(o)) => (rhs, o, i),
                _ => return Ok(None),
            },
        };

        let m = self.mark();
        let r = self.operand_to_reg(value, self.operand_is_pure(value))?;
        let a = self.reg8(r, span)?;
        // `B` is unused: one register operand and one immediate.
        self.emit(Instruction::abc(o, a, 0, imm as u8), span);
        let label = self.emit_jump(Op::JMP, 0, span);
        self.free_to(m);
        Ok(Some(label))
    }

    /// Every jump that leaves `cond` because it is **false**, in emission
    /// order, for the caller to patch at the false target.
    ///
    /// One label for an ordinary condition. An `and` chain gets one per
    /// conjunct, because in branch position `a and b` is not a value to
    /// compute — it is two tests that leave for the same place:
    ///
    /// ```text
    ///   if a and b then BODY end
    ///
    ///     <test a>   -> false        <test a and b>  -> false
    ///     <test b>   -> false        BODY
    ///     BODY                       false:
    ///     false:
    /// ```
    ///
    /// The right-hand column is what this replaces: `and` compiled as an
    /// expression materialises a `Value` through `TESTSET`, and the branch
    /// then tests *that*. Worse, a comparison inside it never reached
    /// [`fused_compare_jump`](Self::fused_compare_jump) at all, because that
    /// only ever saw the top-level expression — so `while p <= n and …`
    /// emitted the materialising `LEI` that slice 1 was written to remove.
    ///
    /// Measured before writing it, per §16: `--profile-bytecode` on
    /// `benchmarks/sau/json.sau` counts the `LEI TEST` pair **4,541,139**
    /// times, 4.1% of the program, and every one of them is a conjunct of an
    /// `and` — `while self.pos <= self.n and …`, `if b >= 48 and b <= 57`.
    ///
    /// Short-circuiting is preserved by construction rather than by care:
    /// the jump out of `a` is emitted *before* `b`'s code, so a false `a`
    /// leaves without evaluating `b`.
    ///
    /// **`or` is deliberately not split, and it is not symmetry for its own
    /// sake that stops it.** `a or b` needs a jump taken when `a` is
    /// **true**, and the fused comparisons only jump on the predicate they
    /// name. Inverting them at the `BinOp` level — `<` to `>=` — is sound
    /// for integers and **wrong for floats**: `!(a < b)` is true when either
    /// side is NaN and `a >= b` is false, so an `or` over float comparisons
    /// would silently take the wrong branch. That is a real path (`JLTF` is
    /// emitted), so the true-polarity form has to decline floats, and no
    /// profile has asked for any of it: `or` does not appear in a hot pair
    /// table in any benchmark. It keeps the materialising path, unchanged.
    fn cond_jumps_if_false(
        &mut self,
        cond: &Spanned<Expr>,
    ) -> Result<Vec<crate::compile::ctx::Label>, CompileError> {
        if let Expr::Binary { op: saule_ast::BinOp::And, lhs, rhs } = &cond.value {
            let mut out = self.cond_jumps_if_false(lhs)?;
            out.extend(self.cond_jumps_if_false(rhs)?);
            return Ok(out);
        }
        Ok(vec![self.cond_jump_if_false(cond)?])
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
