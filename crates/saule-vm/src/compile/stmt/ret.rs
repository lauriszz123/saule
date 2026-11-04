//! `return`.
//!
//! Multi-return and tail calls both live here: a `return f()` in tail
//! position replaces the frame instead of growing the stack (§6.4), and a
//! `return a, b` leaves a run of registers rather than one.

use saule_ast::{Expr, Spanned};

use super::super::CompileError;
use super::super::ctx::Compiler;
use super::super::expr::Want;
use crate::op::{Instruction, Op};

impl Compiler<'_> {
    pub(crate) fn ret(
        &mut self,
        values: &[Spanned<Expr>],
        span: &std::ops::Range<usize>,
    ) -> Result<(), CompileError> {
        match values.len() {
            0 => {
                self.emit(Instruction::abc(Op::RET0, 0, 0, 0), span);
                Ok(())
            }
            // `return f()` hands the callee's results **through**, however
            // many there are: `eval_expr_list` runs `eval_values` on the
            // last element, so the tree-walker's `return f()` returns every
            // value `f` produced, not just the first. Truncating here to one
            // was silent and invisible until something consumed more than
            // one — which nothing compiled did before parallel `local`.
            1 if matches!(values[0].value, Expr::Call { .. }) => {
                let m = self.mark();
                // A landing register for the shapes that yield exactly one
                // value (a constructor, `self.super()`); a genuine call
                // leaves its results in its own window and says so.
                let dst = self.alloc(span)?;
                // `return f(args)` is a **tail call** — unless one of two
                // things forbids it, both properties of the enclosing
                // function rather than of the call, and both about matching
                // the tree-walker rather than about the VM:
                //
                // * **Inside a `try` body.** `exec_try` forces
                //   `Flow::TailCall` into a real call so the handler is
                //   still on the stack when the callee runs; replacing the
                //   frame would make `try return f() catch` stop catching
                //   what `f` throws.
                // * **The module body.** `run_in` makes a module-level tail
                //   call for real — a module body is not a function, so
                //   there is no frame to replace — and the VM's outermost
                //   frame is the one `run_chunk_entry` returns through.
                //
                // Even then it is a *request*: only the shapes that can
                // replace a frame honour it, and the rest hand back an
                // ordinary result run to return.
                let tail_ok = self.f.try_depth == 0 && !self.f.is_module_body;
                let want = if tail_ok { Want::Tail } else { Want::All };
                let r = self.expr_results(&values[0], dst, want)?;
                if r.terminated {
                    // The frame is gone. Emitting a `RET` after this would
                    // be unreachable code the verifier would then have to
                    // accept.
                    self.free_to(m);
                    return Ok(());
                }
                let a = self.reg8(r.base, span)?;
                match r.count {
                    Some(1) => self.emit(Instruction::abc(Op::RET1, a, 0, 0), span),
                    // `B = 0`: the run ends at the frame's `top`, which the
                    // call that just returned set (§6.3).
                    None => self.emit(Instruction::abc(Op::RET, a, 0, 0), span),
                    Some(n) => self.emit(Instruction::abc(Op::RET, a, n + 1, 0), span),
                }
                self.free_to(m);
                Ok(())
            }
            // The overwhelmingly common shape, and why `RET1` is its own
            // opcode rather than `RET` with a count.
            1 => {
                // **`RET1` deliberately does not read a local in place**,
                // and the copy this leaves in is load-bearing.
                //
                // `pop_frame` calls `close_upvalues(frame.base)` *before* it
                // moves the results out, and closing an upvalue does
                // `mem::replace(slot, Value::Nil)` — so a captured register
                // read by `RET1` reads the nil that closing left behind.
                // Tried, and it turned `fn run() local n = 0; local bump =
                // fn() n = n + 1 end; bump(); return n end` from `3` into
                // `nil`. Caught by
                // `a_closure_writes_through_to_its_captured_variable`.
                //
                // Not fixable at the call site: whether `n` is captured is
                // not settled when the `return` is compiled, because a
                // lambda *below* it can capture it. The `MOVE` reads the
                // register while the frame is still whole, which is the
                // whole reason it is correct.
                let m = self.mark();
                let r = self.expr_tmp(&values[0])?;
                let a = self.reg8(r, span)?;
                self.emit(Instruction::abc(Op::RET1, a, 0, 0), span);
                self.free_to(m);
                Ok(())
            }
            n => {
                // Multi-return wants a contiguous range, which is what lets
                // the caller take the values without allocating (§6.3).
                if n > u8::MAX as usize - 1 {
                    return Err(CompileError::unsupported(
                        "returning over 254 values",
                        span.clone(),
                    ));
                }
                // `return a, f()` returns `a` followed by **all** of `f`'s
                // results — `eval_expr_list` expands the last element and
                // only the last — and how many that is is a run-time fact.
                // The range `RET` reads has to be contiguous, so `f`'s
                // window must begin exactly where the fixed values end.
                //
                // It does, and for free: the register allocator is a bump
                // pointer, so after the first `n - 1` values are in place
                // `free` sits precisely at the landing register. Reserving
                // that register and releasing it again is what makes the
                // frame big enough for a single-valued last expression
                // *and* leaves the next allocation — the call's window —
                // landing on it. This used to refuse for want of noticing
                // that.
                let m = self.mark();
                let base = self.alloc_n(n as u16 - 1, span)?;
                for (i, v) in values.iter().take(n - 1).enumerate() {
                    self.expr_to(v, base + i as u16)?;
                }
                let landing = self.mark();
                let dst = self.alloc(span)?;
                debug_assert_eq!(dst, base + n as u16 - 1);
                self.free_to(landing);

                let r = self.expr_results(&values[n - 1], dst, Want::All)?;
                debug_assert_eq!(r.base, dst, "the last value did not land contiguously");
                let a = self.reg8(base, span)?;
                match r.count {
                    // `B = 0`: the run ends at `top`, which the call set.
                    None => self.emit(Instruction::abc(Op::RET, a, 0, 0), span),
                    Some(k) => {
                        let total = n as u16 - 1 + k as u16;
                        let Ok(total) = u8::try_from(total) else {
                            return Err(CompileError::unsupported(
                                "returning over 254 values",
                                span.clone(),
                            ));
                        };
                        self.emit(Instruction::abc(Op::RET, a, total + 1, 0), span);
                    }
                }
                self.free_to(m);
                Ok(())
            }
        }
    }
}

