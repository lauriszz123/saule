//! Operand rules: whether a subexpression can be read where it already is.
//!
//! Copying every operand into a fresh temporary is always correct and often
//! wasteful. These three answer when the copy can be skipped — which is a
//! question about *purity*, because an in-place read of a captured local is
//! only safe if nothing between the read and the use can write to it.

use crate::compile::CompileError;

use super::Compiler;

impl Compiler<'_> {

    // ---- operands already in a register --------------------------------

    /// The register `e` **already** lives in, if reusing it as an operand
    /// costs nothing.
    ///
    /// `MOVE` is the most-executed opcode in every benchmark this project
    /// has — 25% of `loop_arith`, 30% of `fib`, 42% of `oop`, 26% of `sort`
    /// under `--profile-bytecode` — and most of them are this: a local or a
    /// parameter copied into a fresh temporary purely to be an operand.
    /// `fib`'s `n < 2` emitted `MOVE 1 0` before the comparison even though
    /// `n` was sitting in register 0 the whole time.
    ///
    /// Only a **frame local** counts, and only when the resolver agrees it
    /// is one. A name the resolver classified as a module slot, an upvalue,
    /// a class static or a prelude entity needs real code emitted for it,
    /// and `FuncCtx::lookup` answering for such a name would be answering a
    /// different question than the one asked.
    pub fn in_place_operand(&self, e: &saule_ast::Spanned<saule_ast::Expr>) -> Option<u16> {
        use saule_ast::Expr;
        match &e.value {
            // `self` is parameter 0 by construction (§6.2); a static method
            // has no receiver, and `in_method` is exactly that distinction.
            Expr::Self_ if self.f.in_method => Some(0),
            Expr::Ident(name) => match self.binding(e.id) {
                Some(saule_semantic::Binding::Local { .. }) => self.f.lookup(name),
                _ => None,
            },
            _ => None,
        }
    }

    /// Whether evaluating `e` can run **no** user code and write **no**
    /// register other than its own destination.
    ///
    /// This is what makes reusing a register in place safe rather than
    /// merely tempting. Reading a local early and consuming it late is only
    /// equivalent to copying it if nothing in between can change it — and
    /// something can: a captured local is an *open* upvalue pointing at this
    /// frame's register, so a closure called between the two would write
    /// through it and the operand would read the new value where the
    /// tree-walker read the old one.
    ///
    /// Rather than reason about which locals are captured (a fact that is
    /// not even settled until a later lambda in the same function is
    /// compiled), require that every operand of the instruction is pure, so
    /// there is no "in between" at all.
    pub fn operand_is_pure(&self, e: &saule_ast::Spanned<saule_ast::Expr>) -> bool {
        use saule_ast::{BinOp, Expr};
        match &e.value {
            Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Bool(_) | Expr::Nil => true,
            // Arithmetic on pure operands is itself pure — but **only** with
            // a proved numeric kind on both sides. Without one the operator
            // compiles to `ARITHX`, which calls `ops::binary`, which
            // dispatches an `Op*` overload: user code, in the middle of what
            // this function is promising runs none. `s + i * 2` is the shape
            // that needs this; `loop_arith`'s inner loop is nothing else.
            Expr::Binary { op, lhs, rhs }
                if !matches!(op, BinOp::And | BinOp::Or | BinOp::Coalesce | BinOp::Concat) =>
            {
                matches!(
                    (self.num_of_node(lhs), self.num_of_node(rhs)),
                    (Some(l), Some(r)) if l == r
                ) && self.operand_is_pure(lhs)
                    && self.operand_is_pure(rhs)
            }
            Expr::Unary { op, rhs } => {
                matches!(op, saule_ast::UnaryOp::Neg)
                    && self.num_of_node(rhs).is_some()
                    && self.operand_is_pure(rhs)
            }
            // Reading an element of a plain table runs no user code: it
            // compiles to `GETIDX`/`GETIDXU`, which touch the table and the
            // destination and nothing else. A class receiver is the case
            // that *would* — `index_to` sends it to the `OpIndex` overload,
            // which is a method call — so the receiver's type decides.
            //
            // This does not let the index read itself happen in place;
            // `in_place_operand` still answers `None` for it. What it unlocks
            // is the operand **beside** it: `aik * b[j]!` copied `aik` into a
            // temporary purely because its neighbour was an index, and that
            // one `MOVE` was a third of the matrix inner loop.
            Expr::Index { obj, index } => {
                self.class_of_expr(obj).is_none()
                    && self.operand_is_pure(obj)
                    && self.operand_is_pure(index)
            }
            // `x!` is `UNWRAPNIL` — one register written, no user code. It
            // can raise, but raising unwinds rather than running anything
            // that could write the operand beside it.
            Expr::ForceUnwrap(inner) => self.operand_is_pure(inner),
            // A field read on a class whose layout resolves the name is
            // `GETF`: one indexed load out of the instance, no user code.
            // The slot lookup is the whole test — a member that is *not* a
            // known field is a method reference, a static, a module slot or
            // an `any` receiver, and those compile to something that can
            // run code or that this function has no business promising for.
            //
            // `self.pos = self.pos + 1` is the shape this unlocks, and it is
            // the inner loop of every scanner: the receiver was copied into
            // a temporary purely because the right-hand side mentioned a
            // field.
            Expr::Member { obj, name } => {
                self.class_of_expr(obj)
                    .and_then(|c| self.chunk.classes[c as usize].layout.slot(name))
                    .is_some()
                    && self.operand_is_pure(obj)
            }
            _ => self.in_place_operand(e).is_some(),
        }
    }

    /// `e` into a register: the one it is already in when that is safe,
    /// otherwise a fresh temporary.
    ///
    /// `in_place` is the caller's judgement that nothing runs between this
    /// read and the instruction that consumes it — see
    /// [`Compiler::operand_is_pure`].
    pub fn operand_to_reg(
        &mut self,
        e: &saule_ast::Spanned<saule_ast::Expr>,
        in_place: bool,
    ) -> Result<u16, CompileError> {
        if in_place && let Some(r) = self.in_place_operand(e) {
            return Ok(r);
        }
        self.expr_tmp(e)
    }
}
