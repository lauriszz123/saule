//! Expression codegen (`VM_DESIGN.md` §17 Pass 2).
//!
//! Every expression compiles *into a destination register*. That shape —
//! rather than "returns a register" — is what lets a local's initializer be
//! evaluated directly into the local's own slot, and an argument directly
//! into the callee's future frame (§6.2), instead of being computed
//! somewhere and moved.
//!
//! ## Opcode selection
//!
//! This is where Phase 0.5 pays for itself. `a + b` becomes `ADDI` when the
//! typechecker proved both operands are `integer`, `ADDF` when both are
//! `float`, and otherwise falls back — today to a `CompileError`, in Phase 3
//! to `ARITHX`, which reproduces `ops::binary` exactly.
//!
//! **A missing type is never a wrong opcode.** `ty_name` returning `None`
//! means "not proved", and every path treats that as the dynamic case. That
//! is what makes it safe for the type table to be incomplete (§21.1 0.5).

use std::ops::Range;

use saule_ast::{BinOp, Expr, Spanned, UnaryOp};
use saule_interpreter::Value;
use saule_semantic::Binding;

use super::CompileError;
use super::ctx::{Compiler, Num, num_of};
use crate::op::{Instruction, Op};

/// How many values a call site wants back (§6.2's `C` operand).
///
/// Saule is multi-return, but almost every call site consumes exactly one
/// value, which is why `Fixed(1)` is the default everywhere and the other
/// two shapes are reached only from a parallel `local`/assignment and from
/// `return f()`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Want {
    /// Exactly `n`. A callee returning fewer is padded with nil and a
    /// callee returning more has the surplus dropped — which is what
    /// `pop_frame` does with a non-zero `C`, and exactly the tree-walker's
    /// "extras → nil, surplus dropped".
    Fixed(u8),
    /// Every value the callee produced. The count is a **run-time** fact, so
    /// the results stay in the call window and the frame's `top` marks their
    /// end; only `RET A 0` can read that, which is why `return f()` is the
    /// sole caller.
    All,
}

impl Want {
    /// The `C` operand: `nret + 1`, with 0 meaning "all results, set `top`".
    fn c(self) -> u8 {
        match self {
            Want::Fixed(n) => n + 1,
            Want::All => 0,
        }
    }

    /// Registers the call window must reserve for the results it will
    /// receive. `All` reserves one; anything beyond that lands above the
    /// frame's high-water mark, which is safe because the window is the
    /// top of the register file and `RET` consumes it immediately.
    fn slots(self) -> u16 {
        match self {
            Want::Fixed(n) => n as u16,
            Want::All => 1,
        }
    }
}

/// Where a call left its results.
pub(crate) struct Results {
    /// First register of the run.
    pub base: u16,
    /// How many registers from `base` hold results, or `None` when only the
    /// VM knows — the run then ends at the frame's `top`.
    pub count: Option<u8>,
}

