//! Call sites: `f(...)`, `obj.m(...)`, and their tail-call forms.
//!
//! A call's arguments are compiled *directly into the callee's future
//! frame* (§6.2) rather than computed and moved, which is why so much of
//! this is about register placement rather than about the call itself.

use saule_ast::{Expr, Spanned};
use saule_interpreter::Value;
use saule_semantic::Binding;

use super::CompileError;
use super::args::ArgSlot;
use super::super::ctx::Compiler;
use super::results::{Results, Want};
use crate::op::{Instruction, Op};

impl Compiler<'_> {

    /// A call.
    ///
    /// Two forms are emitted today, both of which skip work the tree-walker
    /// does on every call (§19):
    ///
    /// * `CALLNAT` for a prelude function. The callee is resolved to its
    ///   actual `NativeFn` **at compile time** and stored as a constant, so
    ///   nothing looks up `print` at run time.
    /// * `CALLK` for a top-level `fn`. The proto is known statically, so the
    ///   callee load, the callability test and the arity check all disappear.
    ///
    /// Arguments are evaluated directly into the callee's future frame, so a
    /// call copies nothing (§6.2).
    pub(crate) fn call_to(
        &mut self,
        e: &Spanned<Expr>,
        callee: &Spanned<Expr>,
        args: &[saule_ast::CallArg],
        dst: u16,
    ) -> Result<(), CompileError> {
        self.call_to_want(e, callee, args, dst, Want::Fixed(1)).map(|_| ())
    }

    /// [`call_to`](Self::call_to), asking for a specific number of results.
    ///
    /// `dst` is where a `Fixed` result run lands. For [`Want::All`] the
    /// results stay in the call window — the count is not known until the
    /// callee returns — so `dst` is used only by the shapes that produce
    /// exactly one value, and the returned [`Results`] says which happened.
    /// **The window is not released**: the caller took the mark and must
    /// free back to it once it has consumed the results.
    pub(crate) fn call_to_want(
        &mut self,
        e: &Spanned<Expr>,
        callee: &Spanned<Expr>,
        args: &[saule_ast::CallArg],
        dst: u16,
        want: Want,
    ) -> Result<Results, CompileError> {
        let span = &e.span;
        // Only a bare name can be a `CALLNAT`/`CALLK` target; anything else
        // is a value and falls through to the generic `CALL` below.
        let name = match &callee.value {
            Expr::Ident(n) => n.as_str(),
            _ => "",
        };
        // §19: named arguments are reordered **here**, once, into plain
        // parameter order — so every path below (`CALLK`, `CALLSTAT`,
        // `CALLM`, the constructor, `CALLNAT`) sees an ordinary positional
        // list and the runtime never sees a name.
        //
        // `gap_fill` owns the synthesized `nil`s for parameters a named call
        // skips over; `positional` borrows from it, so it has to outlive the
        // borrow.
        let gap_fill;
        let positional: Vec<&Spanned<Expr>> = if args
            .iter()
            .any(|a| matches!(a, saule_ast::CallArg::Named { .. }) || a.is_trailing_block())
        {
            let Some(params) = self.callee_param_list(callee).cloned() else {
                return Err(CompileError::unsupported(
                    "a named argument to a callee the compiler cannot identify",
                    span.clone(),
                ));
            };
            let (order, fill) = self.reorder_args(args, &params, span)?;
            gap_fill = fill;
            order
                .into_iter()
                .map(|slot| match slot {
                    ArgSlot::Given(i) => match &args[i] {
                        saule_ast::CallArg::Positional(v) => v,
                        saule_ast::CallArg::Named { value, .. } => value,
                    },
                    ArgSlot::Nil(i) => &gap_fill[i],
                })
                .collect()
        } else {
            let mut positional = Vec::with_capacity(args.len());
            for a in args {
                match a {
                    saule_ast::CallArg::Positional(v) => positional.push(v),
                    saule_ast::CallArg::Named { .. } => {
                        return Err(CompileError::unsupported("a named argument", span.clone()));
                    }
                }
            }
            positional
        };

        // `obj.method(args)` and `Class.method(args)`.
        if let Expr::Member { obj, name } = &callee.value {
            return self.method_call_to(e, obj, name, &positional, dst, want);
        }
        // `obj?.method(args)` — the nil check has to wrap the *call*, not
        // just the callee lookup, or `p?.foo()` on a nil `p` would end up
        // calling nil rather than producing nil.
        if let Expr::SafeMember { obj, name } = &callee.value {
            return self.safe_method_call_to(e, obj, name, &positional, dst, want);
        }
        // `ClassName(args)` — a constructor, not a function call.
        if let Some(class) = self.layouts.get(name)
            && self.not_shadowed(name)
        {
            // A class is resolved to an index at compile time, so the
            // module body would happily construct one whose declaration it
            // has not reached yet — where the tree-walker looks the name up
            // in the environment and finds nothing. Exact, like the other
            // two positional guards: the declaration is above this point or
            // it is not. An imported class has no declaration in *this*
            // module, so it is only checked when this module declares it.
            if self.enclosing.is_empty()
                && self.module_type_decls.contains(name)
                && !self.module_decls_seen.contains(name)
            {
                return Err(CompileError::unsupported(
                    "a module-level use of a class declared further down",
                    span.clone(),
                ));
            }
            self.construct_to(class, &positional, dst, span)?;
            return self.one_result(dst, want, span);
        }

        // A name with a compile-time value — the prelude, or something an
        // `import` bound to a native package. Both are fixed before the
        // program runs, so the callee becomes a constant and this is a
        // `CALLNAT` with nothing looked up at run time.
        let folded = self.static_value(callee.id, name);
        match self.binding(callee.id) {
            _ if folded.is_some() => {
                let v = folded.expect("checked");
                let k = self.constant(v, span)?;
                let m = self.mark();
                // `CALLNAT` reads its arguments from `A+1..`, mirroring
                // `CALL`, so the window has room for the callee slot even
                // though the callee itself is a constant.
                let base =
                    self.alloc_n((positional.len() as u16 + 1).max(want.slots()), span)?;
                for (i, arg) in positional.iter().enumerate() {
                    self.expr_to(arg, base + 1 + i as u16)?;
                }
                let a = self.reg8(base, span)?;
                self.emit(
                    Instruction::abc(Op::CALLNAT, a, positional.len() as u8 + 1, want.c()),
                    span,
                );
                self.emit(Instruction::ax_of(Op::EXTRAARG, k as u32), span);
                self.finish_call(base, dst, want, m, span)
            }
            // A `static fn` of the enclosing class, called by its bare name
            // — `check(code)` from a sibling static. The name is a *method*,
            // so it lives in `smindex`, not in the `sindex` that
            // `ident_to`'s static read consults; without this arm it fell
            // through to the generic `CALL` and asked for a static *field*
            // that does not exist.
            Some(Binding::ClassStatic { class, name: m })
                if self.static_method_of(class, m).is_some() =>
            {
                let (cls, slot) = self
                    .static_method_of(&class.clone(), &m.clone())
                    .expect("checked in the guard");
                let mark = self.mark();
                // `CALLSTAT`'s window starts at the arguments: a static has
                // no receiver.
                let n = (positional.len().max(1) as u16).max(want.slots());
                let base = self.alloc_n(n, span)?;
                for (i, arg) in positional.iter().enumerate() {
                    self.expr_to(arg, base + i as u16)?;
                }
                let a = self.reg8(base, span)?;
                let packed = (cls << 16) | slot as u32;
                // `class Main` / `static fn` is the idiomatic shape of a
                // Saule program, so this is the commonest tail-recursive
                // function in the language. The tree-walker tail-calls it:
                // a bare-name static resolves to a `Value::Function`.
                if self.tail_position(want) {
                    self.emit(
                        Instruction::abc(Op::TAILCALLS, a, positional.len() as u8 + 1, 0),
                        span,
                    );
                    self.emit(Instruction::ax_of(Op::EXTRAARG, packed), span);
                    return Ok(Compiler::tail_result(base));
                }
                self.emit(
                    Instruction::abc(Op::CALLSTAT, a, positional.len() as u8 + 1, want.c()),
                    span,
                );
                self.emit(Instruction::ax_of(Op::EXTRAARG, packed), span);
                self.finish_call(base, dst, want, mark, span)
            }
            Some(Binding::Module { .. }) if self.fn_protos.contains_key(name) => {
                // Refused rather than allowed to fall through to the value
                // path below: that would emit a `CALL` on a module slot the
                // `SETMOD` has not reached, which errors with *its* wording
                // rather than the resolver's. `SAULE_DIFF` compares error
                // text, so "also fails" is not the same as "agrees".
                if !self.callk_resolvable(name) {
                    return Err(CompileError::unsupported(
                        "a module-level call to a function declared further down",
                        span.clone(),
                    ));
                }
                // Declared above, but its *body* may still reach something
                // that is not. The tree-walker errors when the callee runs;
                // the VM would resolve the proto and return a value.
                if self.reaches_undeclared(name) {
                    return Err(CompileError::unsupported(
                        "a module-level call whose callee reaches a declaration further down",
                        span.clone(),
                    ));
                }
                let proto = self.fn_protos[name];
                let m = self.mark();
                // `CALLK`'s window starts at the *arguments*: there is no
                // callee register, because the callee is the operand.
                let n = (positional.len().max(1) as u16).max(want.slots());
                let base = self.alloc_n(n, span)?;
                for (i, arg) in positional.iter().enumerate() {
                    self.expr_to(arg, base + i as u16)?;
                }
                let a = self.reg8(base, span)?;
                let t = self.own_call_target(proto, span)?;
                if self.tail_position(want) {
                    self.emit(
                        Instruction::abc(Op::TAILCALLK, a, positional.len() as u8 + 1, 0),
                        span,
                    );
                    self.emit(Instruction::ax_of(Op::EXTRAARG, t), span);
                    return Ok(Compiler::tail_result(base));
                }
                self.emit(
                    Instruction::abc(Op::CALLK, a, positional.len() as u8 + 1, want.c()),
                    span,
                );
                self.emit(Instruction::ax_of(Op::EXTRAARG, t), span);
                self.finish_call(base, dst, want, m, span)
            }
            // Anything else callable is a *value* — a local holding a
            // lambda, a captured one, a module slot bound to a function.
            // `CALL` loads the callee into the window's first register and
            // dispatches on what it finds, which is the general form the
            // typed ones above are specialisations of.
            _ => {
                let m = self.mark();
                let base =
                    self.alloc_n((positional.len() as u16 + 1).max(want.slots()), span)?;
                self.expr_to(callee, base)?;
                for (i, arg) in positional.iter().enumerate() {
                    self.expr_to(arg, base + 1 + i as u16)?;
                }
                let a = self.reg8(base, span)?;
                // The callee is a value, so *whether* this is a tail call is
                // a run-time question — a local can hold a lambda or a
                // native. `TAILCALL` asks it the same way the tree-walker
                // does, and falls back to an ordinary call and return.
                if self.tail_position(want) {
                    self.emit(
                        Instruction::abc(Op::TAILCALL, a, positional.len() as u8 + 1, 0),
                        span,
                    );
                    return Ok(Compiler::tail_result(base));
                }
                self.emit(
                    Instruction::abc(Op::CALL, a, positional.len() as u8 + 1, want.c()),
                    span,
                );
                self.finish_call(base, dst, want, m, span)
            }
        }
    }

    /// Whether this call site asked to replace the frame.
    ///
    /// Only the three branches that *can* replace one consult it; the whole
    /// question of whether a tail call is allowed **here at all** is settled
    /// once, in [`Compiler::ret`], because both vetoes are properties of the
    /// enclosing function rather than of the call.
    fn tail_position(&self, want: Want) -> bool {
        want == Want::Tail
    }

    /// `obj.method(args)` or `Class.method(args)`.
    fn method_call_to(
        &mut self,
        e: &Spanned<Expr>,
        obj: &Spanned<Expr>,
        name: &str,
        args: &[&Spanned<Expr>],
        dst: u16,
        want: Want,
    ) -> Result<Results, CompileError> {
        let span = &e.span;

        // `Event.Click(x, y)` — a tuple-variant constructor, not a call.
        if let Expr::Ident(en) = &obj.value
            && self.not_shadowed(en)
            && let Some(e_idx) = self.layouts.enum_of(en)
            && let Some(&tag) = self.chunk.enums[e_idx as usize].by_name.get(name)
        {
            self.variant_ctor_to(e_idx, tag, args, dst, span)?;
            return self.one_result(dst, want, span);
        }

        // `self.super(args)` — the parent's `init`, dispatched **statically**.
        //
        // It cannot go through the vtable: the child overrode that slot, so
        // a virtual call would re-enter the child's own constructor and
        // recurse forever. The parent's proto is known at compile time, so
        // `CALLK` is both correct and cheaper.
        if name == "super" && matches!(obj.value, Expr::Self_) {
            let Some(class) = self.f.current_class else {
                return Err(CompileError::unsupported("`self.super` outside a method", span.clone()));
            };
            let parent = self.chunk.classes[class as usize].parent.and_then(|p| {
                let pc = &self.chunk.classes[p as usize];
                let slot = pc.init? as usize;
                let target = pc.vtable.get(slot).copied()?;
                // Not `pc.module`: the parent may itself have *inherited*
                // its `init`, in which case the proto index is the
                // grandparent's chunk's (`ClassProto::vowner`). A class with
                // no `init` of its own between two that have one is all it
                // takes.
                let owner = *pc.vowner.get(slot)?;
                Some((self.chunk.classes[owner as usize].module, target))
            });
            let Some((pmod, target)) = parent.filter(|(_, t)| *t != u32::MAX) else {
                return Err(CompileError::unsupported(
                    "`self.super` on a class whose parent has no constructor",
                    span.clone(),
                ));
            };
            let m = self.mark();
            let base = self.alloc_n(args.len() as u16 + 1, span)?;
            let a = self.reg8(base, span)?;
            self.emit(Instruction::abc(Op::MOVE, a, 0, 0), span);
            for (i, arg) in args.iter().enumerate() {
                self.expr_to(arg, base + 1 + i as u16)?;
            }
            self.emit(Instruction::abc(Op::CALLK, a, args.len() as u8 + 2, 1), span);
            let t = self.call_target(pmod, target, span)?;
            self.emit(Instruction::ax_of(Op::EXTRAARG, t), span);
            self.free_to(m);
            // `self.super()` is a statement, not a value.
            let d = self.reg8(dst, span)?;
            self.emit(Instruction::abc(Op::LOADNIL, d, 0, 0), span);
            return self.one_result(dst, want, span);
        }

        // A static method: the receiver is a class name, so the target is
        // known outright and no dispatch happens at all.
        if let Some(class) = self.class_named_by(obj)
            && let Some(&s) = self.chunk.classes[class as usize].smindex.get(name)
        {
            // `C.go()` from the module body, where `go`'s body reads a
            // `fn` declared below the call. The class itself is declared —
            // the direct guard is satisfied — so only reachability sees it.
            if let Expr::Ident(cn) = &obj.value
                && self.reaches_undeclared(cn)
            {
                return Err(CompileError::unsupported(
                    "a module-level call whose callee reaches a declaration further down",
                    span.clone(),
                ));
            }
            let m = self.mark();
            let base = self.alloc_n((args.len().max(1) as u16).max(want.slots()), span)?;
            for (i, arg) in args.iter().enumerate() {
                self.expr_to(arg, base + i as u16)?;
            }
            let a = self.reg8(base, span)?;
            self.emit(
                Instruction::abc(Op::CALLSTAT, a, args.len() as u8 + 1, want.c()),
                span,
            );
            // The *declaring* class, so an inherited `static fn` reached
            // through a subclass name still loads the parent's proto.
            let packed = ((s.class) << 16) | s.slot as u32;
            self.emit(Instruction::ax_of(Op::EXTRAARG, packed), span);
            return self.finish_call(base, dst, want, m, span);
        }

        // `String.len(s)`, `Table.insert(t, v)` — a static on a *stdlib*
        // class. The prelude is fixed before a program runs, so the native
        // is resolved to a constant here and nothing looks it up at run
        // time. This is the same `CALLNAT` a bare `print` compiles to; the
        // only difference is where the value was found.
        // Gated on the resolver saying `Prelude`, for the reason spelled out
        // in `member_to`'s fold: a top-level `local String` is a module slot,
        // which `f.lookup` does not see, and resolving to the stdlib anyway
        // called `String.len` where the program meant its own table.
        if let Expr::Ident(recv) = &obj.value
            && let Some(Value::Class(cls)) = self.static_value(obj.id, recv)
            && let Some(v) = cls
                .lookup_static_field(name)
                .or_else(|| cls.lookup_static_method(name).map(|m| m.to_value()))
            && matches!(v, Value::Native(_) | Value::NativeClosure(_))
        {
            let k = self.constant(v, span)?;
            let m = self.mark();
            let base = self.alloc_n((args.len() as u16 + 1).max(want.slots()), span)?;
            for (i, arg) in args.iter().enumerate() {
                self.expr_to(arg, base + 1 + i as u16)?;
            }
            let a = self.reg8(base, span)?;
            self.emit(
                Instruction::abc(Op::CALLNAT, a, args.len() as u8 + 1, want.c()),
                span,
            );
            self.emit(Instruction::ax_of(Op::EXTRAARG, k as u32), span);
            return self.finish_call(base, dst, want, m, span);
        }

        // An interface-typed receiver: the concrete class is not known, but
        // the *interface's* method numbering is, so the call names a slot
        // and the itable translates it at run time (§8.4).
        if let Some(iface) = self
            .ty_name(obj.id)
            .and_then(|t| self.layouts.interface_of(t))
            && let Some(&slot) = self.chunk.interfaces[iface as usize].index.get(name)
        {
            if iface > 0xFFFF {
                return Err(CompileError::unsupported(
                    "a program with more than 65 536 interfaces",
                    span.clone(),
                ));
            }
            let m = self.mark();
            let base = self.alloc_n((args.len() as u16 + 1).max(want.slots()), span)?;
            self.expr_to(obj, base)?;
            for (i, arg) in args.iter().enumerate() {
                self.expr_to(arg, base + 1 + i as u16)?;
            }
            let a = self.reg8(base, span)?;
            self.emit(
                Instruction::abc(Op::CALLIF, a, args.len() as u8 + 1, slot as u8),
                span,
            );
            // Packed 8/16, the same split `CALLK` uses for its module: `C`
            // is spent on the interface's method slot, so the result count
            // has nowhere else to ride. `return s.area()` is what makes
            // this live — it asks for *all* results, and truncating to one
            // would be the same silent wrong value `RET1` used to produce.
            self.emit(
                Instruction::ax_of(Op::EXTRAARG, ((want.c() as u32) << 16) | iface),
                span,
            );
            return self.finish_call(base, dst, want, m, span);
        }

        // An instance method: one indexed load out of the vtable, and the
        // slot stays correct for a subclass receiver because a subclass's
        // vtable extends its parent's (§8.3).
        // No proved class: `CALLMX` defers to the tree-walker's own
        // `dispatch_member_call_multi`, which is what makes a file handle, an
        // enum variant and a module-level variable all work here without the
        // compiler learning each one (§8.5).
        let Some(class) = self.class_of_expr(obj) else {
            let m = self.mark();
            // The receiver sits at `A` and the arguments after it, exactly
            // as `CALLM` lays them out.
            let base = self.alloc_n((args.len() as u16 + 1).max(want.slots()), span)?;
            self.expr_to(obj, base)?;
            for (i, arg) in args.iter().enumerate() {
                self.expr_to(arg, base + 1 + i as u16)?;
            }
            let key = self.constant(Value::Str(saule_interpreter::value::SauleStr::new(name.to_string())), span)?;
            let a = self.reg8(base, span)?;
            self.emit(
                Instruction::abc(Op::CALLMX, a, args.len() as u8 + 1, want.c()),
                span,
            );
            self.emit(Instruction::ax_of(Op::EXTRAARG, key as u32), span);
            return self.finish_call(base, dst, want, m, span);
        };
        let Some(&slot) = self.chunk.classes[class as usize].vindex.get(name) else {
            return self.field_call_to(class, obj, name, args, dst, want, span);
        };

        let m = self.mark();
        // The receiver occupies the window's first register and becomes the
        // callee's `R[0]`, so `self` costs no copy.
        let base = self.alloc_n((args.len() as u16 + 1).max(want.slots()), span)?;
        self.expr_to(obj, base)?;
        for (i, arg) in args.iter().enumerate() {
            self.expr_to(arg, base + 1 + i as u16)?;
        }
        let a = self.reg8(base, span)?;
        // `CALLM` carries the vtable slot in `C`, so it can only ever
        // produce one result. Anything else moves the slot into `EXTRAARG`
        // and lets `C` say how many — which is the whole reason §15.10
        // reserves a second opcode for this.
        //
        // **`Fixed(0)` takes the one-word form too.** A call in statement
        // position wants nothing back, and letting the callee write its one
        // result into the call window is free: `finish_call` releases the
        // window without reading it. Sending that case down the `CALLM_MR`
        // path instead would trade the `MOVE` this saves for the `EXTRAARG`
        // that path needs, which is no saving at all.
        if matches!(want, Want::Fixed(0) | Want::Fixed(1)) {
            self.emit(
                Instruction::abc(Op::CALLM, a, args.len() as u8 + 1, slot as u8),
                span,
            );
        } else {
            self.emit(
                Instruction::abc(Op::CALLM_MR, a, args.len() as u8 + 1, want.c()),
                span,
            );
            self.emit(Instruction::ax_of(Op::EXTRAARG, slot as u32), span);
        }
        self.finish_call(base, dst, want, m, span)
    }

    /// `obj.name(args)` where `name` is a **field** holding a callable —
    /// `self.builder()`, where `builder: fn() -> View`.
    ///
    /// Reached only after `vindex` has said `name` is not a method. The
    /// tree-walker's order for an instance receiver is instance method,
    /// then static method, then instance field
    /// (`dispatch_member_call_multi`), so this handles the third case and
    /// refuses on anything else — including a name in `smindex`, which has
    /// no compiled form here. Which member a call *names* has to match, not
    /// just what it answers.
    #[allow(clippy::too_many_arguments)]
    fn field_call_to(
        &mut self,
        class: crate::chunk::ClassIdx,
        obj: &Spanned<Expr>,
        name: &str,
        args: &[&Spanned<Expr>],
        dst: u16,
        want: Want,
        span: &std::ops::Range<usize>,
    ) -> Result<Results, CompileError> {
        let proto = &self.chunk.classes[class as usize];
        // `layout.slot` is instance fields only, which is exactly what
        // `InstanceObject::field` consults — a *static* field of the same
        // name is not reachable through an instance under either engine, so
        // resolving against `sindex` here would compile a call the
        // tree-walker rejects.
        let field = (!proto.smindex.contains_key(name))
            .then(|| proto.layout.slot(name))
            .flatten();
        let Some(fslot) = field else {
            return Err(CompileError::unsupported(
                "a method the class does not declare",
                span.clone(),
            ));
        };

        let m = self.mark();
        let base = self.alloc_n((args.len() as u16 + 1).max(want.slots()), span)?;
        // Receiver first, then the arguments, then the field read — in that
        // order, because the tree-walker looks the field up *after* it has
        // evaluated the arguments. An argument that reassigns the field
        // would otherwise call the value the field held before it ran.
        //
        // Holding the receiver in `base` while the arguments are evaluated
        // is also what keeps it alive; `GETF` then overwrites it with the
        // callee, which is sound because the opcode clones the field out
        // before it writes its destination.
        self.expr_to(obj, base)?;
        for (i, arg) in args.iter().enumerate() {
            self.expr_to(arg, base + 1 + i as u16)?;
        }
        let a = self.reg8(base, span)?;
        self.emit(Instruction::abc(Op::GETF, a, a, fslot as u8), span);
        // The callee is a value now, so this is the same `CALL` a local
        // holding a lambda compiles to — no vtable, no `EXTRAARG`, and `C`
        // free to carry the result count directly.
        self.emit(
            Instruction::abc(Op::CALL, a, args.len() as u8 + 1, want.c()),
            span,
        );
        self.finish_call(base, dst, want, m, span)
    }
}
