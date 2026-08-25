//! The `?.` family: safe member reads and safe method calls.
//!
//! Each one is a branch on nil around the access it guards. The awkward
//! case is `return x?.m()`, where the two arms return a different number of
//! values, so they have to return separately rather than join.

use std::ops::Range;

use saule_ast::{Expr, Spanned};
use saule_interpreter::Value;

use super::CompileError;
use super::super::ctx::Compiler;
use super::results::{Results, Want};
use crate::op::{Instruction, Op};

/// How a safe method call `obj?.m(...)` reaches its method.
#[derive(Clone, Copy)]
pub(crate) enum SafeCall {
    /// A proved class: one indexed load out of the vtable.
    Vtable(u16),
    /// No proved class: the method name, as a constant index, looked up by
    /// the tree-walker's own `dispatch_member_call_multi` (§8.5).
    Dynamic(u16),
}

impl Compiler<'_> {

    /// `obj?.name` — a member read that yields `nil` when the receiver is
    /// nil instead of faulting on it.
    pub(crate) fn safe_member_to(
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
        //
        // With no proved class the read becomes `GETFX`, which defers to
        // `read_member` — the same escape hatch §8.5 gives an ordinary
        // member read, and the reason a file handle, an enum variant and a
        // module-level variable all work here without the compiler learning
        // each one. A nullable receiver is if anything *more* likely to be
        // unproved, so refusing here was the odd one out.
        let access = self
            .class_of_nullable_expr(obj)
            .and_then(|class| self.member_access(class, name));
        // `GETFX` carries its key in `C`, an 8-bit constant index — same
        // window, and the same clean refusal past it, as the ordinary
        // dynamic read.
        let key = match access {
            Some(_) => None,
            None => {
                let k = self.constant(Value::Str(saule_interpreter::value::SauleStr::new(name.to_string())), span)?;
                let Ok(kc) = u8::try_from(k) else {
                    return Err(CompileError::unsupported(
                        "a dynamic member name past the 256-constant window",
                        span.clone(),
                    ));
                };
                Some(kc)
            }
        };

        let m = self.mark();
        let r = self.expr_tmp(obj)?;
        let (a, b) = (self.reg8(dst, span)?, self.reg8(r, span)?);
        // `JNOTNIL` skips the jump when the receiver is present, so the
        // jump is taken exactly on the nil path.
        self.emit(Instruction::abc(Op::JNOTNIL, b, 0, 0), span);
        let to_nil = self.emit_jump(Op::JMP, 0, span);
        match (access, key) {
            (Some(access), _) => self.emit(self.member_load(access, a, b), span),
            (None, Some(kc)) => self.emit(Instruction::abc(Op::GETFX, a, b, kc), span),
            (None, None) => unreachable!("a key is interned whenever the access is unresolved"),
        }
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
    pub(crate) fn safe_method_call_to(
        &mut self,
        e: &Spanned<Expr>,
        obj: &Spanned<Expr>,
        name: &str,
        args: &[&Spanned<Expr>],
        dst: u16,
        want: Want,
    ) -> Result<Results, CompileError> {
        let span = &e.span;

        let dispatch = self.safe_call_dispatch(obj, name, span)?;
        // `return x?.m()` — the two arms produce different numbers of
        // values and only the call arm's count is knowable at run time, so
        // there is no single register run for a `RET` to read. It used to
        // refuse for that reason. It does not need to: the arms never have
        // to *merge*. Each returns for itself, which also matches the
        // tree-walker exactly — a nil receiver yields one nil
        // (`values_of(Value::Nil)`) while a present one yields everything
        // `dispatch_member_call_multi` produced.
        if !matches!(want, Want::Fixed(_)) {
            return self.safe_method_call_returning(obj, args, dispatch, span);
        }
        let Want::Fixed(nret) = want else { unreachable!("checked above") };

        let m = self.mark();
        let base = self.alloc_n((args.len() as u16 + 1).max(want.slots()), span)?;
        self.expr_to(obj, base)?;
        let rb = self.reg8(base, span)?;
        self.emit(Instruction::abc(Op::JNOTNIL, rb, 0, 0), span);
        let to_nil = self.emit_jump(Op::JMP, 0, span);
        for (i, arg) in args.iter().enumerate() {
            self.expr_to(arg, base + 1 + i as u16)?;
        }
        self.emit_safe_call(dispatch, rb, args.len() as u8, want.c(), span);
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
            terminated: false,
        })
    }

    /// `return x?.m(args)` — the safe call as a **whole return**.
    ///
    /// Emitted only from a `return`, which is why it may write `RET` itself:
    /// the two arms need different ones, and forcing them to merge is what
    /// made this refuse before. The present-receiver arm asks for every
    /// result and returns the run `top` delimits; the nil arm returns a
    /// single nil, which is exactly `eval_values`' `values_of(Value::Nil)`.
    ///
    /// Reported back as [`Results::terminated`], so the caller emits nothing
    /// further — the same contract a tail call uses.
    fn safe_method_call_returning(
        &mut self,
        obj: &Spanned<Expr>,
        args: &[&Spanned<Expr>],
        dispatch: SafeCall,
        span: &Range<usize>,
    ) -> Result<Results, CompileError> {
        let m = self.mark();
        let base = self.alloc_n(args.len() as u16 + 1, span)?;
        self.expr_to(obj, base)?;
        let rb = self.reg8(base, span)?;
        // `JNOTNIL` skips the jump when the receiver is present, so the jump
        // is taken exactly on the nil path. The arguments are **inside** the
        // guard: the tree-walker returns before evaluating them, so
        // evaluating them here would run side effects it does not.
        self.emit(Instruction::abc(Op::JNOTNIL, rb, 0, 0), span);
        let to_nil = self.emit_jump(Op::JMP, 0, span);
        for (i, arg) in args.iter().enumerate() {
            self.expr_to(arg, base + 1 + i as u16)?;
        }
        self.emit_safe_call(dispatch, rb, args.len() as u8, 0, span);
        self.emit(Instruction::abc(Op::RET, rb, 0, 0), span);
        self.patch_here(to_nil)?;
        self.emit(Instruction::abc(Op::LOADNIL, rb, 0, 0), span);
        self.emit(Instruction::abc(Op::RET1, rb, 0, 0), span);
        self.free_to(m);
        Ok(Results {
            base,
            count: None,
            terminated: true,
        })
    }

    /// How `obj?.m(...)` reaches its method.
    ///
    /// The same choice `method_call_to` makes for an ordinary call, and it
    /// has to be made **before** anything is emitted: `Unsupported` falls
    /// back to the tree-walker, and half-written code cannot be taken back.
    fn safe_call_dispatch(
        &mut self,
        obj: &Spanned<Expr>,
        name: &str,
        span: &Range<usize>,
    ) -> Result<SafeCall, CompileError> {
        if let Some(class) = self.class_of_nullable_expr(obj)
            && let Some(&slot) = self.chunk.classes[class as usize].vindex.get(name)
        {
            return Ok(SafeCall::Vtable(slot));
        }
        // No proved class, or a method it does not declare: `CALLMX` defers
        // to `dispatch_member_call_multi` (§8.5), which is what makes a
        // stdlib instance, an enum variant or a file handle work here
        // without the compiler learning each one. A *nullable* receiver is
        // if anything more likely to be unproved than a plain one, so
        // refusing here was the odd one out — and it was `todo-app`'s first
        // refusal.
        let k = self.constant(Value::Str(saule_interpreter::value::SauleStr::new(name.to_string())), span)?;
        Ok(SafeCall::Dynamic(k))
    }

    /// Emit the call itself, whichever way it dispatches.
    ///
    /// `c` is the raw `C` operand — `nret + 1`, or 0 for every result.
    /// `CALLM` is the one form that cannot carry it, because `C` is its
    /// vtable slot, so a single-result vtable call takes it and everything
    /// else moves the slot into `EXTRAARG`.
    fn emit_safe_call(
        &mut self,
        dispatch: SafeCall,
        recv: u8,
        n_args: u8,
        c: u8,
        span: &Range<usize>,
    ) {
        match dispatch {
            SafeCall::Vtable(slot) if c == 2 => {
                self.emit(Instruction::abc(Op::CALLM, recv, n_args + 1, slot as u8), span);
            }
            SafeCall::Vtable(slot) => {
                self.emit(Instruction::abc(Op::CALLM_MR, recv, n_args + 1, c), span);
                self.emit(Instruction::ax_of(Op::EXTRAARG, slot as u32), span);
            }
            SafeCall::Dynamic(k) => {
                self.emit(Instruction::abc(Op::CALLMX, recv, n_args + 1, c), span);
                self.emit(Instruction::ax_of(Op::EXTRAARG, k as u32), span);
            }
        }
    }
}
