//! Binding and assignment: `local`, `=`, parallel binding, and `+=`.
//!
//! The rule that shapes all of it is that a parallel assignment evaluates
//! its **whole** right-hand side before writing any target — that is what
//! makes `a, b = b, a` a swap — while a single assignment is free to
//! evaluate straight into the target's own register.

use saule_ast::{Expr, Spanned};
use saule_semantic::Binding;

use super::super::CompileError;
use super::Rhs;
use super::super::ctx::Compiler;
use super::super::expr::Want;
use crate::op::{Instruction, Op};

impl Compiler<'_> {

    pub(crate) fn local(
        &mut self,
        name: &str,
        value: Option<&Spanned<Expr>>,
        ty: Option<&saule_ast::Type>,
        span: &std::ops::Range<usize>,
    ) -> Result<(), CompileError> {
        // The rule mirrors the resolver's exactly (0.6): a declaration at
        // the *top* of the module body is a module slot — visible file-wide
        // and to importers — while one inside any block is an ordinary local.
        // The two have to agree, because reads are classified by the
        // resolver and written by the compiler.
        if self.at_module_top() {
            let slot = match self.bindings.module_slots.iter().position(|n| n.as_ref() == name) {
                Some(i) => i as u16,
                None => {
                    return Err(CompileError::unsupported(
                        "a top-level binding the resolver did not record",
                        span.clone(),
                    ));
                }
            };
            let m = self.mark();
            let r = match value {
                Some(v) => self.expr_tmp(v)?,
                None => {
                    let r = self.alloc(span)?;
                    let a = self.reg8(r, span)?;
                    self.emit(Instruction::abc(Op::LOADNIL, a, 0, 0), span);
                    r
                }
            };
            // A module variable is one of `coerce.rs`'s sites.
            self.coerce_to_declared(r, ty, span)?;
            let a = self.reg8(r, span)?;
            let g = self.mod_slot(slot, span)?;
            self.emit(Instruction::abx(Op::SETMOD, a, g), span);
            self.free_to(m);
            return Ok(());
        }

        // A frame local: allocate its register first, then evaluate straight
        // into it. No temporary, no move.
        let reg = self.alloc(span)?;
        // `local go = fn(k) … go(…) … end`. The tree-walker's `Stmt::Local`
        // arm tests for exactly this shape too, and for the same reason: the
        // recursion must not become a capture. See `Op::SELFFUNC`.
        if matches!(value.map(|v| &v.value), Some(Expr::Lambda { .. })) {
            self.binding_lambda_to = Some(std::rc::Rc::from(name));
        }
        match value {
            Some(v) => self.expr_to(v, reg)?,
            None => {
                let a = self.reg8(reg, span)?;
                self.emit(Instruction::abc(Op::LOADNIL, a, 0, 0), span);
            }
        }
        // An annotated `local` is the other of `coerce.rs`'s sites. Before
        // the name is declared, so the conversion cannot see it.
        self.coerce_to_declared(reg, ty, span)?;
        // Declared *after* the initializer, so `local x = x` reads the outer
        // `x` — the same order the resolver uses.
        self.f.declare(name, reg);
        Ok(())
    }

    /// `local a, b = f()` — parallel binding (§6.3).
    ///
    /// The whole right-hand side is evaluated into a **contiguous** run
    /// before any name is bound. Contiguity is what lets a trailing call
    /// write its results straight into the run instead of through
    /// temporaries, and binding afterwards is what makes `local a, b = b, a`
    /// read the *outer* `a` and `b` — the same order plain `local` uses.
    pub(crate) fn local_multi(
        &mut self,
        names: &[(String, std::ops::Range<usize>, Option<saule_ast::Type>)],
        values: &[Spanned<Expr>],
        span: &std::ops::Range<usize>,
    ) -> Result<(), CompileError> {
        let Ok(n) = u8::try_from(names.len()) else {
            return Err(CompileError::unsupported(
                "a parallel `local` binding over 255 names",
                span.clone(),
            ));
        };
        if n == 0 {
            return Ok(());
        }

        // A module-level `local` is a module *slot*, not a frame register
        // (0.6), so here the run is temporaries that `SETMOD` publishes.
        // Inside a function the run **is** the locals, and the names are
        // declared onto it with no moves at all.
        if self.at_module_top() {
            let mut slots = Vec::with_capacity(names.len());
            for (name, _, _) in names {
                let Some(i) = self
                    .bindings
                    .module_slots
                    .iter()
                    .position(|s| s.as_ref() == name.as_str())
                else {
                    return Err(CompileError::unsupported(
                        "a top-level binding the resolver did not record",
                        span.clone(),
                    ));
                };
                slots.push(i as u16);
            }
            let m = self.mark();
            let base = self.alloc_n(n as u16, span)?;
            self.expr_list_to(values, base, n, span)?;
            for (i, slot) in slots.into_iter().enumerate() {
                let a = self.reg8(base + i as u16, span)?;
                let g = self.mod_slot(slot, span)?;
                self.emit(Instruction::abx(Op::SETMOD, a, g), span);
            }
            self.free_to(m);
            return Ok(());
        }

        let base = self.alloc_n(n as u16, span)?;
        self.expr_list_to(values, base, n, span)?;
        for (i, (name, _, _)) in names.iter().enumerate() {
            self.f.declare(name, base + i as u16);
        }
        Ok(())
    }

    /// `a, b = b, a` — parallel assignment.
    ///
    /// The right-hand side is evaluated **in full** before any target is
    /// written, which is what makes the swap a swap. That is also why the
    /// targets are written from registers rather than from expressions, and
    /// why [`Rhs`] exists.
    pub(crate) fn assign_multi(
        &mut self,
        targets: &[Spanned<Expr>],
        values: &[Spanned<Expr>],
        span: &std::ops::Range<usize>,
    ) -> Result<(), CompileError> {
        let Ok(n) = u8::try_from(targets.len()) else {
            return Err(CompileError::unsupported(
                "a parallel assignment to over 255 targets",
                span.clone(),
            ));
        };
        if n == 0 {
            return Ok(());
        }
        let m = self.mark();
        let base = self.alloc_n(n as u16, span)?;
        self.expr_list_to(values, base, n, span)?;
        for (i, target) in targets.iter().enumerate() {
            self.assign(target, Rhs::Reg(base + i as u16))?;
        }
        self.free_to(m);
        Ok(())
    }

    /// Fill `dst .. dst + n` from an expression list.
    ///
    /// Mirrors `eval_expr_list`: every expression but the last contributes
    /// exactly one value, and **only the last** expands into however many a
    /// call returned. A surplus expression is still evaluated — dropping its
    /// value is not the same as not running it — and a surplus target is
    /// left nil, both of which the tree-walker does by construction.
    fn expr_list_to(
        &mut self,
        values: &[Spanned<Expr>],
        dst: u16,
        n: u8,
        span: &std::ops::Range<usize>,
    ) -> Result<(), CompileError> {
        let n = n as u16;
        if values.is_empty() {
            for i in 0..n {
                let a = self.reg8(dst + i, span)?;
                self.emit(Instruction::abc(Op::LOADNIL, a, 0, 0), span);
            }
            return Ok(());
        }
        let last = values.len() - 1;
        for (i, v) in values.iter().enumerate() {
            let filled = i as u16;
            if filled >= n {
                // Past the last target: evaluated for its side effects and
                // the value dropped, which is what `eval_expr_list` does
                // with the surplus.
                let m = self.mark();
                let _ = self.expr_tmp(v)?;
                self.free_to(m);
            } else if i < last {
                self.expr_to(v, dst + filled)?;
            } else {
                // The count is known here, so no `top` is involved: `C`
                // asks for exactly the registers still to fill, and the VM
                // pads a short callee with nil.
                self.expr_results(v, dst + filled, Want::Fixed((n - filled) as u8))?;
            }
        }
        Ok(())
    }

    pub(crate) fn assign(
        &mut self,
        target: &Spanned<Expr>,
        value: Rhs<'_>,
    ) -> Result<(), CompileError> {
        let span = &target.span;

        // `t[k] = v`. The receiver and key are evaluated once, in source
        // order, before the value — the order the tree-walker uses.
        if let Expr::Index { obj, index } = &target.value {
            // An instance target is its `OpNewIndex` overload, resolved
            // here — `SETIDX` is a table write, and a run-time lookup could
            // not find a bytecode method anyway (§8.7).
            if let Some(class) = self.class_of_expr(obj) {
                let contract = saule_ast::ops::OP_NEW_INDEX;
                let Some(&slot) = self.chunk.classes[class as usize]
                    .vindex
                    .get(contract.method)
                else {
                    return Err(CompileError::unsupported(
                        "an index assignment to a class with no `OpNewIndex` overload",
                        span.clone(),
                    ));
                };
                let m = self.mark();
                let base = self.alloc_n(3, span)?;
                self.expr_to(obj, base)?;
                self.expr_to(index, base + 1)?;
                self.rhs_to(value, base + 2, span)?;
                let a = self.reg8(base, span)?;
                self.emit(Instruction::abc(Op::CALLM, a, 3, slot as u8), span);
                self.free_to(m);
                return Ok(());
            }

            let m = self.mark();
            let o = self.expr_tmp(obj)?;
            let k = self.expr_tmp(index)?;
            let v = self.rhs_tmp(value)?;
            let (a, b, c) = (
                self.reg8(o, span)?,
                self.reg8(k, span)?,
                self.reg8(v, span)?,
            );
            self.emit(Instruction::abc(Op::SETIDX, a, b, c), span);
            self.free_to(m);
            return Ok(());
        }

        // `self.field = v`, `p.health = v`, `Counter.total = v`.
        if let Expr::Member { obj, name } = &target.value {
            if let Some(class) = self.class_named_by(obj)
                && let Some(&s) = self.chunk.classes[class as usize].sindex.get(name.as_str())
            {
                let m = self.mark();
                let r = self.rhs_tmp(value)?;
                let a = self.reg8(r, span)?;
                // `s.class`, not `class`: `Child.counter = 1` writes the
                // slot `Parent` declares, so a bare-name read from a
                // sibling sees it (`declaring_static_field`).
                self.emit(
                    Instruction::abc(Op::SETSTAT, a, s.class as u8, s.slot as u8),
                    span,
                );
                self.free_to(m);
                return Ok(());
            }
            // `t.name = v` on a table is `t["name"] = v`, the write half of
            // the Lua-style sugar `member_to` reads.
            if matches!(self.types.get(&obj.id), Some(saule_ast::Type::Table { .. })) {
                let m = self.mark();
                let o = self.expr_tmp(obj)?;
                let key = self.constant(
                    saule_interpreter::Value::Str(std::rc::Rc::new(name.clone())),
                    span,
                )?;
                let v = self.rhs_tmp(value)?;
                self.map_key_write(o, key, v, span)?;
                self.free_to(m);
                return Ok(());
            }

            // `self.count = …` inside a `static fn`: `self` is the class, so
            // this writes a static, not an instance field. The read side is
            // in `member_to`.
            if matches!(obj.value, Expr::Self_)
                && !self.f.in_method
                && let Some(class) = self.f.current_class
                && let Some(&s) = self.chunk.classes[class as usize].sindex.get(name.as_str())
            {
                let m = self.mark();
                let r = self.rhs_tmp(value)?;
                let a = self.reg8(r, span)?;
                self.emit(
                    Instruction::abc(Op::SETSTAT, a, s.class as u8, s.slot as u8),
                    span,
                );
                self.free_to(m);
                return Ok(());
            }

            // A proved class resolves to a field slot; anything else is
            // `SETFX`, which asks `assign_member` at run time (§8.5). This
            // is the write half of the same escape hatch `GETFX` gives the
            // read, and it used to refuse — which was `json_usage`'s first
            // refusal, on a `table` field reached through an `any`.
            let slot = self
                .class_of_expr(obj)
                .and_then(|class| self.chunk.classes[class as usize].layout.slot(name));
            // The receiver may be read in place only if evaluating the
            // right-hand side cannot disturb it — and an assignment's RHS is
            // an arbitrary expression, so the same purity test the binary
            // operators use applies here and usually declines. `self.x = v`
            // with `v` a local or a literal is the shape that qualifies, and
            // it is `oop`'s inner loop.
            let in_place = self.operand_is_pure(obj)
                && match value {
                    // A `Reg` right-hand side is already materialised, so
                    // nothing runs between the receiver read and the store.
                    Rhs::Reg(_) => true,
                    Rhs::Expr(v) => self.operand_is_pure(v),
                };
            let m = self.mark();
            let o = self.operand_to_reg(obj, in_place)?;
            // The value too: `self.y = y` in a constructor was `MOVE t y`
            // then `SETF self slot t`, for a `y` that was already parameter
            // 1. `in_place` covers both because it is the same question —
            // does anything run between these reads and the store.
            let v = self.rhs_operand(value, in_place)?;
            let (a, c) = (self.reg8(o, span)?, self.reg8(v, span)?);
            match slot {
                Some(slot) => self.emit(Instruction::abc(Op::SETF, a, slot as u8, c), span),
                None => {
                    let k = self.constant(
                        saule_interpreter::Value::Str(std::rc::Rc::new(name.clone())),
                        span,
                    )?;
                    let Ok(kb) = u8::try_from(k) else {
                        return Err(CompileError::unsupported(
                            "a dynamic member name past the 256-constant window",
                            span.clone(),
                        ));
                    };
                    self.emit(Instruction::abc(Op::SETFX, a, kb, c), span);
                }
            }
            self.free_to(m);
            return Ok(());
        }

        let Expr::Ident(name) = &target.value else {
            return Err(CompileError::unsupported(
                "assignment to this target",
                target.span.clone(),
            ));
        };

        match self.binding(target.id) {
            Some(Binding::Module { slot }) => {
                let slot = *slot;
                match self.f.lookup(name) {
                    // The module body holds it in a register.
                    Some(reg) => self.rhs_to(value, reg, span),
                    None => {
                        let m = self.mark();
                        let r = self.rhs_tmp(value)?;
                        let a = self.reg8(r, span)?;
                        let g = self.mod_slot(slot, span)?;
            self.emit(Instruction::abx(Op::SETMOD, a, g), span);
                        self.free_to(m);
                        Ok(())
                    }
                }
            }
            Some(Binding::Local { .. }) => {
                let reg = self.f.lookup(name).ok_or_else(|| CompileError::Unsupported {
                    thing: "assignment to a local the compiler has not seen declared",
                    span: span.clone(),
                })?;
                self.rhs_to(value, reg, span)
            }
            Some(Binding::Upvalue { .. }) => {
                // A closure writing through to the variable it captured —
                // the live-binding half of closure semantics.
                let m = self.mark();
                let r = self.rhs_tmp(value)?;
                let idx = self.capture_upvalue(name).ok_or_else(|| CompileError::Unsupported {
                    thing: "assignment to a captured variable the compiler could not locate",
                    span: span.clone(),
                })?;
                let (a, b) = (self.reg8(r, span)?, self.reg8(idx, span)?);
                self.emit(Instruction::abc(Op::SETUPVAL, a, b, 0), span);
                self.free_to(m);
                Ok(())
            }
            // Writing a static of the enclosing class by its bare name.
            // Mirrors the read in `ident_to`: `s.class` is the *declaring*
            // class, so a subclass assigning an inherited static updates the
            // one cell every sibling reads.
            Some(Binding::ClassStatic { class, name: field }) => {
                let (class, field) = (class.clone(), field.clone());
                let Some(s) = self.static_slot_of(&class, &field) else {
                    return Err(CompileError::unsupported(
                        "an assignment to a class static the compiler could not resolve",
                        span.clone(),
                    ));
                };
                let m = self.mark();
                let r = self.rhs_tmp(value)?;
                let a = self.reg8(r, span)?;
                self.emit(
                    Instruction::abc(Op::SETSTAT, a, s.class as u8, s.slot as u8),
                    span,
                );
                self.free_to(m);
                Ok(())
            }
            _ => Err(CompileError::unsupported(
                "assignment to this kind of binding",
                span.clone(),
            )),
        }
    }


    /// `target op= value`.
    ///
    /// Compiled as `target = target op value` with the target resolved
    /// **once**. The AST keeps this a node of its own rather than desugaring
    /// precisely so the target is not evaluated twice — `t[f()] += 1` must
    /// call `f` once — and the compiler has to honour that.
    pub(crate) fn compound_assign(
        &mut self,
        target: &Spanned<Expr>,
        op: saule_ast::BinOp,
        value: &Spanned<Expr>,
        span: &std::ops::Range<usize>,
    ) -> Result<(), CompileError> {
        // The target appears **twice** in what this builds — once as the
        // read inside `combined`, once as the destination — so every
        // sub-expression of the target is evaluated twice. The AST keeps
        // `CompoundAssign` as its own node precisely so that cannot happen,
        // and this is where that promise has to be kept.
        //
        // **This was a live silent miscompile, not merely a gap.** `t[idx()]
        // += 1` called `idx` twice under the VM and once under the
        // tree-walker — wrong value, exit status 0 — and no test could see
        // it, because the one fixture that writes it also compound-assigns
        // to a *member* two lines later, which refused and sent the whole
        // file to the oracle. A refusal hiding a miscompile beside it is
        // trap 3.
        //
        // The rule now: a target may be compiled here only if re-reading it
        // is unobservable. `self`, a bare name and a literal all qualify;
        // a call, a nested index or a chain does not, and refuses so the
        // module falls back to the engine that evaluates it once.
        fn rereadable(e: &Expr) -> bool {
            matches!(
                e,
                Expr::Self_
                    | Expr::Ident(_)
                    | Expr::Int(_)
                    | Expr::Float(_)
                    | Expr::Str(_)
                    | Expr::Bool(_)
                    | Expr::Nil
            )
        }
        let ok = match &target.value {
            Expr::Member { obj, .. } => rereadable(&obj.value),
            Expr::Index { obj, index } => rereadable(&obj.value) && rereadable(&index.value),
            // A bare local or module slot has no sub-expression at all.
            _ => true,
        };
        if !ok {
            return Err(CompileError::unsupported(
                "a compound assignment whose target cannot be evaluated only once",
                span.clone(),
            ));
        }
        // A synthetic `target op value` node, carrying the target's id so
        // the type table still answers for the operands.
        let combined = Spanned {
            value: Expr::Binary {
                op,
                lhs: Box::new(target.clone()),
                rhs: Box::new(value.clone()),
            },
            span: span.clone(),
            id: saule_ast::NodeId::NONE,
        };
        self.assign(target, Rhs::Expr(&combined))
    }

}
