//! `try`/`catch`, and the type descriptors a `catch` clause tests against.
//!
//! A handler is a range of instructions plus a type to match, recorded on
//! the proto rather than emitted inline (§12.1), so the non-throwing path
//! costs nothing.

use saule_ast::{Spanned, Stmt};

use super::super::CompileError;
use super::super::ctx::Compiler;
use super::super::ctx::func::PendingFinally;
use crate::op::{Instruction, Op};

impl Compiler<'_> {
    /// `try … [catch e: T …]… [finally …] end`.
    ///
    /// Entering the `try` emits **no instructions at all** (§12.1): the
    /// protected range is recorded out of band in the proto's handler table,
    /// and only a `throw` ever consults it. The happy path costs nothing.
    pub(crate) fn try_catch(
        &mut self,
        body: &[Spanned<Stmt>],
        catches: &[saule_ast::CatchClause],
        finally: Option<&[Spanned<Stmt>]>,
        span: &std::ops::Range<usize>,
    ) -> Result<(), CompileError> {
        // The register the caught value lands in has to be reserved for the
        // whole protected range, not just the handler: unwinding writes it
        // before any of the handler's own code runs. One register serves
        // every clause, since at most one of them ever runs.
        let err_reg = self.alloc(span)?;
        let err8 = self.reg8(err_reg, span)?;

        // Pending from here on, so a `return` / `break` / `continue` inside
        // the body or any clause runs it on the way out.
        if let Some(f) = finally {
            self.f.finallys.push(PendingFinally {
                body: f.to_vec(),
                loops_at_entry: self.loops.len(),
            });
        }

        let start = self.f.label_here() as u32;
        // A `return f()` in here must **not** replace the frame: the handler
        // has to still be on the stack when the callee runs, or
        // `try return f() catch` stops catching what `f` throws. The clause
        // bodies are deliberately outside this.
        self.f.try_depth += 1;
        let r = self.block(body);
        self.f.try_depth -= 1;
        r?;
        let body_end = self.f.label_here() as u32;

        // Normal completion. The `finally` is emitted *after* `body_end`, so
        // it sits outside every protected range this `try` installs — a
        // throw from inside a `finally` belongs to the enclosing `try`, not
        // to this one, which has already finished.
        self.emit_finally(finally, span)?;
        let mut over = vec![self.emit_jump(Op::JMP, 0, span)];

        // One handler per clause, all covering the body, pushed in source
        // order so `unwind`'s first match is the first clause that accepts
        // the value. Each clause's own body gets a protected range too, for
        // the `finally` handler below: a throw inside a clause still has to
        // run the `finally`, but must not reach a sibling clause.
        let mut installed: Vec<(u32, u32)> = Vec::new();
        let mut clause_ranges: Vec<(u32, u32)> = Vec::new();
        for c in catches {
            let target = self.f.label_here() as u32;
            self.f.enter_scope();
            self.f.declare(&c.var, err_reg);
            let r = self.block(&c.body);
            self.f.leave_scope();
            r?;
            let clause_end = self.f.label_here() as u32;
            clause_ranges.push((target, clause_end));
            self.emit_finally(finally, span)?;
            over.push(self.emit_jump(Op::JMP, 0, span));
            installed.push((target, self.type_desc(&c.ty)));
        }

        // The path where nothing handled the throw: run the `finally`, then
        // put the value back in flight for whoever is above us. Without
        // this a `finally` would be skipped by exactly the case it exists
        // for — the one where the body failed.
        let rethrow = if finally.is_some() {
            let target = self.f.label_here() as u32;
            self.emit_finally(finally, span)?;
            self.emit(Instruction::abc(Op::THROW, err8, 0, 0), span);
            Some(target)
        } else {
            None
        };

        for l in over {
            self.patch_here(l)?;
        }
        if finally.is_some() {
            self.f.finallys.pop();
        }

        for (target, ty) in installed {
            self.f.handlers.push(crate::chunk::Handler {
                pc_start: start,
                pc_end: body_end,
                target,
                err_reg: err8,
                catch_ty: ty,
            });
        }
        if let Some(target) = rethrow {
            let any = self.type_desc(&saule_ast::Type::Named("any".into()));
            // The body, and then each clause body — separate entries rather
            // than one wide range, because the ranges are not contiguous:
            // the `finally` copies emitted between them must not be covered.
            for (pc_start, pc_end) in std::iter::once((start, body_end)).chain(clause_ranges) {
                self.f.handlers.push(crate::chunk::Handler {
                    pc_start,
                    pc_end,
                    target,
                    err_reg: err8,
                    catch_ty: any,
                });
            }
        }
        Ok(())
    }

    /// Emit one copy of `finally`, if there is one.
    ///
    /// The block is popped from the pending stack while it is emitted, so a
    /// `return` inside a `finally` does not try to run that same `finally`
    /// again on its way out.
    fn emit_finally(
        &mut self,
        finally: Option<&[Spanned<Stmt>]>,
        _span: &std::ops::Range<usize>,
    ) -> Result<(), CompileError> {
        let Some(f) = finally else {
            return Ok(());
        };
        let saved = self.f.finallys.pop();
        self.f.enter_scope();
        let r = self.block(f);
        self.f.leave_scope();
        if let Some(p) = saved {
            self.f.finallys.push(p);
        }
        r
    }

    /// Emit the `finally` blocks a jump out of this point passes through,
    /// innermost first.
    ///
    /// `from_loop_depth` is how many loops the jump stays inside, so it says
    /// which `try` blocks are actually being left: `0` for a `return`, which
    /// leaves every one of them, and the current loop count for a `break` or
    /// `continue`, which leaves only those opened since that loop began. A
    /// `try` wrapped *around* the loop is not being left, and its `finally`
    /// must not run on a `break`.
    pub(crate) fn run_pending_finallys(
        &mut self,
        from_loop_depth: usize,
        _span: &std::ops::Range<usize>,
    ) -> Result<(), CompileError> {
        let mut pending: Vec<Vec<Spanned<Stmt>>> = Vec::new();
        for p in self.f.finallys.iter().rev() {
            if p.loops_at_entry >= from_loop_depth {
                pending.push(p.body.clone());
            }
        }
        for b in pending {
            // Taken off the stack for the duration, as `emit_finally` does
            // and for the same reason.
            let saved = std::mem::take(&mut self.f.finallys);
            self.f.enter_scope();
            let r = self.block(&b);
            self.f.leave_scope();
            self.f.finallys = saved;
            r?;
        }
        Ok(())
    }

    /// Intern a runtime type descriptor for a `catch` clause.
    fn type_desc(&mut self, ty: &saule_ast::Type) -> u32 {
        use crate::chunk::TypeDesc;
        use saule_ast::Type;
        let desc = match ty {
            Type::Named(n) => match n.as_str() {
                "any" => TypeDesc::Any,
                "nil" => TypeDesc::Nil,
                "boolean" => TypeDesc::Bool,
                "integer" => TypeDesc::Int,
                "float" => TypeDesc::Float,
                "string" => TypeDesc::Str,
                "table" => TypeDesc::Table,
                other => match self.layouts.get(other) {
                    Some(c) => TypeDesc::Class(c),
                    None => match self.layouts.enum_of(other) {
                        Some(e) => TypeDesc::Enum(e),
                        None => TypeDesc::Named(std::rc::Rc::from(other)),
                    },
                },
            },
            Type::Function { .. } => TypeDesc::Function,
            Type::Table { .. } => TypeDesc::Table,
            // Interned inner-first, so the index is already valid when the
            // wrapper is pushed.
            Type::Nullable(inner) => TypeDesc::Nullable(self.type_desc(inner)),
            // A nullable or generic `catch` type is not a runtime test the
            // tree-walker performs either; `any` is the honest answer.
            _ => TypeDesc::Any,
        };
        self.chunk.type_descs.push(desc);
        (self.chunk.type_descs.len() - 1) as u32
    }
}