impl Compiler<'_> {
    /// Compile `e`, leaving its value in register `dst`.
    pub fn expr_to(&mut self, e: &Spanned<Expr>, dst: u16) -> Result<(), CompileError> {
        let span = &e.span;
        let a = self.reg8(dst, span)?;

        match &e.value {
            Expr::Nil => self.emit(Instruction::abc(Op::LOADNIL, a, 0, 0), span),
            Expr::Bool(b) => self.emit(Instruction::abc(Op::LOADBOOL, a, *b as u8, 0), span),

            // Small integers ride in the instruction word; anything larger
            // goes through the constant pool. `try_asbx` is the seam: it
            // answers "does this fit?" without panicking, which is exactly
            // the check whose absence silently truncated a literal in
            // Phase 1.
            Expr::Int(n) => {
                match i32::try_from(*n).ok().and_then(|small| {
                    Instruction::try_asbx(Op::LOADI, a, small)
                }) {
                    Some(ins) => self.emit(ins, span),
                    None => {
                        let k = self.constant(Value::Int(*n), span)?;
                        self.emit(Instruction::abx(Op::LOADK, a, k), span);
                    }
                }
            }
            Expr::Float(f) => {
                let k = self.constant(Value::Float(*f), span)?;
                self.emit(Instruction::abx(Op::LOADK, a, k), span);
            }
            Expr::Str(s) => {
                let k = self.constant(Value::Str(std::rc::Rc::new(s.clone())), span)?;
                self.emit(Instruction::abx(Op::LOADK, a, k), span);
            }

            Expr::Ident(name) => self.ident_to(e, name, dst)?,

            Expr::Unary { op, rhs } => self.unary_to(e, *op, rhs, dst)?,

            // Short-circuiting operators are control flow, not arithmetic:
            // the right operand must not be evaluated at all when the left
            // one decides the answer. They are handled before `binary_to`
            // for that reason.
            Expr::Binary {
                op: op @ (BinOp::And | BinOp::Or | BinOp::Coalesce),
                lhs,
                rhs,
            } => self.short_circuit_to(*op, lhs, rhs, dst)?,

            Expr::Binary { op, lhs, rhs } => self.binary_to(e, *op, lhs, rhs, dst)?,

            Expr::Call { callee, args } => self.call_to(e, callee, args, dst)?,

            Expr::Self_ => {
                if !self.f.in_method {
                    return Err(CompileError::unsupported("`self` outside a method", span.clone()));
                }
                // `self` is parameter 0, so it is already in register 0.
                if a != 0 {
                    self.emit(Instruction::abc(Op::MOVE, a, 0, 0), span);
                }
            }

            Expr::Member { obj, name } => self.member_to(e, obj, name, dst)?,

            Expr::SafeMember { obj, name } => self.safe_member_to(e, obj, name, dst)?,

            // `x!` — the value, or `ForceUnwrapNil` at this span.
            Expr::ForceUnwrap(inner) => {
                let m = self.mark();
                let r = self.expr_tmp(inner)?;
                let (a, b) = (self.reg8(dst, span)?, self.reg8(r, span)?);
                self.emit(Instruction::abc(Op::UNWRAPNIL, a, b, 0), span);
                self.free_to(m);
            }

            // `x as T`. The type travels as an index into the chunk's cast
            // table rather than as a `TypeDesc`, because the test is deep —
            // see `Chunk::cast_types`.
            Expr::Cast { value, ty } => {
                let k = self.chunk.add_cast_type(ty);
                let Ok(k) = u8::try_from(k) else {
                    return Err(CompileError::unsupported(
                        "a module casting to more than 256 distinct types",
                        span.clone(),
                    ));
                };
                let m = self.mark();
                let r = self.expr_tmp(value)?;
                let (a, b) = (self.reg8(dst, span)?, self.reg8(r, span)?);
                self.emit(Instruction::abc(Op::CASTCHK, a, b, k), span);
                self.free_to(m);
            }

            Expr::Match { scrutinee, arms } => self.match_to(e, scrutinee, arms, dst)?,

            Expr::Lambda { params, body, .. } => self.lambda_to(e, params, body, dst)?,

            Expr::Table(entries) => self.table_to(e, entries, dst)?,

            Expr::Index { obj, index } => self.index_to(e, obj, index, dst)?,

            Expr::Pipe { source, stages } => self.pipe_to(source, stages, dst, span)?,

            other => {
                return Err(CompileError::unsupported(expr_label(other), span.clone()));
            }
        }
        Ok(())
    }

    /// Compile `e` into a freshly allocated register.
    ///
    /// The caller owns the register and must release it with a mark; see
    /// [`Compiler::mark`].
    pub fn expr_tmp(&mut self, e: &Spanned<Expr>) -> Result<u16, CompileError> {
        let r = self.alloc(&e.span)?;
        self.expr_to(e, r)?;
        Ok(r)
    }

    /// Compile `e` as the value **list** it produces, not the single value
    /// `expr_to` reduces it to.
    ///
    /// Only a call can yield more than one value: `eval_values` expands
    /// `Expr::Call` and returns a one-element list for everything else, so
    /// matching that here is reproducing the oracle rather than approximating
    /// it. `dst` must own `want.slots()` consecutive registers.
    pub(crate) fn expr_results(
        &mut self,
        e: &Spanned<Expr>,
        dst: u16,
        want: Want,
    ) -> Result<Results, CompileError> {
        match &e.value {
            Expr::Call { callee, args } => {
                let m = self.mark();
                let r = self.call_to_want(e, callee, args, dst, want)?;
                // `Want::All` leaves the window allocated on purpose, so
                // only a counted result run can be released here — the `All`
                // caller frees once its `RET` has read the run.
                if r.count.is_some() {
                    self.free_to(m);
                }
                Ok(r)
            }
            _ => {
                self.expr_to(e, dst)?;
                self.one_result(dst, want, &e.span)
            }
        }
    }

    fn ident_to(
        &mut self,
        e: &Spanned<Expr>,
        name: &str,
        dst: u16,
    ) -> Result<(), CompileError> {
        let span = &e.span;
        let a = self.reg8(dst, span)?;
        // The resolver already decided what this name is; the compiler only
        // decides which register holds it (see `ctx`'s module docs).
        match self.binding(e.id) {
            Some(Binding::Local { .. }) => {
                let src = self.f.lookup(name).ok_or_else(|| CompileError::Unsupported {
                    thing: "a local the compiler has not seen declared",
                    span: span.clone(),
                })?;
                let b = self.reg8(src, span)?;
                if a != b {
                    self.emit(Instruction::abc(Op::MOVE, a, b, 0), span);
                }
                Ok(())
            }
            Some(Binding::Module { slot }) => {
                let slot = *slot;
                // A top-level `local` inside a block is an ordinary local of
                // the module body, and the resolver says so — but a name it
                // classified as a module slot may still be one this function
                // holds in a register, when we *are* the module body.
                match self.f.lookup(name) {
                    Some(src) => {
                        let b = self.reg8(src, span)?;
                        if a != b {
                            self.emit(Instruction::abc(Op::MOVE, a, b, 0), span);
                        }
                    }
                    None => {
                        let g = self.mod_slot(slot, span)?;
                        self.emit(Instruction::abx(Op::GETMOD, a, g), span)
                    }
                }
                Ok(())
            }
            Some(Binding::Prelude { .. }) => Err(CompileError::unsupported(
                "a prelude name outside a call",
                span.clone(),
            )),
            Some(Binding::Upvalue { .. }) => {
                // The resolver proved this crosses a function boundary; the
                // index is ours to assign, and asking for it builds the
                // capture chain lazily.
                let idx = self.capture_upvalue(name).ok_or_else(|| CompileError::Unsupported {
                    thing: "a captured variable the compiler could not locate",
                    span: span.clone(),
                })?;
                let b = self.reg8(idx, span)?;
                self.emit(Instruction::abc(Op::GETUPVAL, a, b, 0), span);
                Ok(())
            }
            // A static of the enclosing class, reached by its bare name from
            // inside a method. The resolver carries the *class* name here
            // rather than a slot, because the answer has to survive a lambda
            // nested inside the method — which is a different `FuncCtx` with
            // no `current_class` of its own.
            Some(Binding::ClassStatic { class, name: field }) => {
                let (class, field) = (class.clone(), field.clone());
                let Some(s) = self.static_slot_of(&class, &field) else {
                    return Err(CompileError::unsupported(
                        "a class static the compiler could not resolve",
                        span.clone(),
                    ));
                };
                // `s.class`, not the class we are *in*: an inherited static
                // lives in the cell its declaring class owns, so a sibling
                // reading the bare name sees the same value
                // (`declaring_static_field`).
                self.emit(
                    Instruction::abc(Op::GETSTAT, a, s.class as u8, s.slot as u8),
                    span,
                );
                Ok(())
            }
            Some(Binding::SelfRef) => {
                Err(CompileError::unsupported("`self`", span.clone()))
            }
            Some(Binding::WildcardImport) | None => Err(CompileError::unsupported(
                "a name the resolver could not classify",
                span.clone(),
            )),
        }
    }

    fn unary_to(
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

        let m = self.mark();
        let r = self.expr_tmp(rhs)?;
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

    fn binary_to(
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

        let m = self.mark();
        let lr = self.expr_tmp(lhs)?;
        let rr = self.expr_tmp(rhs)?;
        let (a, b, c) = (
            self.reg8(dst, span)?,
            self.reg8(lr, span)?,
            self.reg8(rr, span)?,
        );

        let result = self.binary_opcode(op, kind, a, b, c, span);
        self.free_to(m);
        result
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
        if let Some(o) = bitwise {
            if kind == Some(Num::Int) {
                self.emit(Instruction::abc(o, a, b, c), span);
                return Ok(());
            }
            // Untyped operands: `ops::bitwise` rejects a non-integer with
            // the message the tree-walker gives, which is better than a
            // compile-time refusal of a program that might be fine.
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
    fn short_circuit_to(
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

    /// A call.
    ///
    /// Two forms are emitted today, both of which skip work the tree-walker
    /// does on every call (§19):
    ///
    /// * `CALLNAT` for a prelude function. The callee is resolved to its
    ///   actual `NativeFn` **at compile time** and stored as a constant, so
    ///   nothing looks up `print` at run time.
    /// * `CALLK` for a top-level `fn`. The proto is known statically, so the
    ///   callee load, the callability test and the arity check all disappear.
    ///
    /// Arguments are evaluated directly into the callee's future frame, so a
    /// call copies nothing (§6.2).
    fn call_to(
        &mut self,
        e: &Spanned<Expr>,
        callee: &Spanned<Expr>,
        args: &[saule_ast::CallArg],
        dst: u16,
    ) -> Result<(), CompileError> {
        self.call_to_want(e, callee, args, dst, Want::Fixed(1)).map(|_| ())
    }

    /// [`call_to`](Self::call_to), asking for a specific number of results.
    ///
    /// `dst` is where a `Fixed` result run lands. For [`Want::All`] the
    /// results stay in the call window — the count is not known until the
    /// callee returns — so `dst` is used only by the shapes that produce
    /// exactly one value, and the returned [`Results`] says which happened.
    /// **The window is not released**: the caller took the mark and must
    /// free back to it once it has consumed the results.
    fn call_to_want(
        &mut self,
        e: &Spanned<Expr>,
        callee: &Spanned<Expr>,
        args: &[saule_ast::CallArg],
        dst: u16,
        want: Want,
    ) -> Result<Results, CompileError> {
        let span = &e.span;
        // Only a bare name can be a `CALLNAT`/`CALLK` target; anything else
        // is a value and falls through to the generic `CALL` below.
        let name = match &callee.value {
            Expr::Ident(n) => n.as_str(),
            _ => "",
        };
        // §19: named arguments are reordered **here**, once, into plain
        // parameter order — so every path below (`CALLK`, `CALLSTAT`,
        // `CALLM`, the constructor, `CALLNAT`) sees an ordinary positional
        // list and the runtime never sees a name.
        //
        // `gap_fill` owns the synthesized `nil`s for parameters a named call
        // skips over; `positional` borrows from it, so it has to outlive the
        // borrow.
        let gap_fill;
        let positional: Vec<&Spanned<Expr>> = if args
            .iter()
            .any(|a| matches!(a, saule_ast::CallArg::Named { .. }) || a.is_trailing_block())
        {
            let Some(params) = self.callee_param_list(callee).cloned() else {
                return Err(CompileError::unsupported(
                    "a named argument to a callee the compiler cannot identify",
                    span.clone(),
                ));
            };
            let (order, fill) = self.reorder_args(args, &params, span)?;
            gap_fill = fill;
            order
                .into_iter()
                .map(|slot| match slot {
                    ArgSlot::Given(i) => match &args[i] {
                        saule_ast::CallArg::Positional(v) => v,
                        saule_ast::CallArg::Named { value, .. } => value,
                    },
                    ArgSlot::Nil(i) => &gap_fill[i],
                })
                .collect()
        } else {
            let mut positional = Vec::with_capacity(args.len());
            for a in args {
                match a {
                    saule_ast::CallArg::Positional(v) => positional.push(v),
                    saule_ast::CallArg::Named { .. } => {
                        return Err(CompileError::unsupported("a named argument", span.clone()));
                    }
                }
            }
            positional
        };

        // `obj.method(args)` and `Class.method(args)`.
        if let Expr::Member { obj, name } = &callee.value {
            return self.method_call_to(e, obj, name, &positional, dst, want);
        }
        // `obj?.method(args)` — the nil check has to wrap the *call*, not
        // just the callee lookup, or `p?.foo()` on a nil `p` would end up
        // calling nil rather than producing nil.
        if let Expr::SafeMember { obj, name } = &callee.value {
            return self.safe_method_call_to(e, obj, name, &positional, dst, want);
        }
        // `ClassName(args)` — a constructor, not a function call.
        if let Some(class) = self.layouts.get(name)
            && self.not_shadowed(name)
        {
            self.construct_to(class, &positional, dst, span)?;
            return self.one_result(dst, want, span);
        }

        // A name with a compile-time value — the prelude, or something an
        // `import` bound to a native package. Both are fixed before the
        // program runs, so the callee becomes a constant and this is a
        // `CALLNAT` with nothing looked up at run time.
        let folded = self.static_value(callee.id, name);
        match self.binding(callee.id) {
            _ if folded.is_some() => {
                let v = folded.expect("checked");
                let k = self.constant(v, span)?;
                let m = self.mark();
                // `CALLNAT` reads its arguments from `A+1..`, mirroring
                // `CALL`, so the window has room for the callee slot even
                // though the callee itself is a constant.
                let base =
                    self.alloc_n((positional.len() as u16 + 1).max(want.slots()), span)?;
                for (i, arg) in positional.iter().enumerate() {
                    self.expr_to(arg, base + 1 + i as u16)?;
                }
                let a = self.reg8(base, span)?;
                self.emit(
                    Instruction::abc(Op::CALLNAT, a, positional.len() as u8 + 1, want.c()),
                    span,
                );
                self.emit(Instruction::ax_of(Op::EXTRAARG, k as u32), span);
                self.finish_call(base, dst, want, m, span)
            }
            // A `static fn` of the enclosing class, called by its bare name
            // — `check(code)` from a sibling static. The name is a *method*,
            // so it lives in `smindex`, not in the `sindex` that
            // `ident_to`'s static read consults; without this arm it fell
            // through to the generic `CALL` and asked for a static *field*
            // that does not exist.
            Some(Binding::ClassStatic { class, name: m })
                if self.static_method_of(class, m).is_some() =>
            {
                let (cls, slot) = self
                    .static_method_of(&class.clone(), &m.clone())
                    .expect("checked in the guard");
                let mark = self.mark();
                // `CALLSTAT`'s window starts at the arguments: a static has
                // no receiver.
                let n = (positional.len().max(1) as u16).max(want.slots());
                let base = self.alloc_n(n, span)?;
                for (i, arg) in positional.iter().enumerate() {
                    self.expr_to(arg, base + i as u16)?;
                }
                let a = self.reg8(base, span)?;
                self.emit(
                    Instruction::abc(Op::CALLSTAT, a, positional.len() as u8 + 1, want.c()),
                    span,
                );
                self.emit(
                    Instruction::ax_of(Op::EXTRAARG, ((cls as u32) << 16) | slot as u32),
                    span,
                );
                self.finish_call(base, dst, want, mark, span)
            }
            Some(Binding::Module { .. }) if self.fn_protos.contains_key(name) => {
                let proto = self.fn_protos[name];
                let m = self.mark();
                // `CALLK`'s window starts at the *arguments*: there is no
                // callee register, because the callee is the operand.
                let n = (positional.len().max(1) as u16).max(want.slots());
                let base = self.alloc_n(n, span)?;
                for (i, arg) in positional.iter().enumerate() {
                    self.expr_to(arg, base + i as u16)?;
                }
                let a = self.reg8(base, span)?;
                self.emit(
                    Instruction::abc(Op::CALLK, a, positional.len() as u8 + 1, want.c()),
                    span,
                );
                let t = self.own_call_target(proto, span)?;
                self.emit(Instruction::ax_of(Op::EXTRAARG, t), span);
                self.finish_call(base, dst, want, m, span)
            }
            // Anything else callable is a *value* — a local holding a
            // lambda, a captured one, a module slot bound to a function.
            // `CALL` loads the callee into the window's first register and
            // dispatches on what it finds, which is the general form the
            // typed ones above are specialisations of.
            _ => {
                let m = self.mark();
                let base =
                    self.alloc_n((positional.len() as u16 + 1).max(want.slots()), span)?;
                self.expr_to(callee, base)?;
                for (i, arg) in positional.iter().enumerate() {
                    self.expr_to(arg, base + 1 + i as u16)?;
                }
                let a = self.reg8(base, span)?;
                self.emit(
                    Instruction::abc(Op::CALL, a, positional.len() as u8 + 1, want.c()),
                    span,
                );
                self.finish_call(base, dst, want, m, span)
            }
        }
    }

    /// `obj.method(args)` or `Class.method(args)`.
    fn method_call_to(
        &mut self,
        e: &Spanned<Expr>,
        obj: &Spanned<Expr>,
        name: &str,
        args: &[&Spanned<Expr>],
        dst: u16,
        want: Want,
    ) -> Result<Results, CompileError> {
        let span = &e.span;

        // `Event.Click(x, y)` — a tuple-variant constructor, not a call.
        if let Expr::Ident(en) = &obj.value
            && self.not_shadowed(en)
            && let Some(e_idx) = self.layouts.enum_of(en)
            && let Some(&tag) = self.chunk.enums[e_idx as usize].by_name.get(name)
        {
            self.variant_ctor_to(e_idx, tag, args, dst, span)?;
            return self.one_result(dst, want, span);
        }

        // `self.super(args)` — the parent's `init`, dispatched **statically**.
        //
        // It cannot go through the vtable: the child overrode that slot, so
        // a virtual call would re-enter the child's own constructor and
        // recurse forever. The parent's proto is known at compile time, so
        // `CALLK` is both correct and cheaper.
        if name == "super" && matches!(obj.value, Expr::Self_) {
            let Some(class) = self.f.current_class else {
                return Err(CompileError::unsupported("`self.super` outside a method", span.clone()));
            };
            let parent = self.chunk.classes[class as usize].parent.and_then(|p| {
                let pc = &self.chunk.classes[p as usize];
                pc.init
                    .and_then(|slot| pc.vtable.get(slot as usize).copied())
                    .map(|t| (pc.module, t))
            });
            let Some((pmod, target)) = parent.filter(|(_, t)| *t != u32::MAX) else {
                return Err(CompileError::unsupported(
                    "`self.super` on a class whose parent has no constructor",
                    span.clone(),
                ));
            };
            let m = self.mark();
            let base = self.alloc_n(args.len() as u16 + 1, span)?;
            let a = self.reg8(base, span)?;
            self.emit(Instruction::abc(Op::MOVE, a, 0, 0), span);
            for (i, arg) in args.iter().enumerate() {
                self.expr_to(arg, base + 1 + i as u16)?;
            }
            self.emit(Instruction::abc(Op::CALLK, a, args.len() as u8 + 2, 1), span);
            let t = self.call_target(pmod, target, span)?;
            self.emit(Instruction::ax_of(Op::EXTRAARG, t), span);
            self.free_to(m);
            // `self.super()` is a statement, not a value.
            let d = self.reg8(dst, span)?;
            self.emit(Instruction::abc(Op::LOADNIL, d, 0, 0), span);
            return self.one_result(dst, want, span);
        }

        // A static method: the receiver is a class name, so the target is
        // known outright and no dispatch happens at all.
        if let Some(class) = self.class_named_by(obj)
            && let Some(&s) = self.chunk.classes[class as usize].smindex.get(name)
        {
            let m = self.mark();
            let base = self.alloc_n((args.len().max(1) as u16).max(want.slots()), span)?;
            for (i, arg) in args.iter().enumerate() {
                self.expr_to(arg, base + i as u16)?;
            }
            let a = self.reg8(base, span)?;
            self.emit(
                Instruction::abc(Op::CALLSTAT, a, args.len() as u8 + 1, want.c()),
                span,
            );
            // The *declaring* class, so an inherited `static fn` reached
            // through a subclass name still loads the parent's proto.
            let packed = ((s.class) << 16) | s.slot as u32;
            self.emit(Instruction::ax_of(Op::EXTRAARG, packed), span);
            return self.finish_call(base, dst, want, m, span);
        }

        // `String.len(s)`, `Table.insert(t, v)` — a static on a *stdlib*
        // class. The prelude is fixed before a program runs, so the native
        // is resolved to a constant here and nothing looks it up at run
        // time. This is the same `CALLNAT` a bare `print` compiles to; the
        // only difference is where the value was found.
        // Gated on the resolver saying `Prelude`, for the reason spelled out
        // in `member_to`'s fold: a top-level `local String` is a module slot,
        // which `f.lookup` does not see, and resolving to the stdlib anyway
        // called `String.len` where the program meant its own table.
        if let Expr::Ident(recv) = &obj.value
            && let Some(Value::Class(cls)) = self.static_value(obj.id, recv)
            && let Some(v) = cls
                .lookup_static_field(name)
                .or_else(|| cls.lookup_static_method(name).map(|m| m.to_value()))
            && matches!(v, Value::Native(_) | Value::NativeClosure(_))
        {
            let k = self.constant(v, span)?;
            let m = self.mark();
            let base = self.alloc_n((args.len() as u16 + 1).max(want.slots()), span)?;
            for (i, arg) in args.iter().enumerate() {
                self.expr_to(arg, base + 1 + i as u16)?;
            }
            let a = self.reg8(base, span)?;
            self.emit(
                Instruction::abc(Op::CALLNAT, a, args.len() as u8 + 1, want.c()),
                span,
            );
            self.emit(Instruction::ax_of(Op::EXTRAARG, k as u32), span);
            return self.finish_call(base, dst, want, m, span);
        }

        // An interface-typed receiver: the concrete class is not known, but
        // the *interface's* method numbering is, so the call names a slot
        // and the itable translates it at run time (§8.4).
        if let Some(iface) = self
            .ty_name(obj.id)
            .and_then(|t| self.layouts.interface_of(t))
            && let Some(&slot) = self.chunk.interfaces[iface as usize].index.get(name)
        {
            if iface > 0xFFFF {
                return Err(CompileError::unsupported(
                    "a program with more than 65 536 interfaces",
                    span.clone(),
                ));
            }
            let m = self.mark();
            let base = self.alloc_n((args.len() as u16 + 1).max(want.slots()), span)?;
            self.expr_to(obj, base)?;
            for (i, arg) in args.iter().enumerate() {
                self.expr_to(arg, base + 1 + i as u16)?;
            }
            let a = self.reg8(base, span)?;
            self.emit(
                Instruction::abc(Op::CALLIF, a, args.len() as u8 + 1, slot as u8),
                span,
            );
            // Packed 8/16, the same split `CALLK` uses for its module: `C`
            // is spent on the interface's method slot, so the result count
            // has nowhere else to ride. `return s.area()` is what makes
            // this live — it asks for *all* results, and truncating to one
            // would be the same silent wrong value `RET1` used to produce.
            self.emit(
                Instruction::ax_of(Op::EXTRAARG, ((want.c() as u32) << 16) | iface),
                span,
            );
            return self.finish_call(base, dst, want, m, span);
        }

        // An instance method: one indexed load out of the vtable, and the
        // slot stays correct for a subclass receiver because a subclass's
        // vtable extends its parent's (§8.3).
        // No proved class: `CALLMX` defers to the tree-walker's own
        // `dispatch_member_call_multi`, which is what makes a file handle, an
        // enum variant and a module-level variable all work here without the
        // compiler learning each one (§8.5).
        let Some(class) = self.class_of_expr(obj) else {
            let m = self.mark();
            // The receiver sits at `A` and the arguments after it, exactly
            // as `CALLM` lays them out.
            let base = self.alloc_n((args.len() as u16 + 1).max(want.slots()), span)?;
            self.expr_to(obj, base)?;
            for (i, arg) in args.iter().enumerate() {
                self.expr_to(arg, base + 1 + i as u16)?;
            }
            let key = self.constant(Value::Str(std::rc::Rc::new(name.to_string())), span)?;
            let a = self.reg8(base, span)?;
            self.emit(
                Instruction::abc(Op::CALLMX, a, args.len() as u8 + 1, want.c()),
                span,
            );
            self.emit(Instruction::ax_of(Op::EXTRAARG, key as u32), span);
            return self.finish_call(base, dst, want, m, span);
        };
        let Some(&slot) = self.chunk.classes[class as usize].vindex.get(name) else {
            return Err(CompileError::unsupported(
                "a method the class does not declare",
                span.clone(),
            ));
        };

        let m = self.mark();
        // The receiver occupies the window's first register and becomes the
        // callee's `R[0]`, so `self` costs no copy.
        let base = self.alloc_n((args.len() as u16 + 1).max(want.slots()), span)?;
        self.expr_to(obj, base)?;
        for (i, arg) in args.iter().enumerate() {
            self.expr_to(arg, base + 1 + i as u16)?;
        }
        let a = self.reg8(base, span)?;
        // `CALLM` carries the vtable slot in `C`, so it can only ever
        // produce one result. Anything else moves the slot into `EXTRAARG`
        // and lets `C` say how many — which is the whole reason §15.10
        // reserves a second opcode for this.
        if want == Want::Fixed(1) {
            self.emit(
                Instruction::abc(Op::CALLM, a, args.len() as u8 + 1, slot as u8),
                span,
            );
        } else {
            self.emit(
                Instruction::abc(Op::CALLM_MR, a, args.len() as u8 + 1, want.c()),
                span,
            );
            self.emit(Instruction::ax_of(Op::EXTRAARG, slot as u32), span);
        }
        self.finish_call(base, dst, want, m, span)
    }

    /// A call leaves its single result in the first register of its window;
    /// move it where the caller wanted it.
    /// `when(source):a(x):b(y)` — the colon pipeline (§21.4 item 9).
    ///
    /// Lowered to a chain of ordinary calls, each threading the upstream
    /// value in as argument 0, which is exactly what `eval`'s `Expr::Pipe`
    /// arm does. The value lives in one register for the whole chain and
    /// every stage writes its result back there.
    fn pipe_to(
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

    /// Put a call's arguments into **parameter order** (§19).
    ///
    /// The slot assignment is `saule_ast::resolve_arg_slots` — the very
    /// function the typechecker uses — so the two cannot disagree about
    /// which parameter an argument fills, including the trailing-block rule.
    ///
    /// A parameter left unfilled *below* the last one supplied is a gap:
    ///
    /// * nullable, no default → a synthesized `nil`, which is exactly what
    ///   the callee would have left there;
    /// * has a **default** → refused. A default must be evaluated in the
    ///   *callee's* frame (§19's stated trap), and the entry stubs can only
    ///   fill a suffix — there is no entry point meaning "fill slot 1 but
    ///   not slot 2".
    ///
    /// A trailing parameter that is simply not passed is not a gap: the call
    /// reports a shorter arity and the callee's own stub fills it.
    fn reorder_args(
        &self,
        args: &[saule_ast::CallArg],
        params: &[saule_ast::Param],
        span: &Range<usize>,
    ) -> Result<(Vec<ArgSlot>, Vec<Spanned<Expr>>), CompileError> {
        let slots: Vec<saule_ast::ParamSlot<'_>> = params
            .iter()
            .map(|p| saule_ast::ParamSlot::new(&p.name, &p.ty))
            .collect();
        let assigned = saule_ast::resolve_arg_slots(args, &slots);

        let mut filled: Vec<Option<usize>> = vec![None; params.len()];
        for (arg_i, slot) in assigned.iter().enumerate() {
            // An argument the resolver could not place, or one that fills a
            // slot twice. The typechecker reports both; refusing keeps this
            // from inventing a position.
            let Some(slot) = slot.filter(|s| filled[*s].is_none()) else {
                return Err(CompileError::unsupported(
                    "an argument that fills no parameter, or fills one twice",
                    span.clone(),
                ));
            };
            filled[slot] = Some(arg_i);
        }

        let n = filled.iter().rposition(Option::is_some).map_or(0, |i| i + 1);
        let mut order = Vec::with_capacity(n);
        let mut gaps = Vec::new();
        for (i, slot) in filled.iter().take(n).enumerate() {
            match slot {
                Some(a) => order.push(ArgSlot::Given(*a)),
                None if params[i].default.is_some() => {
                    return Err(CompileError::unsupported(
                        "a skipped parameter whose default must run in the callee",
                        span.clone(),
                    ));
                }
                None => {
                    order.push(ArgSlot::Nil(gaps.len()));
                    gaps.push(Spanned::new(Expr::Nil, span.clone()));
                }
            }
        }
        Ok((order, gaps))
    }

    /// The declared parameters of whatever `callee` names, when the compiler
    /// can identify it — a top-level `fn`, a class's constructor, a static
    /// or instance method. `None` for a callee that is only a value, where
    /// there is no declaration to read names from.
    fn callee_param_list(&self, callee: &Spanned<Expr>) -> Option<&Vec<saule_ast::Param>> {
        use super::ctx::CalleeKey;
        match &callee.value {
            Expr::Ident(n) => {
                // `ClassName(args)` is a constructor, so its parameters are
                // `init`'s.
                if let Some(c) = self.layouts.get(n).filter(|_| self.not_shadowed(n)) {
                    return self.callee_params.get(&CalleeKey::Method(c, "init".into()));
                }
                if let Some(Binding::ClassStatic { class, .. }) = self.binding(callee.id)
                    && let Some(c) = self.layouts.get(class).or(self.f.current_class)
                {
                    return self.callee_params.get(&CalleeKey::Method(c, n.clone()));
                }
                self.callee_params.get(&CalleeKey::Function(n.clone()))
            }
            Expr::Member { obj, name } => {
                let c = self.class_named_by(obj).or_else(|| self.class_of_expr(obj))?;
                self.callee_params.get(&CalleeKey::Method(c, name.clone()))
            }
            _ => None,
        }
    }

    pub(crate) fn move_result(
        &mut self,
        from: u16,
        dst: u16,
        span: &Range<usize>,
    ) -> Result<(), CompileError> {
        if from == dst {
            return Ok(());
        }
        let (a, b) = (self.reg8(dst, span)?, self.reg8(from, span)?);
        self.emit(Instruction::abc(Op::MOVE, a, b, 0), span);
        Ok(())
    }

    /// Land a call's results where the caller asked, and say where they are.
    ///
    /// `m` is the mark taken before the call window was allocated. A `Fixed`
    /// count moves down to `dst` — ascending, which is safe because the
    /// window is always above `dst` — and the window is released.
    /// [`Want::All`] can move nothing, since the count is only known once the
    /// callee has returned, so the window **stays allocated** and its base is
    /// handed back for the caller to consume with `RET A 0`.
    fn finish_call(
        &mut self,
        base: u16,
        dst: u16,
        want: Want,
        m: crate::compile::regalloc::Mark,
        span: &Range<usize>,
    ) -> Result<Results, CompileError> {
        match want {
            Want::All => Ok(Results { base, count: None }),
            Want::Fixed(n) => {
                for i in 0..n as u16 {
                    self.move_result(base + i, dst + i, span)?;
                }
                self.free_to(m);
                Ok(Results {
                    base: dst,
                    count: Some(n),
                })
            }
        }
    }

    /// A call shape that yields exactly one value, already written to `dst`.
    ///
    /// Constructors, variant constructors, `self.super()` and pipelines are
    /// all single-valued under the tree-walker too, so padding the surplus
    /// with nil here is `eval_expr_list`'s own rule rather than a shortcut:
    /// `local a, b = Foo()` binds the instance and a nil.
    fn one_result(
        &mut self,
        dst: u16,
        want: Want,
        span: &Range<usize>,
    ) -> Result<Results, CompileError> {
        let n = match want {
            Want::Fixed(n) => n,
            Want::All => return Ok(Results { base: dst, count: Some(1) }),
        };
        for i in 1..n as u16 {
            let a = self.reg8(dst + i, span)?;
            self.emit(Instruction::abc(Op::LOADNIL, a, 0, 0), span);
        }
        Ok(Results {
            base: dst,
            count: Some(n),
        })
    }

    /// `obj.name` — a field read, or a static read off a class name.
    fn member_to(
        &mut self,
        e: &Spanned<Expr>,
        obj: &Spanned<Expr>,
        name: &str,
        dst: u16,
    ) -> Result<(), CompileError> {
        let span = &e.span;
        let a = self.reg8(dst, span)?;

        // `self.count` inside a `static fn`. There, `self` denotes the
        // **class** — `call_static_method_multi` binds it to
        // `Value::Class` — so this is a static read, resolved at compile
        // time. Which also means the VM never needs a class in a register:
        // `self` bare in a static method is still refused, and no fixture
        // asks for it.
        if matches!(obj.value, Expr::Self_)
            && !self.f.in_method
            && let Some(class) = self.f.current_class
            && let Some(&s) = self.chunk.classes[class as usize].sindex.get(name)
        {
            self.emit(
                Instruction::abc(Op::GETSTAT, a, s.class as u8, s.slot as u8),
                span,
            );
            return Ok(());
        }

        // `Status.Alive` — a singleton variant reference.
        if let Expr::Ident(en) = &obj.value
            && self.not_shadowed(en)
            && let Some(e_idx) = self.layouts.enum_of(en)
            && let Some(&tag) = self.chunk.enums[e_idx as usize].by_name.get(name)
        {
            return self.variant_ref_to(e_idx, tag, dst, span);
        }

        // `Counter.total` — the receiver is a type name, not a value. No
        // subclass ambiguity here: the class is named outright.
        if let Some(class) = self.class_named_by(obj)
            && let Some(&s) = self.chunk.classes[class as usize].sindex.get(name)
        {
            self.emit(
                Instruction::abc(Op::GETSTAT, a, s.class as u8, s.slot as u8),
                span,
            );
            return Ok(());
        }

        // `Math.pi`, `Os.sep`, `IoMode.Write` — a stdlib member holding a
        // value. The prelude is fixed before a program runs, so this is
        // resolved to the value itself and the read costs one `LOADK` —
        // the same compile-time resolution `String.len` already gets on the
        // call path, applied to members that are not functions.
        //
        // Gated on the **resolver's** classification, not on a hand-rolled
        // "is it a local, a class, an enum" check. A top-level `local Math`
        // is a *module slot* rather than a frame local, so `f.lookup` misses
        // it — and folding then read the stdlib's `pi` where the program
        // meant its own table. The resolver already answers "what is this
        // name", and `Prelude` is the only answer that may be folded.
        if let Expr::Ident(recv) = &obj.value
            && !self.mutated_receivers.contains(recv)
            && let Some(v) = self.prelude_member(obj.id, recv, name)
        {
            let k = self.constant(v, span)?;
            self.emit(Instruction::abx(Op::LOADK, a, k), span);
            return Ok(());
        }

        // `variant.value` — the payload a valued variant carries. Not a
        // field: an enum variant has no layout, so this is `UNWRAP`.
        if name == "value"
            && self
                .ty_name(obj.id)
                .is_some_and(|t| self.layouts.enum_of(t).is_some())
        {
            let m = self.mark();
            let r = self.expr_tmp(obj)?;
            let b = self.reg8(r, span)?;
            self.emit(Instruction::abc(Op::UNWRAP, a, b, 0), span);
            self.free_to(m);
            return Ok(());
        }

        // `p.health` where the front end proved `p`'s class: one indexed
        // load. Without a proved class there is no safe slot to use — a
        // wrong one reads a different field silently — so it is refused
        // rather than guessed.
        // `t.foo` on a table is `t["foo"]` — Lua-style sugar, and a miss is
        // `nil` rather than an error, so it is a safe probe (`members.rs`).
        // Nothing to do with field slots.
        if matches!(self.types.get(&obj.id), Some(saule_ast::Type::Table { .. })) {
            let m = self.mark();
            let r = self.expr_tmp(obj)?;
            let key = self.constant(Value::Str(std::rc::Rc::new(name.to_string())), span)?;
            self.map_key_read(dst, r, key, span)?;
            self.free_to(m);
            return Ok(());
        }

        // No proved class: `GETFX` asks the tree-walker's own `read_member`
        // at run time (§8.5). A missing type selects the dynamic form; it is
        // never a wrong opcode.
        let Some(class) = self.class_of_expr(obj) else {
            let m = self.mark();
            let r = self.expr_tmp(obj)?;
            let key = self.constant(Value::Str(std::rc::Rc::new(name.to_string())), span)?;
            let Ok(kc) = u8::try_from(key) else {
                return Err(CompileError::unsupported(
                    "a dynamic member name past the 256-constant window",
                    span.clone(),
                ));
            };
            let (a, b) = (self.reg8(dst, span)?, self.reg8(r, span)?);
            self.emit(Instruction::abc(Op::GETFX, a, b, kc), span);
            self.free_to(m);
            return Ok(());
        };
        let Some(access) = self.member_access(class, name) else {
            return Err(CompileError::unsupported(
                "a member that is neither an instance field nor a static",
                span.clone(),
            ));
        };
        // The receiver is evaluated even when the answer is a static: the
        // tree-walker evaluates it too, and it may have side effects.
        let m = self.mark();
        let r = self.expr_tmp(obj)?;
        let b = self.reg8(r, span)?;
        self.emit(self.member_load(access, a, b), span);
        self.free_to(m);
        Ok(())
    }

    /// `R[dst] := R[table][K[key]]` — a table read on a constant key.
    ///
    /// `GETMAPK` carries the constant index in its 8-bit `C`, so a key
    /// interned past 255 does not fit. Rather than cap a module at 256
    /// constants — which any real program reaches, on an operation as
    /// ordinary as `t.name` — the key is materialised into a register and
    /// the general `GETIDX` does the read. One extra instruction, no cliff.
    pub(crate) fn map_key_read(
        &mut self,
        dst: u16,
        table: u16,
        key: u16,
        span: &Range<usize>,
    ) -> Result<(), CompileError> {
        let (a, b) = (self.reg8(dst, span)?, self.reg8(table, span)?);
        if let Ok(c) = u8::try_from(key) {
            self.emit(Instruction::abc(Op::GETMAPK, a, b, c), span);
            return Ok(());
        }
        let m = self.mark();
        let k = self.alloc(span)?;
        let kr = self.reg8(k, span)?;
        self.emit(Instruction::abx(Op::LOADK, kr, key), span);
        self.emit(Instruction::abc(Op::GETIDX, a, b, kr), span);
        self.free_to(m);
        Ok(())
    }

    /// `R[table][K[key]] := R[value]`, with the same 8-bit escape hatch as
    /// [`Self::map_key_read`] — `SETMAPK` holds the key index in `B`.
    pub(crate) fn map_key_write(
        &mut self,
        table: u16,
        key: u16,
        value: u16,
        span: &Range<usize>,
    ) -> Result<(), CompileError> {
        let (a, c) = (self.reg8(table, span)?, self.reg8(value, span)?);
        if let Ok(b) = u8::try_from(key) {
            self.emit(Instruction::abc(Op::SETMAPK, a, b, c), span);
            return Ok(());
        }
        let m = self.mark();
        let k = self.alloc(span)?;
        let kr = self.reg8(k, span)?;
        self.emit(Instruction::abx(Op::LOADK, kr, key), span);
        self.emit(Instruction::abc(Op::SETIDX, a, kr, c), span);
        self.free_to(m);
        Ok(())
    }

    /// How `obj.name` resolves against a class the front end proved.
    ///
    /// `None` means "refuse", never "guess" — including the one case where
    /// the static answer would depend on the receiver's *runtime* class
    /// rather than its declared one.
    pub(crate) fn member_access(
        &self,
        class: crate::chunk::ClassIdx,
        name: &str,
    ) -> Option<MemberAccess> {
        let proto = &self.chunk.classes[class as usize];
        if let Some(slot) = proto.layout.slot(name) {
            return Some(MemberAccess::Field(slot));
        }
        let s = *proto.sindex.get(name)?;
        // A class-level default with no `init` is a *static*, so `b.label`
        // is a legitimate static read through an instance. The slot is
        // resolved against the receiver's declared class — which is the
        // right cell unless some subclass redeclares the name, since then
        // the answer depends on what the receiver actually is at run time.
        if self.a_subclass_shadows_static(class, name, s) {
            return None;
        }
        Some(MemberAccess::Static(s))
    }

    /// Whether any descendant of `class` declares its own `name`, making a
    /// statically resolved static read ambiguous.
    fn a_subclass_shadows_static(
        &self,
        class: crate::chunk::ClassIdx,
        name: &str,
        resolved: crate::chunk::StaticSlot,
    ) -> bool {
        self.chunk.classes.iter().enumerate().any(|(i, c)| {
            i as u32 != class
                && c.sindex.get(name).is_some_and(|s| *s != resolved)
                && self.descends_from(i as u32, class)
        })
    }

    fn descends_from(&self, mut who: crate::chunk::ClassIdx, ancestor: crate::chunk::ClassIdx) -> bool {
        while let Some(p) = self.chunk.classes[who as usize].parent {
            if p == ancestor {
                return true;
            }
            who = p;
        }
        false
    }

    /// The instruction that loads a resolved member into `a` from a
    /// receiver in `b`.
    fn member_load(&self, access: MemberAccess, a: u8, b: u8) -> Instruction {
        match access {
            MemberAccess::Field(slot) => Instruction::abc(Op::GETF, a, b, slot as u8),
            MemberAccess::Static(s) => {
                Instruction::abc(Op::GETSTAT, a, s.class as u8, s.slot as u8)
            }
        }
    }

    /// `obj[index]`.
    ///
    /// `GETIDX` is a *table* read despite §15.9 calling it the dynamic form,
    /// so an instance receiver needs its `OpIndex` overload resolved here —
    /// for the same reason the arithmetic overloads are (§8.7): a bytecode
    /// method never reaches the runtime `ClassObject`'s method map, so no
    /// run-time lookup could find it.
    fn index_to(
        &mut self,
        e: &Spanned<Expr>,
        obj: &Spanned<Expr>,
        index: &Spanned<Expr>,
        dst: u16,
    ) -> Result<(), CompileError> {
        let span = &e.span;

        if let Some(class) = self.class_of_expr(obj) {
            let contract = saule_ast::ops::OP_INDEX;
            let Some(&slot) = self.chunk.classes[class as usize]
                .vindex
                .get(contract.method)
            else {
                // A class receiver with no `index` method: `GETIDX` would
                // report "expected `table`", blaming the compiler for a
                // program error. The tree-walker has the right message.
                return Err(CompileError::unsupported(
                    "an index read on a class with no `OpIndex` overload",
                    span.clone(),
                ));
            };
            let m = self.mark();
            let base = self.alloc_n(2, span)?;
            self.expr_to(obj, base)?;
            self.expr_to(index, base + 1)?;
            let a = self.reg8(base, span)?;
            self.emit(Instruction::abc(Op::CALLM, a, 2, slot as u8), span);
            self.move_result(base, dst, span)?;
            self.free_to(m);
            return Ok(());
        }

        let m = self.mark();
        let o = self.expr_tmp(obj)?;
        let i = self.expr_tmp(index)?;
        let (a, b, c) = (
            self.reg8(dst, span)?,
            self.reg8(o, span)?,
            self.reg8(i, span)?,
        );
        self.emit(Instruction::abc(Op::GETIDX, a, b, c), span);
        self.free_to(m);
        Ok(())
    }

    /// `obj?.name` — a member read that yields `nil` when the receiver is
    /// nil instead of faulting on it.
    fn safe_member_to(
        &mut self,
        e: &Spanned<Expr>,
        obj: &Spanned<Expr>,
        name: &str,
        dst: u16,
    ) -> Result<(), CompileError> {
        let span = &e.span;

        // Resolve the slot *before* emitting anything. A refusal has to
        // leave the code array untouched, because `Unsupported` falls back
        // to the tree-walker and half-written code cannot be taken back.
        let Some(class) = self.class_of_nullable_expr(obj) else {
            return Err(CompileError::unsupported(
                "a safe member access on a receiver with no proved class",
                span.clone(),
            ));
        };
        let Some(access) = self.member_access(class, name) else {
            return Err(CompileError::unsupported(
                "a safe member access on a member that is neither an instance field nor a static",
                span.clone(),
            ));
        };

        let m = self.mark();
        let r = self.expr_tmp(obj)?;
        let (a, b) = (self.reg8(dst, span)?, self.reg8(r, span)?);
        // `JNOTNIL` skips the jump when the receiver is present, so the
        // jump is taken exactly on the nil path.
        self.emit(Instruction::abc(Op::JNOTNIL, b, 0, 0), span);
        let to_nil = self.emit_jump(Op::JMP, 0, span);
        self.emit(self.member_load(access, a, b), span);
        let done = self.emit_jump(Op::JMP, 0, span);
        self.patch_here(to_nil)?;
        self.emit(Instruction::abc(Op::LOADNIL, a, 0, 0), span);
        self.patch_here(done)?;
        self.free_to(m);
        Ok(())
    }

    /// `obj?.method(args)`.
    ///
    /// The nil check guards the **whole call**, arguments included. That is
    /// not an optimisation: the tree-walker returns before it evaluates them
    /// (`eval/expr/mod.rs`), so evaluating them here would run side effects
    /// it does not.
    fn safe_method_call_to(
        &mut self,
        e: &Spanned<Expr>,
        obj: &Spanned<Expr>,
        name: &str,
        args: &[&Spanned<Expr>],
        dst: u16,
        want: Want,
    ) -> Result<Results, CompileError> {
        let span = &e.span;

        let Some(class) = self.class_of_nullable_expr(obj) else {
            return Err(CompileError::unsupported(
                "a safe method call on a receiver with no proved class",
                span.clone(),
            ));
        };
        let Some(&slot) = self.chunk.classes[class as usize].vindex.get(name) else {
            return Err(CompileError::unsupported(
                "a safe method call on a method the class does not declare",
                span.clone(),
            ));
        };
        // The nil arm has to produce *as many* results as the call arm, and
        // for `Want::All` that count is a run-time fact carried in `top` —
        // which nothing but a return can set. `return x?.m()` therefore
        // refuses and falls back rather than returning one value where the
        // tree-walker returns several.
        let Want::Fixed(nret) = want else {
            return Err(CompileError::unsupported(
                "a safe method call whose results are passed straight through by `return`",
                span.clone(),
            ));
        };

        let m = self.mark();
        let base = self.alloc_n((args.len() as u16 + 1).max(want.slots()), span)?;
        self.expr_to(obj, base)?;
        let rb = self.reg8(base, span)?;
        self.emit(Instruction::abc(Op::JNOTNIL, rb, 0, 0), span);
        let to_nil = self.emit_jump(Op::JMP, 0, span);
        for (i, arg) in args.iter().enumerate() {
            self.expr_to(arg, base + 1 + i as u16)?;
        }
        if nret == 1 {
            self.emit(
                Instruction::abc(Op::CALLM, rb, args.len() as u8 + 1, slot as u8),
                span,
            );
        } else {
            self.emit(
                Instruction::abc(Op::CALLM_MR, rb, args.len() as u8 + 1, want.c()),
                span,
            );
            self.emit(Instruction::ax_of(Op::EXTRAARG, slot as u32), span);
        }
        for i in 0..nret as u16 {
            self.move_result(base + i, dst + i, span)?;
        }
        let done = self.emit_jump(Op::JMP, 0, span);
        self.patch_here(to_nil)?;
        for i in 0..nret as u16 {
            let d = self.reg8(dst + i, span)?;
            self.emit(Instruction::abc(Op::LOADNIL, d, 0, 0), span);
        }
        self.patch_here(done)?;
        self.free_to(m);
        Ok(Results {
            base: dst,
            count: Some(nret),
        })
    }

    /// A lambda.
    ///
    /// Compiles the body into a nested proto, then emits `CLOSURE`, which
    /// binds the upvalues at run time from the descriptors the body's
    /// compilation produced. The descriptors are built by `capture_upvalue`
    /// as the body refers to names, so the list is exactly the free-variable
    /// set `saule-semantic` proved — no over-approximation.
    fn lambda_to(
        &mut self,
        e: &Spanned<Expr>,
        params: &[saule_ast::Param],
        body: &saule_ast::LambdaBody,
        dst: u16,
    ) -> Result<(), CompileError> {
        let span = &e.span;
        if params.len() > u8::MAX as usize {
            return Err(CompileError::unsupported(
                "a lambda with over 255 parameters",
                span.clone(),
            ));
        }

        self.push_function(None);
        let outcome = (|| -> Result<(), CompileError> {
            let label = self.func_label();
            self.f
                .regs
                .reserve_params(params.len() as u16)
                .map_err(|o| o.at(&label, span.clone()))?;
            self.f.n_params = params.len() as u8;
            for (i, p) in params.iter().enumerate() {
                self.f.declare(&p.name, i as u16);
            }
            self.f.entries = self.param_entries(params, 0, span)?;
            match body {
                saule_ast::LambdaBody::Expr(inner) => {
                    // An expression-bodied lambda returns its expression.
                    let m = self.mark();
                    let r = self.expr_tmp(inner)?;
                    let a = self.reg8(r, span)?;
                    self.emit(Instruction::abc(Op::RET1, a, 0, 0), span);
                    self.free_to(m);
                }
                saule_ast::LambdaBody::Block(stmts) => {
                    for st in stmts.iter() {
                        self.stmt(st)?;
                    }
                }
            }
            Ok(())
        })();

        // Popped unconditionally: leaving the compiler inside a half-built
        // function after an error would corrupt every later diagnostic.
        let proto = self.pop_function(span);
        outcome?;
        let idx = self.chunk.add_proto(proto);
        let nested = self.f.nested_index(idx);
        let a = self.reg8(dst, span)?;
        self.emit(Instruction::abx(Op::CLOSURE, a, nested), span);
        Ok(())
    }

    /// A table literal.
    ///
    /// `NEWT` takes size hints straight from the literal, so the array and
    /// the map are allocated once at the right capacity rather than grown;
    /// `SETLIST` then moves a whole register range into the array part in
    /// one go, replacing the per-entry push the tree-walker does (§10).
    fn table_to(
        &mut self,
        e: &Spanned<Expr>,
        entries: &[saule_ast::TableEntry],
        dst: u16,
    ) -> Result<(), CompileError> {
        use saule_ast::TableEntry;
        let span = &e.span;

        let n_array = entries
            .iter()
            .filter(|x| matches!(x, TableEntry::Positional(_)))
            .count();
        let n_map = entries.len() - n_array;
        if n_array > u8::MAX as usize || n_map > u8::MAX as usize {
            return Err(CompileError::unsupported(
                "a table literal with more than 255 entries of one kind",
                span.clone(),
            ));
        }

        // Built in a scratch register and moved at the end, so a literal
        // that mentions `dst` in one of its own entries still reads the old
        // value — `t = {t}` must not see the half-built table.
        let m = self.mark();
        let t = self.alloc(span)?;
        let ta = self.reg8(t, span)?;
        self.emit(
            Instruction::abc(Op::NEWT, ta, n_array as u8, n_map as u8),
            span,
        );

        // Positional entries go in as one contiguous run.
        if n_array > 0 {
            let run = self.alloc_n(n_array as u16, span)?;
            let mut i = 0u16;
            for entry in entries {
                if let TableEntry::Positional(v) = entry {
                    self.expr_to(v, run + i)?;
                    i += 1;
                }
            }
            // `SETLIST` reads `R[A+1]..R[A+B]`, so the run has to sit
            // directly above the table register.
            debug_assert_eq!(run, t + 1, "SETLIST needs its values above the table");
            self.emit(
                Instruction::abc(Op::SETLIST, ta, n_array as u8, 0),
                span,
            );
        }

        for entry in entries {
            if let TableEntry::Field { key, value } = entry {
                let em = self.mark();
                let k = self.expr_tmp(key)?;
                let v = self.expr_tmp(value)?;
                let (kb, vc) = (self.reg8(k, span)?, self.reg8(v, span)?);
                self.emit(Instruction::abc(Op::SETIDX, ta, kb, vc), span);
                self.free_to(em);
            }
        }

        self.move_result(t, dst, span)?;
        self.free_to(m);
        Ok(())
    }

    /// The numeric kind the typechecker proved for a node, if any.
    pub fn num_of_node(&self, e: &Spanned<Expr>) -> Option<Num> {
        self.ty_name(e.id).and_then(num_of)
    }
}

/// What `obj.name` turned out to be, once resolved against a proved class.
///
/// A class-level field with a default and no `init` is promoted to a static
/// by both engines, so "member read" covers both shapes and the difference
/// is which opcode loads it.
#[derive(Debug, Clone, Copy)]
pub(crate) enum MemberAccess {
    Field(u16),
    Static(crate::chunk::StaticSlot),
}

/// A human-readable name for an unsupported construct, so the diagnostic
/// says what is missing rather than just "expression".
fn expr_label(e: &Expr) -> &'static str {
    match e {
        Expr::Pipe { .. } => "a pipe",
        Expr::Error => "an unparsable expression",
        _ => "this expression",
    }
}

/// Where one argument slot's value comes from, after §19's reordering.
enum ArgSlot {
    /// Index into the call's own `args`.
    Given(usize),
    /// Index into the synthesized-`nil` list, for a parameter a named call
    /// skipped over.
    Nil(usize),
}
