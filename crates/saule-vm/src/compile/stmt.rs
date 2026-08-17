//! Statement codegen (`VM_DESIGN.md` §17 Pass 2, §11).

use saule_ast::{Expr, Spanned, Stmt};
use saule_semantic::Binding;

use super::CompileError;
use super::ctx::{Compiler, Num};
use crate::op::{Instruction, Op};

impl Compiler<'_> {
    /// Compile a block in its own scope.
    pub fn block(&mut self, stmts: &[Spanned<Stmt>]) -> Result<(), CompileError> {
        self.f.enter_scope();
        for s in stmts {
            self.stmt(s)?;
        }
        // A block that something captured needs its registers closed before
        // they are reused — otherwise the next iteration of a loop would
        // overwrite the value a closure from the previous one points at
        // (§7.2). Closures are Phase 2's next slice; the hook is here so the
        // allocator and the emitter agree from the start.
        if let Some(reg) = self.f.leave_scope() {
            let a = self.reg8(reg, &stmts.last().map(|s| s.span.clone()).unwrap_or(0..0))?;
            self.emit(Instruction::abc(Op::CLOSEUP, a, 0, 0), &(0..0));
        }
        Ok(())
    }

    /// Compile one statement. Returns the register holding its value when
    /// the statement is a bare expression — the module body's result is the
    /// last of those, which is what `run_in` returns and therefore what a
    /// differential test compares.
    pub fn stmt(&mut self, s: &Spanned<Stmt>) -> Result<Option<u16>, CompileError> {
        let span = &s.span;
        match &s.value {
            Stmt::Local { name, value, .. } => {
                self.local(name, value.as_ref(), span)?;
                Ok(None)
            }

            Stmt::Assign { target, value } => {
                self.assign(target, value)?;
                Ok(None)
            }

            Stmt::Expr(e) => {
                // Kept live rather than released: the module body's value is
                // the last expression statement's.
                let r = self.expr_tmp(e)?;
                Ok(Some(r))
            }

            Stmt::If {
                cond,
                then_block,
                elseifs,
                else_block,
            } => {
                self.if_chain(cond, then_block, elseifs, else_block.as_deref())?;
                Ok(None)
            }

            Stmt::While { cond, body } => {
                self.while_loop(cond, body)?;
                Ok(None)
            }

            Stmt::Repeat { body, cond } => {
                self.repeat_loop(body, cond)?;
                Ok(None)
            }

            Stmt::CompoundAssign { target, op, value } => {
                self.compound_assign(target, *op, value, span)?;
                Ok(None)
            }

            Stmt::ForNumeric {
                var,
                from,
                to,
                step,
                body,
                ..
            } => {
                self.for_numeric(var, from, to, step.as_ref(), body, span)?;
                Ok(None)
            }

            Stmt::ForIn { vars, iter, body } => {
                self.for_in(vars, iter, body, span)?;
                Ok(None)
            }

            Stmt::Try {
                body,
                catch_var,
                catch_ty,
                catch_body,
            } => {
                self.try_catch(body, catch_var, catch_ty, catch_body, span)?;
                Ok(None)
            }

            Stmt::Throw(e) => {
                let m = self.mark();
                let r = self.expr_tmp(e)?;
                let a = self.reg8(r, span)?;
                self.emit(Instruction::abc(Op::THROW, a, 0, 0), span);
                self.free_to(m);
                Ok(None)
            }

            Stmt::Return(values) => {
                self.ret(values, span)?;
                Ok(None)
            }

            Stmt::Break | Stmt::Continue => {
                if self.loops.is_empty() {
                    // `saule-semantic`'s control-flow pass already rejects
                    // this, so reaching it means analysis was skipped.
                    return Err(CompileError::unsupported(
                        "`break` or `continue` outside a loop",
                        span.clone(),
                    ));
                }
                let is_break = matches!(s.value, Stmt::Break);
                let label = self.emit_jump(Op::JMP, 0, span);
                let l = self.loops.last_mut().expect("checked");
                if is_break {
                    l.breaks.push(label);
                } else {
                    l.continues.push(label);
                }
                Ok(None)
            }

            Stmt::Decl(d) => {
                self.decl(d)?;
                Ok(None)
            }

            other => Err(CompileError::unsupported(stmt_label(other), span.clone())),
        }
    }

    /// `try … catch e: T … end`.
    ///
    /// Entering the `try` emits **no instructions at all** (§12.1): the
    /// protected range is recorded out of band in the proto's handler table,
    /// and only a `throw` ever consults it. The happy path costs nothing.
    fn try_catch(
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
        self.block(body)?;
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
            // A nullable or generic `catch` type is not a runtime test the
            // tree-walker performs either; `any` is the honest answer.
            _ => TypeDesc::Any,
        };
        self.chunk.type_descs.push(desc);
        (self.chunk.type_descs.len() - 1) as u32
    }

    /// A declaration. Only `fn` for now.
    ///
    /// The proto index was reserved by the pre-pass in `compile_with`, so a
    /// forward call already resolves; what happens here is compiling the body
    /// into that reserved slot and binding the name.
    fn decl(&mut self, d: &Spanned<saule_ast::Decl>) -> Result<(), CompileError> {
        let span = &d.span;
        // Classes are compiled in their own pass, before the module body,
        // so the declaration statement itself emits nothing.
        // Classes and enums are laid out in their own passes before the
        // module body, so the declaration statement itself emits nothing.
        if matches!(
            &d.value,
            saule_ast::Decl::Class { .. }
                | saule_ast::Decl::Enum { .. }
                | saule_ast::Decl::Interface { .. }
        ) {
            return Ok(());
        }
        // An `import` a program driver already resolved emits nothing: the
        // names it binds are types, and a type is a compile-time index
        // (§14). Without a driver the name has a module slot nothing writes,
        // so compiling on would read `nil`.
        if matches!(&d.value, saule_ast::Decl::Import { .. }) {
            return if self.imports_bound {
                Ok(())
            } else {
                Err(CompileError::unsupported(
                    "an import declaration",
                    span.clone(),
                ))
            };
        }

        let saule_ast::Decl::Function {
            name, params, body, ..
        } = &d.value
        else {
            return Err(CompileError::unsupported(
                "a declaration the compiler does not handle",
                span.clone(),
            ));
        };

        let Some(&idx) = self.fn_protos.get(name.as_str()) else {
            return Err(CompileError::unsupported(
                "a nested function declaration",
                span.clone(),
            ));
        };

        // Compile-time argument binding is §19's own slice of work; until it
        // lands, refuse the shapes that need it rather than mis-bind them.
        if params.len() > u8::MAX as usize {
            return Err(CompileError::unsupported("a function with over 255 parameters", span.clone()));
        }

        self.push_function(Some(name));
        // Parameters occupy registers `0..n`: the calling convention leaves
        // them there, because the callee's frame *is* the argument window
        // (§6.2). Nothing is copied on entry.
        let outcome = (|| -> Result<(), CompileError> {
            let n = params.len() as u16;
            let label = self.func_label();
            self.f
                .regs
                .reserve_params(n)
                .map_err(|o| o.at(&label, span.clone()))?;
            self.f.n_params = params.len() as u8;
            for (i, p) in params.iter().enumerate() {
                self.f.declare(&p.name, i as u16);
            }
            self.f.entries = self.param_entries(params, 0, span)?;
            for st in body {
                self.stmt(st)?;
            }
            Ok(())
        })();

        // Pop unconditionally: leaving the compiler inside a half-built
        // function after an error would corrupt every later diagnostic.
        let proto = self.pop_function(span);
        outcome?;
        self.chunk.protos[idx as usize] = std::rc::Rc::new(proto);

        // Bind the name too, so the function is a first-class value and not
        // only a `CALLK` target.
        if let Some(slot) = self
            .bindings
            .module_slots
            .iter()
            .position(|n| n.as_ref() == name.as_str())
        {
            let m = self.mark();
            let r = self.alloc(span)?;
            let a = self.reg8(r, span)?;
            let nested = self.f.nested_index(idx);
            self.emit(Instruction::abx(Op::CLOSURE, a, nested), span);
            let g = self.mod_slot(slot as u16, span)?;
            self.emit(Instruction::abx(Op::SETMOD, a, g), span);
            self.free_to(m);
        }
        Ok(())
    }

    fn ret(
        &mut self,
        values: &[Spanned<Expr>],
        span: &std::ops::Range<usize>,
    ) -> Result<(), CompileError> {
        match values.len() {
            0 => {
                self.emit(Instruction::abc(Op::RET0, 0, 0, 0), span);
                Ok(())
            }
            // The overwhelmingly common shape, and why `RET1` is its own
            // opcode rather than `RET` with a count.
            1 => {
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
                    return Err(CompileError::unsupported("returning over 254 values", span.clone()));
                }
                let m = self.mark();
                let base = self.alloc_n(n as u16, span)?;
                for (i, v) in values.iter().enumerate() {
                    self.expr_to(v, base + i as u16)?;
                }
                let a = self.reg8(base, span)?;
                self.emit(Instruction::abc(Op::RET, a, n as u8 + 1, 0), span);
                self.free_to(m);
                Ok(())
            }
        }
    }

    fn local(
        &mut self,
        name: &str,
        value: Option<&Spanned<Expr>>,
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
            let a = self.reg8(r, span)?;
            let g = self.mod_slot(slot, span)?;
            self.emit(Instruction::abx(Op::SETMOD, a, g), span);
            self.free_to(m);
            return Ok(());
        }

        // A frame local: allocate its register first, then evaluate straight
        // into it. No temporary, no move.
        let reg = self.alloc(span)?;
        match value {
            Some(v) => self.expr_to(v, reg)?,
            None => {
                let a = self.reg8(reg, span)?;
                self.emit(Instruction::abc(Op::LOADNIL, a, 0, 0), span);
            }
        }
        // Declared *after* the initializer, so `local x = x` reads the outer
        // `x` — the same order the resolver uses.
        self.f.declare(name, reg);
        Ok(())
    }

    fn assign(
        &mut self,
        target: &Spanned<Expr>,
        value: &Spanned<Expr>,
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
                self.expr_to(value, base + 2)?;
                let a = self.reg8(base, span)?;
                self.emit(Instruction::abc(Op::CALLM, a, 3, slot as u8), span);
                self.free_to(m);
                return Ok(());
            }

            let m = self.mark();
            let o = self.expr_tmp(obj)?;
            let k = self.expr_tmp(index)?;
            let v = self.expr_tmp(value)?;
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
                let r = self.expr_tmp(value)?;
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
                let v = self.expr_tmp(value)?;
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
                let r = self.expr_tmp(value)?;
                let a = self.reg8(r, span)?;
                self.emit(
                    Instruction::abc(Op::SETSTAT, a, s.class as u8, s.slot as u8),
                    span,
                );
                self.free_to(m);
                return Ok(());
            }

            let Some(class) = self.class_of_expr(obj) else {
                return Err(CompileError::unsupported(
                    "an assignment to a member of a receiver with no proved class",
                    span.clone(),
                ));
            };
            let Some(slot) = self.chunk.classes[class as usize].layout.slot(name) else {
                return Err(CompileError::unsupported(
                    "an assignment to something that is not an instance field",
                    span.clone(),
                ));
            };
            let m = self.mark();
            let o = self.expr_tmp(obj)?;
            let v = self.expr_tmp(value)?;
            let (a, c) = (self.reg8(o, span)?, self.reg8(v, span)?);
            self.emit(Instruction::abc(Op::SETF, a, slot as u8, c), span);
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
                    Some(reg) => self.expr_to(value, reg),
                    None => {
                        let m = self.mark();
                        let r = self.expr_tmp(value)?;
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
                self.expr_to(value, reg)
            }
            Some(Binding::Upvalue { .. }) => {
                // A closure writing through to the variable it captured —
                // the live-binding half of closure semantics.
                let m = self.mark();
                let r = self.expr_tmp(value)?;
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
                let r = self.expr_tmp(value)?;
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

    /// `repeat … until cond`.
    ///
    /// Two things separate it from `while`. The body always runs once, and
    /// the condition is evaluated **inside the body's scope** — Lua-style,
    /// so `until` can read a local the body declared. That is why the scope
    /// is opened here rather than delegated to `block`.
    fn repeat_loop(
        &mut self,
        body: &[Spanned<Stmt>],
        cond: &Spanned<Expr>,
    ) -> Result<(), CompileError> {
        let top = self.f.label_here();
        self.loops.push(Default::default());
        self.f.enter_scope();
        for st in body {
            self.stmt(st)?;
        }

        // `continue` skips the rest of the body but still has to test the
        // condition, so it lands here.
        let test_at = self.f.label_here();
        let m = self.mark();
        let r = self.expr_tmp(cond)?;
        let a = self.reg8(r, &cond.span)?;
        // `C = 0` skips the next instruction when the condition is truthy —
        // and the next instruction is the back edge, so a true `until` exits.
        self.emit(Instruction::abc(Op::TEST, a, 0, 0), &cond.span);
        self.free_to(m);
        self.emit_jump_back(Op::JMP, 0, top, &cond.span)?;

        if let Some(reg) = self.f.leave_scope() {
            let a = self.reg8(reg, &cond.span)?;
            self.emit(Instruction::abc(Op::CLOSEUP, a, 0, 0), &cond.span);
        }
        let l = self.loops.pop().expect("pushed above");
        for c in l.continues {
            self.patch_to(c, test_at)?;
        }
        for b in l.breaks {
            self.patch_here(b)?;
        }
        Ok(())
    }

    /// `target op= value`.
    ///
    /// Compiled as `target = target op value` with the target resolved
    /// **once**. The AST keeps this a node of its own rather than desugaring
    /// precisely so the target is not evaluated twice — `t[f()] += 1` must
    /// call `f` once — and the compiler has to honour that.
    fn compound_assign(
        &mut self,
        target: &Spanned<Expr>,
        op: saule_ast::BinOp,
        value: &Spanned<Expr>,
        span: &std::ops::Range<usize>,
    ) -> Result<(), CompileError> {
        if matches!(&target.value, Expr::Member { .. }) {
            return Err(CompileError::unsupported(
                "a compound assignment to a member",
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
        self.assign(target, &combined)
    }

    fn if_chain(
        &mut self,
        cond: &Spanned<Expr>,
        then_block: &[Spanned<Stmt>],
        elseifs: &[(Spanned<Expr>, Vec<Spanned<Stmt>>)],
        else_block: Option<&[Spanned<Stmt>]>,
    ) -> Result<(), CompileError> {
        let mut to_end = Vec::new();

        let mut arms: Vec<(&Spanned<Expr>, &[Spanned<Stmt>])> = vec![(cond, then_block)];
        arms.extend(elseifs.iter().map(|(c, b)| (c, b.as_slice())));

        for (c, body) in arms {
            // Jump past this arm when the condition is false.
            let skip = self.cond_jump_if_false(c)?;
            self.block(body)?;
            // Only worth a jump to the end when something follows.
            to_end.push(self.emit_jump(Op::JMP, 0, &c.span));
            self.patch_here(skip)?;
        }

        if let Some(b) = else_block {
            self.block(b)?;
        }
        for l in to_end {
            self.patch_here(l)?;
        }
        Ok(())
    }

    fn while_loop(
        &mut self,
        cond: &Spanned<Expr>,
        body: &[Spanned<Stmt>],
    ) -> Result<(), CompileError> {
        let top = self.f.label_here();
        let exit = self.cond_jump_if_false(cond)?;
        self.loops.push(Default::default());
        self.block(body)?;
        let l = self.loops.pop().expect("pushed above");
        // `continue` re-tests the condition, so it lands where the back edge
        // goes.
        for c in l.continues {
            self.patch_to(c, top)?;
        }
        self.emit_jump_back(Op::JMP, 0, top, &cond.span)?;
        self.patch_here(exit)?;
        for b in l.breaks {
            self.patch_here(b)?;
        }
        Ok(())
    }

    /// Emit a test of `cond` plus a jump taken when it is **false**.
    ///
    /// `TEST` skips the following instruction when truthiness matches, and
    /// by convention that following instruction is the jump — which is why
    /// the comparison opcodes carry no jump operand at all (§15.7).
    fn cond_jump_if_false(
        &mut self,
        cond: &Spanned<Expr>,
    ) -> Result<super::ctx::Label, CompileError> {
        let m = self.mark();
        let r = self.expr_tmp(cond)?;
        let a = self.reg8(r, &cond.span)?;
        // `TEST` skips the next instruction when truthiness *matches* `C`.
        // The next instruction is the jump-to-else, so `C = 0` means "skip
        // the jump when the condition is true" — fall through into the
        // then-branch. Getting this polarity backwards inverts every `if` in
        // the language and is invisible in the disassembly, which is why the
        // differential test caught it and reading the listing did not.
        self.emit(Instruction::abc(Op::TEST, a, 0, 0), &cond.span);
        let label = self.emit_jump(Op::JMP, 0, &cond.span);
        self.free_to(m);
        Ok(label)
    }

    fn for_numeric(
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
    fn for_in(
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
                return Err(CompileError::unsupported(
                    "a `for … in` over a source that is not a proved table, function or iterable",
                    span.clone(),
                ));
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

    /// Whether a declaration here is a module slot rather than a register:
    /// the module body, outside any block.
    fn at_module_top(&self) -> bool {
        self.f.name.as_deref() == Some("main") && self.f.regs.block_depth() == 0
    }
}

fn stmt_label(s: &Stmt) -> &'static str {
    match s {
        Stmt::LocalMulti { .. } => "a parallel `local`",
        Stmt::AssignMulti { .. } => "a parallel assignment",
        Stmt::Error => "an unparsable statement",
        _ => "this statement",
    }
}