impl Compiler<'_> {
    /// `return` out of a `try` that has a `finally`.
    ///
    /// The ordinary [`Compiler::ret`] cannot be used behind
    /// `run_pending_finallys`, because that would evaluate the returned
    /// expression *after* the cleanup — so
    ///
    /// ```text
    /// fn readPort() -> string
    ///     local f: File = Io.open(path, IoMode.Read)!
    ///     try
    ///         return f.read("a") ?? ""
    ///     finally
    ///         f.close()
    ///     end
    /// end
    /// ```
    ///
    /// read from a file the `finally` had already closed, and
    /// `try return n finally n = 99 end` returned `99`. Every language with
    /// `finally` settles the value first; this does too.
    ///
    /// So: evaluate into a block of registers, run the cleanup **without
    /// freeing them** — the allocator is a bump pointer, so the cleanup's
    /// own temporaries land above the block and cannot touch it — then
    /// `RET` the block.
    ///
    /// The one thing this gives up is `RET A 0`, the "however many the
    /// callee produced" form, which only works when the `RET` is the
    /// instruction right after the call. A fixed count is needed instead,
    /// and `FuncCtx::ret_arity` is it: the enclosing function's declared
    /// return type already says how many values a `return` here produces,
    /// so nothing is truncated that the signature did not already truncate.
    pub(crate) fn ret_through_finallys(
        &mut self,
        values: &[Spanned<Expr>],
        span: &std::ops::Range<usize>,
    ) -> Result<(), CompileError> {
        if values.is_empty() {
            self.run_pending_finallys(0, span)?;
            self.emit(Instruction::abc(Op::RET0, 0, 0, 0), span);
            return Ok(());
        }

        let m = self.mark();
        // `return f()` in a function declared `-> (a, b)` spreads `f`'s
        // results across the whole block; every other shape is one value
        // per expression, padded to the declared arity with nil.
        let spread = values.len() == 1 && matches!(values[0].value, Expr::Call { .. });
        let n = if spread {
            self.f.ret_arity.max(1) as u16
        } else {
            values.len() as u16
        };

        let base = self.alloc_n(n, span)?;
        if spread {
            let Ok(want) = u8::try_from(n) else {
                return Err(CompileError::unsupported(
                    "returning over 254 values",
                    span.clone(),
                ));
            };
            let r = self.expr_results(&values[0], base, Want::Fixed(want))?;
            debug_assert!(!r.terminated, "`Want::Fixed` never replaces the frame");
            debug_assert_eq!(r.base, base, "`Want::Fixed` lands where it was asked");
        } else {
            for (i, v) in values.iter().enumerate() {
                self.expr_to(v, base + i as u16)?;
            }
        }

        // The block stays allocated across this, which is the whole point.
        self.run_pending_finallys(0, span)?;

        let a = self.reg8(base, span)?;
        if n == 1 {
            self.emit(Instruction::abc(Op::RET1, a, 0, 0), span);
        } else {
            let Ok(count) = u8::try_from(n) else {
                return Err(CompileError::unsupported(
                    "returning over 254 values",
                    span.clone(),
                ));
            };
            self.emit(Instruction::abc(Op::RET, a, count + 1, 0), span);
        }
        self.free_to(m);
        Ok(())
    }
}
