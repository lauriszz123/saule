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
//!
//! ## What lives where
//!
//! | file | holds |
//! |---|---|
//! | `mod.rs` | the three entry points every other file calls back into |
//! | [`ident`] | a name in a value position |
//! | [`arith`] | unary, binary, immediate forms, and short-circuiting |
//! | [`call`] | plain calls and method calls |
//! | [`args`] | §19 argument reordering, at the call site |
//! | [`results`] | [`Want`] and [`Results`]: how many values, and where they landed |
//! | [`pipe`] | the pipe operator |
//! | [`member`] | `a.b`, `a[b]`, and what a member turned out to be |
//! | [`safe`] | the `?.` family |
//! | [`literal`] | lambdas and table constructors |

pub mod args;
pub mod arith;
pub mod call;
pub mod ident;
pub mod literal;
pub mod member;
pub mod pipe;
pub mod results;
pub mod safe;

pub(crate) use results::{Results, Want};


use saule_ast::{BinOp, Expr, Spanned};
use saule_interpreter::Value;

use super::CompileError;
use super::ctx::{Compiler, Num, num_of};
use crate::op::{Instruction, Op};

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
                if self.f.in_method {
                    // `self` is parameter 0, so it is already in register 0.
                    if a != 0 {
                        self.emit(Instruction::abc(Op::MOVE, a, 0, 0), span);
                    }
                } else if let Some(idx) = self.capture_upvalue("self") {
                    // A lambda written in a method body. `self` is an
                    // ordinary local of the enclosing frame — `method_proto`
                    // declares it under that name at register 0 — so the
                    // same capture walk every other free variable takes
                    // reaches it, and `CLOSEUP` keeps it alive if the
                    // closure outlives the call. Mirrors the tree-walker,
                    // where a lambda body mentioning `self` captures it by
                    // name like anything else.
                    let b = self.reg8(idx, span)?;
                    self.emit(Instruction::abc(Op::GETUPVAL, a, b, 0), span);
                } else {
                    // Nothing enclosing declares it: a `static fn`, where
                    // `self` denotes the class rather than an instance.
                    // `member_to` folds `self.x` there at compile time; bare
                    // `self` would need a class in a register, which no
                    // opcode produces.
                    return Err(CompileError::unsupported("`self` outside a method", span.clone()));
                }
            }

            Expr::Member { obj, name } => self.member_to(e, obj, name, dst)?,

            Expr::SafeMember { obj, name } => self.safe_member_to(e, obj, name, dst)?,

            // `x!` — the value, or `ForceUnwrapNil` at this span.
            //
            // `(x as T)!` fuses into one `CASTUNWRAP`. Emitted at the
            // **`!`'s** span, which is the span `UNWRAPNIL` carried, so the
            // error a failed cast raises points where it always did.
            Expr::ForceUnwrap(inner) => match &inner.value {
                Expr::Cast { value, ty } => self.cast_to(value, ty, dst, span, true)?,
                _ => {
                    let m = self.mark();
                    let r = self.operand_to_reg(inner, self.operand_is_pure(inner))?;
                    let (a, b) = (self.reg8(dst, span)?, self.reg8(r, span)?);
                    self.emit(Instruction::abc(Op::UNWRAPNIL, a, b, 0), span);
                    self.free_to(m);
                }
            },

            // `x as T`. The type travels as an index into the chunk's cast
            // table rather than as a `TypeDesc`, because the test is deep —
            // see `Chunk::cast_types`.
            Expr::Cast { value, ty } => self.cast_to(value, ty, dst, span, false)?,

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


    /// The numeric kind the typechecker proved for a node, if any.
    pub fn num_of_node(&self, e: &Spanned<Expr>) -> Option<Num> {
        self.ty_name(e.id).and_then(num_of)
    }

    /// `value as ty`, optionally force-unwrapped in the same instruction.
    ///
    /// One function for both because the two differ only in the opcode and
    /// in what a failed cast does — `CASTCHK` yields nil, `CASTUNWRAP`
    /// raises `ForceUnwrapNil`. Writing them apart would leave two places
    /// that have to agree about interning the cast type and about the
    /// 256-type limit.
    ///
    /// `span` is the caller's, not the cast's: for the fused form it is the
    /// `!`'s span, which is the one `UNWRAPNIL` used to carry, so a failed
    /// cast still points where it always did.
    fn cast_to(
        &mut self,
        value: &Spanned<Expr>,
        ty: &saule_ast::Type,
        dst: u16,
        span: &std::ops::Range<usize>,
        unwrap: bool,
    ) -> Result<(), CompileError> {
        let k = self.chunk.add_cast_type(ty);
        let Ok(k) = u8::try_from(k) else {
            return Err(CompileError::unsupported(
                "a module casting to more than 256 distinct types",
                span.clone(),
            ));
        };
        // `sort`'s comparator is `(a as integer)! < (b as integer)!` on
        // untyped lambda parameters, so this pair is 46% of that benchmark
        // and every one of them used to be preceded by a `MOVE` of a
        // parameter into a temporary as well.
        let m = self.mark();
        let r = self.operand_to_reg(value, self.operand_is_pure(value))?;
        let (a, b) = (self.reg8(dst, span)?, self.reg8(r, span)?);
        let op = if unwrap { Op::CASTUNWRAP } else { Op::CASTCHK };
        self.emit(Instruction::abc(op, a, b, k), span);
        self.free_to(m);
        Ok(())
    }
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
