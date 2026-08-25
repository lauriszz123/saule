//! Arithmetic, comparison, and the boolean operators.
//!
//! This is where the type table pays for itself: `a + b` picks `ADDI`,
//! `ADDF` or the dynamic `ARITHX` purely from what the typechecker proved,
//! and a *missing* type is always the dynamic case, never a wrong opcode.

use std::ops::Range;

use saule_ast::{BinOp, Expr, Spanned, UnaryOp};

use super::CompileError;
use super::super::ctx::{Compiler, Num};
use crate::op::{Instruction, Op};

impl Compiler<'_> {

    pub(crate) fn unary_to(
        &mut self,
        e: &Spanned<Expr>,
        op: UnaryOp,
        rhs: &Spanned<Expr>,
        dst: u16,
    ) -> Result<(), CompileError> {
        let span = &e.span;

        // An `Op*` overload on the operand's class, resolved here rather
        // than at run time — the same reason `binary_to` must do it (§8.7).
        // A bytecode method is a proto, not a `FunctionObject`, so the
        // runtime `ClassObject` the VM builds has an empty method map and
        // `ops::unary`'s overload lookup finds nothing: `-money` reported
        // "cannot negate a `Money`" where the tree-walker called `OpNeg`.
        if let Some(contract) = saule_ast::ops::unary_contract(op)
            && let Some(class) = self.class_of_expr(rhs)
            && let Some(&slot) = self.chunk.classes[class as usize]
                .vindex
                .get(contract.method)
        {
            let m = self.mark();
            // The receiver is the window's only register: an overload takes
            // no arguments beyond `self`.
            let base = self.alloc(span)?;
            self.expr_to(rhs, base)?;
            let a = self.reg8(base, span)?;
            self.emit(Instruction::abc(Op::CALLM, a, 1, slot as u8), span);
            self.move_result(base, dst, span)?;
            self.free_to(m);
            return Ok(());
        }

        // One operand, so nothing is evaluated between the read and the
        // instruction that consumes it — but the purity test still has to
        // run, because `UNARYX` below calls `ops::unary`, which can dispatch
        // an `Op*` overload. `operand_is_pure` declines anything that is not
        // a literal, `self`, a frame local, or arithmetic over those.
        let m = self.mark();
        let r = self.operand_to_reg(rhs, self.operand_is_pure(rhs))?;
        let (a, b) = (self.reg8(dst, span)?, self.reg8(r, span)?);

        let ins = match op {
            UnaryOp::Not => Instruction::abc(Op::NOT, a, b, 0),
            UnaryOp::Neg => match self.num_of_node(rhs) {
                Some(Num::Int) => Instruction::abc(Op::NEGI, a, b, 0),
                Some(Num::Float) => Instruction::abc(Op::NEGF, a, b, 0),
                None => {
                    // Dynamic negation: `-x` where nothing was proved.
                    // `ops::unary` reproduces the tree-walker exactly for
                    // every non-instance operand.
                    self.emit(Instruction::abc(Op::UNARYX, a, b, 0), span);
                    self.emit(
                        Instruction::ax_of(
                            Op::EXTRAARG,
                            crate::op::dynop::encode_unary(UnaryOp::Neg),
                        ),
                        span,
                    );
                    self.free_to(m);
                    return Ok(());
                }
            },
            UnaryOp::BNot => Instruction::abc(Op::BNOT, a, b, 0),
            UnaryOp::Len => Instruction::abc(Op::LEN, a, b, 0),
        };
        self.emit(ins, span);
        self.free_to(m);
        Ok(())
    }

    pub(crate) fn binary_to(
        &mut self,
        e: &Spanned<Expr>,
        op: BinOp,
        lhs: &Spanned<Expr>,
        rhs: &Spanned<Expr>,
        dst: u16,
    ) -> Result<(), CompileError> {
        let span = &e.span;

        // Both operands must agree on a numeric kind for a typed opcode.
        // Disagreement is not a compiler bug — `saule-typeck` rejects mixing
        // `integer` and `float` — so it can only mean one side went
        // uninferred, which is the dynamic case.
        let kind = match (self.num_of_node(lhs), self.num_of_node(rhs)) {
            (Some(l), Some(r)) if l == r => Some(l),
            _ => None,
        };

        // An `Op*` overload, resolved here rather than at run time (§8.7).
        //
        // It *must* be resolved here: a bytecode method is a proto, not a
        // `FunctionObject`, so the runtime `ClassObject` the VM builds has an
        // empty method map and `ops::binary`'s overload lookup would find
        // nothing. The dispatch-on-left-operand rule moves into the compiler,
        // where it costs nothing at run time.
        if let Some(contract) = saule_ast::ops::binary_contract(op)
            && let Some(class) = self.class_of_expr(lhs)
            && let Some(&slot) = self.chunk.classes[class as usize]
                .vindex
                .get(contract.method)
        {
            let m = self.mark();
            let base = self.alloc_n(2, span)?;
            self.expr_to(lhs, base)?;
            self.expr_to(rhs, base + 1)?;
            let a = self.reg8(base, span)?;
            self.emit(Instruction::abc(Op::CALLM, a, 2, slot as u8), span);

            // Two contracts do not return the operator's answer directly,
            // and `ops::binary` post-processes both. Doing the same here is
            // what keeps the engines identical: without it `b < a` yielded
            // `compare`'s raw `-180` instead of `true`.
            //
            // * `equals` returns a value read for truthiness, which `!=`
            //   then negates.
            // * `compare` returns a negative / zero / positive integer, and
            //   all four ordering operators read it against zero — which is
            //   why one method covers them.
            use BinOp::*;
            match op {
                Eq | NotEq => {
                    self.move_result(base, dst, span)?;
                    let d = self.reg8(dst, span)?;
                    // `NOT` is the only truthiness-to-`Bool` opcode, so two
                    // of them normalise the way `is_truthy()` does.
                    self.emit(Instruction::abc(Op::NOT, d, d, 0), span);
                    if op == Eq {
                        self.emit(Instruction::abc(Op::NOT, d, d, 0), span);
                    }
                }
                Lt | LtEq | Gt | GtEq => {
                    let z = self.alloc(span)?;
                    let zr = self.reg8(z, span)?;
                    self.emit(Instruction::asbx(Op::LOADI, zr, 0), span);
                    let d = self.reg8(dst, span)?;
                    // Operands swapped rather than a `GT` opcode, the same
                    // trick `binary_opcode` uses.
                    let (o, lo, hi) = match op {
                        Lt => (Op::LTI, a, zr),
                        LtEq => (Op::LEI, a, zr),
                        Gt => (Op::LTI, zr, a),
                        _ => (Op::LEI, zr, a),
                    };
                    self.emit(Instruction::abc(o, d, lo, hi), span);
                }
                _ => self.move_result(base, dst, span)?,
            }
            self.free_to(m);
            return Ok(());
        }

        // `x + 1` — a small integer literal folded into the instruction, so
        // the `LOADI` that materialised it into a register disappears.
        if let Some(()) = self.immediate_arith(op, kind, lhs, rhs, dst, span)? {
            return Ok(());
        }

        // Operands that are already in a register are used where they are.
        // Not for `..`: `CONCAT` is n-ary over a register *range*, so its
        // operands have to be adjacent temporaries and reusing a local's
        // register would break the range rather than shorten it.
        let in_place = op != BinOp::Concat
            && self.operand_is_pure(lhs)
            && self.operand_is_pure(rhs);

        let m = self.mark();
        let lr = self.operand_to_reg(lhs, in_place)?;
        let rr = self.operand_to_reg(rhs, in_place)?;
        let (a, b, c) = (
            self.reg8(dst, span)?,
            self.reg8(lr, span)?,
            self.reg8(rr, span)?,
        );

        let result = self.binary_opcode(op, kind, a, b, c, span);
        self.free_to(m);
        result
    }

    /// `R[dst] := R[operand] <op> imm` — the `ADDII` family.
    ///
    /// `ADDII` / `SUBII` / `MULII` take a **signed 8-bit immediate** in `C`,
    /// have been implemented in the dispatch loop since Phase 1, and — like
    /// `JLTI` before this slice — were **never emitted**. `loop_arith`'s
    /// `s = s + i * 2 - 1` spent two of its six instructions on `LOADI`s
    /// materialising `2` and `1` into registers that were read once.
    ///
    /// Integer only. There is no float immediate form, and `sext(C)` is an
    /// `i64`, so a literal outside `i8` keeps the register form rather than
    /// being truncated — which would be a silently wrong answer, the one
    /// outcome this project treats as worse than a slow one.
    ///
    /// `Add` and `Mul` commute, so a literal on *either* side folds; `Sub`
    /// does not (`ADDII` is `R[B] + sext(C)`, and `1 - x` is not
    /// `x - 1`), so only its right operand is eligible.
    fn immediate_arith(
        &mut self,
        op: BinOp,
        kind: Option<Num>,
        lhs: &Spanned<Expr>,
        rhs: &Spanned<Expr>,
        dst: u16,
        span: &Range<usize>,
    ) -> Result<Option<()>, CompileError> {
        if kind != Some(Num::Int) {
            return Ok(None);
        }
        let o = match op {
            BinOp::Add => Op::ADDII,
            BinOp::Sub => Op::SUBII,
            BinOp::Mul => Op::MULII,
            _ => return Ok(None),
        };
        // An `Op*` overload cannot apply — `kind` is `Int` on both sides, so
        // neither operand is an instance — which is what lets this run
        // before `binary_to`'s contract lookup rather than after it.
        let imm8 = |e: &Spanned<Expr>| match crate::compile::literal_value(&e.value) {
            Some(saule_interpreter::Value::Int(v)) => i8::try_from(v).ok(),
            _ => None,
        };
        let (value, imm) = match imm8(rhs) {
            Some(i) => (lhs, i),
            None if op != BinOp::Sub => match imm8(lhs) {
                Some(i) => (rhs, i),
                None => return Ok(None),
            },
            None => return Ok(None),
        };

        let m = self.mark();
        let r = self.operand_to_reg(value, self.operand_is_pure(value))?;
        let (a, b) = (self.reg8(dst, span)?, self.reg8(r, span)?);
        self.emit(Instruction::abc(o, a, b, imm as u8), span);
        self.free_to(m);
        Ok(Some(()))
    }

    fn binary_opcode(
        &mut self,
        op: BinOp,
        kind: Option<Num>,
        a: u8,
        b: u8,
        c: u8,
        span: &Range<usize>,
    ) -> Result<(), CompileError> {
        use BinOp::*;

        // Arithmetic: one opcode per (operator, numeric kind).
        let arith = match (op, kind) {
            (Add, Some(Num::Int)) => Some(Op::ADDI),
            (Sub, Some(Num::Int)) => Some(Op::SUBI),
            (Mul, Some(Num::Int)) => Some(Op::MULI),
            (Div, Some(Num::Int)) => Some(Op::DIVI),
            (Mod, Some(Num::Int)) => Some(Op::MODI),
            (Pow, Some(Num::Int)) => Some(Op::POWI),
            (Add, Some(Num::Float)) => Some(Op::ADDF),
            (Sub, Some(Num::Float)) => Some(Op::SUBF),
            (Mul, Some(Num::Float)) => Some(Op::MULF),
            (Div, Some(Num::Float)) => Some(Op::DIVF),
            (Mod, Some(Num::Float)) => Some(Op::MODF),
            (Pow, Some(Num::Float)) => Some(Op::POWF),
            _ => None,
        };
        if let Some(o) = arith {
            self.emit(Instruction::abc(o, a, b, c), span);
            return Ok(());
        }

        // Bitwise is integer-only by the language's rules, so the typed form
        // is the only form.
        let bitwise = match op {
            BAnd => Some(Op::BAND),
            BOr => Some(Op::BOR),
            BXor => Some(Op::BXOR),
            Shl => Some(Op::SHL),
            Shr => Some(Op::SHR),
            _ => None,
        };
        // An untyped operand falls through on purpose: `ops::bitwise` rejects
        // a non-integer at runtime with the message the tree-walker gives,
        // which is better than a compile-time refusal of a program that might
        // be fine.
        if let Some(o) = bitwise
            && kind == Some(Num::Int)
        {
            self.emit(Instruction::abc(o, a, b, c), span);
            return Ok(());
        }

        // Comparisons produce a boolean here. The *fused* branch forms
        // (§15.7) are used where the value feeds an `if`, which
        // `stmt::cond_jump` handles; this is the materialising case.
        match op {
            Lt | LtEq | Gt | GtEq if kind.is_some() => {
                let k = kind.expect("guarded by the arm");
                // `x > y` is `y < x`. Swapping operands is why there is no
                // `GT` opcode: the instruction set stays half the size and
                // the compiler does the work once.
                let (o, lo, hi) = match (op, k) {
                    (Lt, Num::Int) => (Op::LTI, b, c),
                    (LtEq, Num::Int) => (Op::LEI, b, c),
                    (Gt, Num::Int) => (Op::LTI, c, b),
                    (GtEq, Num::Int) => (Op::LEI, c, b),
                    (Lt, Num::Float) => (Op::LTF, b, c),
                    (LtEq, Num::Float) => (Op::LEF, b, c),
                    (Gt, Num::Float) => (Op::LTF, c, b),
                    (GtEq, Num::Float) => (Op::LEF, c, b),
                    _ => unreachable!("guarded by the outer match"),
                };
                self.emit(Instruction::abc(o, a, lo, hi), span);
                Ok(())
            }
            Eq | NotEq => {
                let o = match kind {
                    Some(Num::Int) => Op::EQI,
                    Some(Num::Float) => Op::EQF,
                    // `EQV` is the general form and is always correct — it
                    // is `values_equal`, including `Rc::ptr_eq` identity for
                    // reference types.
                    None => Op::EQV,
                };
                self.emit(Instruction::abc(o, a, b, c), span);
                if op == NotEq {
                    self.emit(Instruction::abc(Op::NOT, a, a, 0), span);
                }
                Ok(())
            }
            Concat => {
                // `CONCAT` is n-ary over a *register range*, which is what
                // makes `a .. b .. c` one allocation instead of two. Both
                // operands are already adjacent because they were compiled
                // into consecutive temporaries.
                debug_assert_eq!(c, b + 1, "CONCAT operands must be adjacent");
                self.emit(Instruction::abc(Op::CONCAT, a, b, c), span);
                Ok(())
            }
            And | Or | Coalesce => Err(CompileError::unsupported(
                "a short-circuiting operator",
                span.clone(),
            )),
            // Nothing was proved about the operands, so fall back to the
            // fully dynamic form rather than refusing. A missing type costs
            // a slower opcode, never a wrong answer (§15.6).
            other => {
                let Some(encoded) = crate::op::dynop::encode_binary(other) else {
                    return Err(CompileError::unsupported("this operator", span.clone()));
                };
                self.emit(Instruction::abc(Op::ARITHX, a, b, c), span);
                self.emit(Instruction::ax_of(Op::EXTRAARG, encoded), span);
                Ok(())
            }
        }
    }

    /// `a and b`, `a or b`, `a ?? b`.
    ///
    /// All three are "evaluate the left; decide whether the right is even
    /// needed", so all three share one shape: compute the left into the
    /// destination, test it, and either keep it or overwrite it with the
    /// right. The test opcode is the only difference.
    pub(crate) fn short_circuit_to(
        &mut self,
        op: BinOp,
        lhs: &Spanned<Expr>,
        rhs: &Spanned<Expr>,
        dst: u16,
    ) -> Result<(), CompileError> {
        let span = &lhs.span;
        self.expr_to(lhs, dst)?;
        let a = self.reg8(dst, span)?;

        // Each test *skips the following jump* in the case where the right
        // operand is needed, so the jump is taken exactly when the left
        // operand is the answer.
        let test = match op {
            // `and`: keep a falsy left. `C = 0` skips when truthy.
            BinOp::And => Instruction::abc(Op::TEST, a, 0, 0),
            // `or`: keep a truthy left. `C = 1` skips when falsy.
            BinOp::Or => Instruction::abc(Op::TEST, a, 0, 1),
            // `??`: keep a non-nil left. `JNIL` skips when nil.
            BinOp::Coalesce => Instruction::abc(Op::JNIL, a, 0, 0),
            _ => unreachable!("only the short-circuiting operators reach here"),
        };
        self.emit(test, span);
        let keep_left = self.emit_jump(Op::JMP, 0, span);
        self.expr_to(rhs, dst)?;
        self.patch_here(keep_left)?;
        Ok(())
    }
}
