//! The two composite literals: lambdas and table constructors.

use saule_ast::{Expr, Spanned};

use super::CompileError;
use super::super::ctx::Compiler;
use crate::op::{Instruction, Op};

impl Compiler<'_> {

    /// A lambda.
    ///
    /// Compiles the body into a nested proto, then emits `CLOSURE`, which
    /// binds the upvalues at run time from the descriptors the body's
    /// compilation produced. The descriptors are built by `capture_upvalue`
    /// as the body refers to names, so the list is exactly the free-variable
    /// set `saule-semantic` proved — no over-approximation.
    pub(crate) fn lambda_to(
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

        // Taken, not read: `binding_lambda_to` names *this* lambda only. A
        // lambda nested inside it must not inherit the name, or a reference
        // there would compile to the inner closure instead of the outer one.
        let self_fn_name = self.binding_lambda_to.take();
        self.push_function(None);
        self.f.self_fn_name = self_fn_name;
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
            self.coerce_params(params, 0, span)?;
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
    pub(crate) fn table_to(
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
}
