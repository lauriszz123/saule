//! The `for` family: numeric `for`, and `for … in` over each kind of source.
//!
//! `for … in` has three shapes depending on what the typechecker proved
//! about the source — a known iterable, a closure driver (§15.8), and a
//! fully dynamic source that has to be asked at run time.

use saule_ast::{Expr, Spanned, Stmt};

use super::super::CompileError;
use super::super::ctx::{Compiler, Num};
use crate::op::{Instruction, Op};

impl Compiler<'_> {

    pub(crate) fn for_numeric(
        &mut self,
        var: &str,
        from: &Spanned<Expr>,
        to: &Spanned<Expr>,
        step: Option<&Spanned<Expr>>,
        body: &[Spanned<Stmt>],
        span: &std::ops::Range<usize>,
    ) -> Result<(), CompileError> {
        // The loop wants counter, limit, step and the user variable in four
        // consecutive registers (§11.1), so they are allocated as a block
        // and live for the whole loop.
        let kind = self
            .num_of_node(from)
            .or_else(|| self.num_of_node(to))
            .ok_or_else(|| {
                CompileError::unsupported(
                    "a numeric `for` whose bounds have no proved numeric type",
                    span.clone(),
                )
            })?;

        self.f.enter_scope();
        let base = self.alloc_n(4, span)?;

        self.expr_to(from, base)?;
        self.expr_to(to, base + 1)?;
        match step {
            Some(s) => self.expr_to(s, base + 2)?,
            None => {
                let a = self.reg8(base + 2, span)?;
                // The default step matches the bounds' type, because
                // `FORPREP` validates that all three agree — mixing them is
                // a `TypeError` in the tree-walker and must stay one here.
                let ins = match kind {
                    Num::Int => Instruction::asbx(Op::LOADI, a, 1),
                    Num::Float => {
                        let k = self.constant(saule_interpreter::Value::Float(1.0), span)?;
                        Instruction::abx(Op::LOADK, a, k)
                    }
                };
                self.emit(ins, span);
            }
        }

        let a = self.reg8(base, span)?;
        let (prep, loop_op) = match kind {
            Num::Int => (Op::FORPREP_I, Op::FORLOOP_I),
            Num::Float => (Op::FORPREP_F, Op::FORLOOP_F),
        };
        let exit = self.emit_jump(prep, a, span);

        let body_start = self.f.label_here();
        // The user-visible loop variable is the fourth control register;
        // `FORPREP`/`FORLOOP` write it, the body reads it like any local.
        self.f.declare(var, base + 3);
        self.loops.push(Default::default());
        self.block(body)?;
        let l = self.loops.pop().expect("pushed above");
        // `continue` in a numeric `for` must still *step* the loop, so it
        // targets the `FORLOOP` about to be emitted. Sending it to the body
        // top instead would spin forever.
        let step_at = self.f.label_here();
        for c in l.continues {
            self.patch_to(c, step_at)?;
        }
        self.emit_jump_back(loop_op, a, body_start, span)?;
        self.patch_here(exit)?;
        for b in l.breaks {
            self.patch_here(b)?;
        }

        self.f.leave_scope();
        Ok(())
    }

    /// `for k, v in t do … end`.
    ///
    /// Control state occupies `R[A]..R[A+2]` and the loop variables
    /// `R[A+3]`/`R[A+4]`, so five consecutive registers (§15.8). With one
    /// variable the *value* is bound, matching the tree-walker.
    pub(crate) fn for_in(
        &mut self,
        vars: &[(String, Option<saule_ast::Type>)],
        iter: &Spanned<Expr>,
        body: &[Spanned<Stmt>],
        span: &std::ops::Range<usize>,
    ) -> Result<(), CompileError> {
        if vars.is_empty() || vars.len() > 2 {
            // `saule-semantic` already reports this; refusing keeps the
            // compiler from emitting something shaped wrongly.
            return Err(CompileError::unsupported(
                "a `for … in` with other than one or two variables",
                span.clone(),
            ));
        }

        // Three sources, and the front end has to have told us which:
        //
        //   * a **table** — the snapshot path below;
        //   * a **function** — the §15.8 closure driver;
        //   * an **instance** — `iter()` returns the closure, then as above.
        //
        // Anything unproved is refused. The three lower to completely
        // different code, and guessing wrong would iterate a function as a
        // table (or call a table) rather than fail cleanly.
        // A variadic parameter is always bound to a table by `VARARG`, but
        // the front end types it as the *element* type — `...values: integer`
        // makes `values` an `integer` in the `TypeTable` — so the type alone
        // never proves it. The compiler knows, because it emitted the
        // `VARARG` itself.
        let is_varargs = matches!(&iter.value, Expr::Ident(n)
            if self.f.variadic_param.as_deref() == Some(n.as_str()));

        match self.types.get(&iter.id) {
            _ if is_varargs => {}
            Some(saule_ast::Type::Table { .. }) => {}
            Some(saule_ast::Type::Function { .. }) => {
                return self.for_in_driver(vars, iter, body, None, span);
            }
            // A class receiver: `iter()` yields the driver. Its vtable slot
            // has to be resolvable, which needs the class proved — an
            // interface-typed receiver would want `CALLIF` and is not
            // handled here.
            _ => {
                if let Some(class) = self.class_of_expr(iter)
                    && let Some(&slot) = self.chunk.classes[class as usize].vindex.get("iter")
                {
                    return self.for_in_driver(vars, iter, body, Some(slot), span);
                }
                // Nothing proved: decide at run time, the way the
                // tree-walker always has.
                return self.for_in_dynamic(vars, iter, body, span);
            }
        }

        self.f.enter_scope();
        let base = self.alloc_n(5, span)?;
        self.expr_to(iter, base)?;

        let a = self.reg8(base, span)?;
        let prep = self.emit_jump_abx(Op::ITERPREP, a, span)?;

        let top = self.f.label_here();
        // One variable binds the value; two bind key then value.
        if vars.len() == 1 {
            self.f.declare(&vars[0].0, base + 4);
        } else {
            self.f.declare(&vars[0].0, base + 3);
            self.f.declare(&vars[1].0, base + 4);
        }

        // `ITERNEXT` runs *before* the body each pass, so the loop is
        // entered through it rather than falling into the body.
        let enter = self.emit_jump(Op::JMP, 0, span);
        let body_start = self.f.label_here();
        self.loops.push(Default::default());
        self.block(body)?;
        let l = self.loops.pop().expect("pushed above");
        let step_at = self.f.label_here();
        for c in l.continues {
            self.patch_to(c, step_at)?;
        }
        self.patch_to(enter, step_at)?;
        self.emit_jump_back(Op::ITERNEXT, a, body_start, span)?;
        self.patch_here(prep)?;
        for b in l.breaks {
            self.patch_here(b)?;
        }
        let _ = top;
        self.f.leave_scope();
        Ok(())
    }

    /// `for … in` over a **closure driver** (§15.8).
    ///
    /// The driver is called with no arguments and yields the next iteration's
    /// values; the loop stops when the first of them is `nil` — Lua's
    /// nil-terminator, and what `exec_for_in` does. `iter_slot` is `Some`
    /// when the source is an instance, whose `iter()` produces the driver.
    ///
    /// Lowered to an ordinary `CALL` in a `while` shape rather than taught to
    /// `ITERNEXT`, for one reason: `CALL` already dispatches on what it finds
    /// — a bytecode closure, a native, a native closure — so the driver can
    /// be any of them without a single new opcode or VM path. Making
    /// `ITERNEXT` call would have meant pushing a frame from inside an opcode
    /// and resuming into it, which is the dispatch loop's hardest corner for
    /// no gain.
    ///
    /// **The result count is fixed, not variadic.** `C = nvars + 1` asks for
    /// exactly as many values as there are loop variables, so `pop_frame`
    /// pads the short cases with `nil` and drops the surplus — which is
    /// precisely the tree-walker's "extras → nil, surplus dropped". Asking
    /// for *all* results would leave the callee register holding the driver
    /// itself when a step returned nothing, and the nil test would then read
    /// a function and loop forever.
    fn for_in_driver(
        &mut self,
        vars: &[(String, Option<saule_ast::Type>)],
        iter: &Spanned<Expr>,
        body: &[Spanned<Stmt>],
        iter_slot: Option<u16>,
        span: &std::ops::Range<usize>,
    ) -> Result<(), CompileError> {
        self.f.enter_scope();
        // `d` holds the driver for the whole loop; the call window starts at
        // `c` because `CALL` overwrites its callee register with the results.
        let nvars = vars.len() as u16;
        let base = self.alloc_n(1 + nvars, span)?;
        let (d, c) = (base, base + 1);

        self.expr_to(iter, d)?;
        if let Some(slot) = iter_slot {
            // `CALLM` takes the receiver in `A` and writes its result there,
            // so the instance is replaced by the driver it returned.
            let da = self.reg8(d, span)?;
            self.emit(Instruction::abc(Op::CALLM, da, 1, slot as u8), span);
        }

        let top = self.f.label_here();
        let (da, ca) = (self.reg8(d, span)?, self.reg8(c, span)?);
        self.emit(Instruction::abc(Op::MOVE, ca, da, 0), span);
        self.emit(Instruction::abc(Op::CALL, ca, 1, nvars as u8 + 1), span);
        // Skips the following jump when the step produced a value, so the
        // jump is taken exactly when the driver said stop.
        self.emit(Instruction::abc(Op::JNOTNIL, ca, 0, 0), span);
        let exit = self.emit_jump(Op::JMP, 0, span);

        // The results *are* the loop variables — no moves.
        for (i, (name, _)) in vars.iter().enumerate() {
            self.f.declare(name, c + i as u16);
        }

        self.loops.push(Default::default());
        self.block(body)?;
        let l = self.loops.pop().expect("pushed above");
        // `continue` re-enters at the call: the next step is what advances
        // this loop, there is no separate increment.
        for k in l.continues {
            self.patch_to(k, top)?;
        }
        self.emit_jump_back(Op::JMP, 0, top, span)?;
        self.patch_here(exit)?;
        for b in l.breaks {
            self.patch_here(b)?;
        }
        self.f.leave_scope();
        Ok(())
    }

    /// `for … in` over a source the front end could **not** prove.
    ///
    /// The two proved paths above lower to completely different code, and
    /// the honest reason this one is harder is that the difference is
    /// *semantic*, not just representational: a driver stops on a nil, and
    /// a table snapshot has no terminator at all. Saule's `t[i] = nil`
    /// stores a nil rather than deleting the key, so a table can hold one,
    /// and a single-variable loop binds the **value** — meaning a table
    /// normalised into a nil-terminated driver would stop early here and
    /// run to completion under the tree-walker. Silent divergence, in
    /// exactly the shape `SAULE_DIFF=1` only catches by luck.
    ///
    /// So this does not normalise. It emits **both** steps, once, behind a
    /// mode flag that `ITERPREPX` sets from the runtime value — which is
    /// precisely the dispatch `exec_for_in` performs with its `match`. The
    /// loop body is emitted once and reads its variables from fixed
    /// registers, so neither mode pays for the other beyond a single
    /// predictable branch per step.
    ///
    /// The driver's call window is *placed* on the loop-variable registers
    /// rather than moved into them: `R[A+4]` for one variable, `R[A+3]` for
    /// two. `CALL` writes its results exactly where `ITERNEXT` writes its
    /// key and value, so the merge costs no `MOVE`s and the nil test lands
    /// on the first returned value, which is what the tree-walker tests.
    fn for_in_dynamic(
        &mut self,
        vars: &[(String, Option<saule_ast::Type>)],
        iter: &Spanned<Expr>,
        body: &[Spanned<Stmt>],
        span: &std::ops::Range<usize>,
    ) -> Result<(), CompileError> {
        self.f.enter_scope();
        let nvars = vars.len() as u16;
        // The same five-register control block as the proved table path, so
        // `ITERNEXT` drives it unchanged.
        let base = self.alloc_n(5, span)?;
        self.expr_to(iter, base)?;

        let a = self.reg8(base, span)?;
        let prep = self.emit_jump_abx(Op::ITERPREPX, a, span)?;

        let win = if nvars == 1 { base + 4 } else { base + 3 };
        if nvars == 1 {
            self.f.declare(&vars[0].0, base + 4);
        } else {
            self.f.declare(&vars[0].0, base + 3);
            self.f.declare(&vars[1].0, base + 4);
        }

        // Entered through the step, like the table path: the step is what
        // binds the variables, so it has to run before the first body pass.
        let enter = self.emit_jump(Op::JMP, 0, span);
        let body_start = self.f.label_here();
        self.loops.push(Default::default());
        self.block(body)?;
        let l = self.loops.pop().expect("pushed above");

        let step_at = self.f.label_here();
        for c in l.continues {
            self.patch_to(c, step_at)?;
        }
        self.patch_to(enter, step_at)?;

        // `TEST` skips the next instruction when the flag is falsy, so the
        // table mode falls straight through to `ITERNEXT` and only the
        // driver mode pays for the extra jump.
        let m = self.reg8(base + 2, span)?;
        self.emit(Instruction::abc(Op::TEST, m, 0, 1), span);
        let to_drv = self.emit_jump(Op::JMP, 0, span);
        self.emit_jump_back(Op::ITERNEXT, a, body_start, span)?;
        let table_done = self.emit_jump(Op::JMP, 0, span);

        self.patch_here(to_drv)?;
        let w = self.reg8(win, span)?;
        // `CALL` overwrites its callee register with the results, so the
        // driver is copied out of `R[A]` afresh each step.
        self.emit(Instruction::abc(Op::MOVE, w, a, 0), span);
        self.emit(Instruction::abc(Op::CALL, w, 1, nvars as u8 + 1), span);
        // Skips the following jump when the step produced a value, so the
        // jump is taken exactly when the driver said stop.
        self.emit(Instruction::abc(Op::JNOTNIL, w, 0, 0), span);
        let drv_done = self.emit_jump(Op::JMP, 0, span);
        self.emit_jump_back(Op::JMP, 0, body_start, span)?;

        // Every exit — empty table, table exhausted, driver stopped —
        // lands here.
        self.patch_here(prep)?;
        self.patch_here(table_done)?;
        self.patch_here(drv_done)?;
        for b in l.breaks {
            self.patch_here(b)?;
        }
        self.f.leave_scope();
        Ok(())
    }

}
