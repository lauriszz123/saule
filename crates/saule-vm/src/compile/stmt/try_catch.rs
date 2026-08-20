//! `try`/`catch`, and the type descriptors a `catch` clause tests against.
//!
//! A handler is a range of instructions plus a type to match, recorded on
//! the proto rather than emitted inline (§12.1), so the non-throwing path
//! costs nothing.

use saule_ast::{Spanned, Stmt};

use super::super::CompileError;
use super::super::ctx::Compiler;
use crate::op::Op;

impl Compiler<'_> {

    /// `try … catch e: T … end`.
    ///
    /// Entering the `try` emits **no instructions at all** (§12.1): the
    /// protected range is recorded out of band in the proto's handler table,
    /// and only a `throw` ever consults it. The happy path costs nothing.
    pub(crate) fn try_catch(
        &mut self,
        body: &[Spanned<Stmt>],
        catch_var: &str,
        catch_ty: &saule_ast::Type,
        catch_body: &[Spanned<Stmt>],
        span: &std::ops::Range<usize>,
    ) -> Result<(), CompileError> {
        // The register the caught value lands in has to be reserved for the
        // whole protected range, not just the handler: unwinding writes it
        // before any of the handler's own code runs.
        let err_reg = self.alloc(span)?;
        let ty = self.type_desc(catch_ty);

        let start = self.f.label_here() as u32;
        // A `return f()` in here must **not** replace the frame: the handler
        // has to still be on the stack when the callee runs, or
        // `try return f() catch` stops catching what `f` throws. `exec_try`
        // forces the tree-walker's `Flow::TailCall` into a real call for the
        // same reason, and the two engines must agree — the depth at which a
        // program dies is observable. The `catch` body is deliberately
        // outside this, exactly as it is there.
        self.f.try_depth += 1;
        let r = self.block(body);
        self.f.try_depth -= 1;
        r?;
        let end = self.f.label_here() as u32;
        let over = self.emit_jump(Op::JMP, 0, span);

        let target = self.f.label_here() as u32;
        self.f.enter_scope();
        self.f.declare(catch_var, err_reg);
        self.block(catch_body)?;
        self.f.leave_scope();
        self.patch_here(over)?;

        let err8 = self.reg8(err_reg, span)?;
        self.f.handlers.push(crate::chunk::Handler {
            pc_start: start,
            pc_end: end,
            target,
            err_reg: err8,
            catch_ty: ty,
        });
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
