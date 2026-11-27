//! Call sites: `f(...)`, `obj.m(...)`, and their tail-call forms.
//!
//! A call's arguments are compiled *directly into the callee's future
//! frame* (§6.2) rather than computed and moved, which is why so much of
//! this is about register placement rather than about the call itself.

use saule_ast::{Expr, Spanned};
use saule_runtime::Value;
use saule_semantic::Binding;

use super::super::ctx::Compiler;
use super::CompileError;
use super::args::ArgSlot;
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
        self.call_to_want(e, callee, args, dst, Want::Fixed(1))
            .map(|_| ())
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
        let has_named = args
            .iter()
            .any(|a| matches!(a, saule_ast::CallArg::Named { .. }));
        let declared = if has_named || args.last().is_some_and(|a| a.is_trailing_block()) {
            let params = self
                .callee_param_list(callee)
                .cloned()
                .or_else(|| self.native_param_list(callee));
            if params.is_none() {
                self.check_unbound_trailing_block(callee, args, has_named, span)?;
            }
            params
        } else {
            None
        };
        let positional: Vec<&Spanned<Expr>> = if let Some(params) = declared {
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
            // An operand beside this call may have been read where it is
            // on the word that the call runs nothing (`runs_in_place`). If
            // the call was then compiled as a call after all, that operand
            // could be changed under it: refused here, loudly, rather than
            // compiled wrong.
            let promised = want == Want::Fixed(1) && self.runs_in_place(e);
            let out = self.method_call_to(e, obj, name, &positional, dst, want)?;
            if promised && self.in_place_last != Some(e.id) {
                return Err(CompileError::unsupported(
                    "a call that was to be compiled in place and was not",
                    span.clone(),
                ));
            }
            return Ok(out);
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
                && self.module_decls.contains(name)
                && !self.module_decls_seen.contains(name)
            {
                return Err(crate::compile::declared_later(name, false, span.clone()));
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
                let base = self.alloc_n((positional.len() as u16 + 1).max(want.slots()), span)?;
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
                self.emit_static_call(a, positional.len() as u8 + 1, want.c(), packed, span);
                self.finish_call(base, dst, want, mark, span)
            }
            Some(Binding::Module { .. }) if self.fn_protos.contains_key(name) => {
                // Refused rather than allowed to fall through to the value
                // path below: that would emit a `CALL` on a module slot the
                // `SETMOD` has not reached, which errors with *its* wording
                // rather than the resolver's. `SAULE_DIFF` compares error
                // text, so "also fails" is not the same as "agrees".
                if !self.callk_resolvable(name) {
                    return Err(crate::compile::declared_later(name, false, span.clone()));
                }
                // Declared above, but its *body* may still reach something
                // that is not — which would otherwise resolve the proto and
                // return a value where the program is an error.
                if self.reaches_undeclared(name) {
                    return Err(crate::compile::declared_later(name, true, span.clone()));
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
            // A local that holds one upvalue-free lambda for its whole
            // life: the callee is known, so this is the `CALLK` a top-level
            // `fn` gets — no `MOVE` of the closure into the window, no
            // handle to check, count or downcast on the way in.
            Some(Binding::Local { .. })
                if let Some((proto, inline)) = self.static_lambda(name, positional.len()) =>
            {
                if let Some(body) = inline
                    && want == Want::Fixed(1)
                {
                    return self.inline_lambda_call(&body, &positional, dst, span);
                }
                let m = self.mark();
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
                let base = self.alloc_n((positional.len() as u16 + 1).max(want.slots()), span)?;
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

    /// A call with a trailing block or a named argument, to a callee with
    /// no parameter list the compiler can read: a stdlib member, a local
    /// holding a function value. (A native package's class method has one —
    /// see `native_param_list`.) `Ok` means the arguments bind positionally.
    ///
    /// A named argument needs the parameter *names*, which only a
    /// declaration has. A trailing block alone needs no names — only which
    /// parameters take a function, and the callee's static type says that
    /// much. When the rule lands it on the next positional slot, as it does
    /// for every native, binding it is positional; landing further on would
    /// skip a parameter whose default only the callee's declaration knows.
    fn check_unbound_trailing_block(
        &self,
        callee: &Spanned<Expr>,
        args: &[saule_ast::CallArg],
        has_named: bool,
        span: &std::ops::Range<usize>,
    ) -> Result<(), CompileError> {
        if has_named {
            return Err(CompileError::unsupported(
                "a named argument to a callee the compiler cannot identify",
                span.clone(),
            ));
        }
        let fn_params = match self.types.get(&callee.id) {
            Some(saule_ast::Type::Function { params, .. }) => Some(params),
            Some(saule_ast::Type::Nullable(inner)) => match &**inner {
                saule_ast::Type::Function { params, .. } => Some(params),
                _ => None,
            },
            _ => None,
        };
        let Some(ps) = fn_params else {
            return Ok(());
        };
        let last = args.len() - 1;
        let slots: Vec<_> = ps
            .iter()
            .map(|t| saule_ast::ParamSlot::new("", t))
            .collect();
        match saule_ast::trailing_block_slot(&slots, |i| i < last) {
            Some(s) if s != last => Err(CompileError::unsupported(
                "a trailing block that skips a parameter of a function value",
                span.clone(),
            )),
            _ => Ok(()),
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

        // `Display.connect(name)` / `Display.publish(name, service)` on an
        // `extern interface`.
        if let Some(r) = self.extern_static_call(obj, name, args, dst, want, span) {
            return r;
        }

        // `self.super(args)` — the parent's `init`, dispatched **statically**.
        //
        // It cannot go through the vtable: the child overrode that slot, so
        // a virtual call would re-enter the child's own constructor and
        // recurse forever. The parent's proto is known at compile time, so
        // `CALLK` is both correct and cheaper.
        if name == "super" && matches!(obj.value, Expr::Self_) {
            let Some(class) = self.f.current_class else {
                return Err(CompileError::unsupported(
                    "`self.super` outside a method",
                    span.clone(),
                ));
            };
            let parent = self.chunk.classes[class as usize].parent.and_then(|p| {
                let pc = &self.chunk.classes[p as usize];
                let slot = pc.init? as usize;
                // `vowner` names the ancestor that *declares* this slot, and
                // it is correct from layout time — an inherited slot clones
                // the parent's entry, so it keeps pointing at whoever really
                // declared the `init`. Read both the proto and the module
                // out of *that* class.
                //
                // Not out of `pc`'s own row: the parent may have inherited
                // its `init` without overriding it, and then `pc.vtable[slot]`
                // is still the `u32::MAX` placeholder until Pass 2a sweeps it
                // down — which happens after every body is compiled, far too
                // late for this. The declaring ancestor's row, by contrast,
                // was filled by its own codegen. A class with no `init` of
                // its own between two that have one was all it took.
                let owner = *pc.vowner.get(slot)?;
                let oc = &self.chunk.classes[owner as usize];
                let target = oc.vtable.get(slot).copied()?;
                Some((oc.module, target))
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
            self.emit(
                Instruction::abc(Op::CALLK, a, args.len() as u8 + 2, 1),
                span,
            );
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
                return Err(crate::compile::declared_later(
                    &format!("{cn}.{name}"),
                    true,
                    span.clone(),
                ));
            }
            // The *declaring* class, so an inherited `static fn` reached
            // through a subclass name still loads the parent's proto.
            let packed = ((s.class) << 16) | s.slot as u32;
            // A function of a few early returns, called with what it needs
            // known here: its body, in place of the call.
            if want == Want::Fixed(1)
                && let Some(out) = self.inline_static_call(packed, args, dst, span)?
            {
                self.in_place_last = Some(e.id);
                return Ok(out);
            }
            let m = self.mark();
            let base = self.alloc_n((args.len().max(1) as u16).max(want.slots()), span)?;
            for (i, arg) in args.iter().enumerate() {
                self.expr_to(arg, base + i as u16)?;
            }
            let a = self.reg8(base, span)?;
            self.emit_static_call(a, args.len() as u8 + 1, want.c(), packed, span);
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
        //
        // Not for an `extern interface`: its value is usually a proxy for an
        // object in another process, which has no itable. Those calls go by
        // name (`CALLMX`, below), which reaches proxies and local objects
        // alike.
        if let Some(iface) = self
            .ty_name(obj.id)
            .filter(|t| !saule_semantic::is_extern_interface(t))
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
            let key = self.constant(
                Value::Str(saule_runtime::value::SauleStr::new(name.to_string())),
                span,
            )?;
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
            // A small method that cannot fail is compiled right here, behind
            // a test of the receiver's class: see `compile::inline`.
            let wanted = want != Want::Fixed(0);
            if !self.inline_method(class, slot, base, args.len() + 1, wanted, false, span)? {
                self.emit(
                    Instruction::abc(Op::CALLM, a, args.len() as u8 + 1, slot as u8),
                    span,
                );
            }
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

use crate::compile::ctx::func::{InlineBody, StaticLambda};

impl Compiler<'_> {
    /// The proto `name` is bound to, when it is a local in
    /// [`FuncCtx::static_lambdas`](crate::compile::ctx::func::FuncCtx) still
    /// in scope, and `n_args` is exactly its parameter count — with its
    /// body, when that can be compiled in place of the call.
    fn static_lambda(&self, name: &str, n_args: usize) -> Option<(u32, Option<InlineBody>)> {
        let reg = self.f.lookup(name)?;
        let l = self
            .f
            .static_lambdas
            .iter()
            .find(|l| l.name.as_ref() == name && l.reg == reg)?;
        (self.chunk.proto(l.proto).n_params as usize == n_args).then(|| (l.proto, l.inline.clone()))
    }

    /// `add(s, i)` where `add` is `(a, b) => a + b`: the body, compiled
    /// right here with `a` and `b` naming the arguments' registers.
    ///
    /// Only for a body [`inlinable_lambda`](Self::inlinable_lambda)
    /// accepted — arithmetic and comparisons on proved numbers, which can
    /// neither fail nor call anything — so no error, stack trace or depth
    /// limit can tell that the call never happened.
    fn inline_lambda_call(
        &mut self,
        body: &InlineBody,
        args: &[&Spanned<Expr>],
        dst: u16,
        span: &std::ops::Range<usize>,
    ) -> Result<Results, CompileError> {
        let (params, expr) = &**body;
        let m = self.mark();
        // Arguments are read where they are when all of them are pure —
        // the body only reads its parameters, so there is nothing to protect
        // them from but each other's evaluation.
        let in_place = args.iter().all(|a| self.operand_is_pure(a));
        let mut regs = Vec::with_capacity(args.len());
        for a in args {
            regs.push(self.operand_to_reg(a, in_place)?);
        }
        // The body writes its destination while it may still read a
        // parameter, so a destination that *is* a parameter's register goes
        // through a temporary.
        let target = if regs.contains(&dst) {
            self.alloc(span)?
        } else {
            dst
        };
        self.f.enter_scope();
        for (p, r) in params.iter().zip(&regs) {
            self.f.declare(p, *r);
        }
        let out = self.expr_to(expr, target);
        self.f.leave_scope();
        out?;
        if target != dst {
            let (a, b) = (self.reg8(dst, span)?, self.reg8(target, span)?);
            self.emit(Instruction::abc(Op::MOVE, a, b, 0), span);
        }
        self.free_to(m);
        Ok(Results {
            base: dst,
            count: Some(1),
            terminated: false,
        })
    }

    /// The parameters and body of `lambda`, if a call to it may be replaced
    /// by its body: one expression, no declared return type, parameters
    /// whose declared types convert nothing on entry, and a body that only
    /// reads them through arithmetic and comparisons the typechecker proved
    /// numeric. Anything that could fail, call, capture or convert keeps the
    /// call.
    pub(crate) fn inlinable_lambda(&self, lambda: &Spanned<Expr>) -> Option<InlineBody> {
        let Expr::Lambda {
            params,
            return_ty: None,
            body: saule_ast::LambdaBody::Expr(expr),
        } = &lambda.value
        else {
            return None;
        };
        let plain = |t: &saule_ast::Type| match t {
            saule_ast::Type::Named(n) => {
                matches!(
                    n.as_str(),
                    "any" | "integer" | "float" | "number" | "boolean"
                )
            }
            _ => false,
        };
        if params
            .iter()
            .any(|p| p.variadic || p.default.is_some() || !plain(&p.ty))
        {
            return None;
        }
        let names: Vec<String> = params.iter().map(|p| p.name.clone()).collect();
        self.cannot_fail(expr, &names)
            .then(|| std::rc::Rc::new((names, std::sync::Arc::clone(expr))))
    }

    /// Whether `m` is a function a call could be replaced by, as far as its
    /// own shape says: fixed parameters, and a body that is `return e`
    /// after at most two `if c then return e end`. Whether a particular
    /// call can be is [`inline_static_call`](Self::inline_static_call)'s.
    pub(crate) fn returns_early_only(m: &saule_ast::Method) -> bool {
        use saule_ast::Stmt;
        let one = |values: &Vec<Spanned<Expr>>| values.len() == 1;
        let Some((last, earlier)) = m.body.split_last() else {
            return false;
        };
        m.is_static
            && m.params.iter().all(|p| !p.variadic && p.default.is_none())
            && earlier.len() <= 2
            && matches!(&last.value, Stmt::Return(values) if one(values))
            && earlier.iter().all(|s| match &s.value {
                Stmt::If {
                    then_block,
                    elseifs,
                    else_block: None,
                    ..
                } => {
                    elseifs.is_empty()
                        && matches!(then_block.as_slice(), [only]
                            if matches!(&only.value, Stmt::Return(values) if one(values)))
                }
                _ => false,
            })
    }

    /// `Main.pickMax(a, b, better)` where `pickMax` only chooses between
    /// its arguments by calling one of them, and `better` is a lambda this
    /// function holds in a local for its whole life: the body, compiled
    /// right here, with the lambda's own body where it is called. A generic
    /// function over "how to compare" becomes the comparison.
    ///
    /// `None` when the call has to stay a call. As for a lambda
    /// ([`inline_lambda_call`](Self::inline_lambda_call)), only what can
    /// neither fail nor call anything is taken, so that no error, stack
    /// trace or depth limit can tell the call never happened: every
    /// expression of the body is a parameter, a call of a parameter that is
    /// such a lambda here, or arithmetic and comparisons on proved numbers.
    /// What [`inline_static_call`](Self::inline_static_call) needs to
    /// know before it emits anything, or `None` when the call stays a
    /// call: the function, which of its parameters are values, and the
    /// lambda each of the others was given. Reads, and changes nothing.
    fn static_call_plan(&self, packed: u32, args: &[&Spanned<Expr>]) -> Option<StaticPlan> {
        use saule_ast::Stmt;
        let m = self.inline_statics.get(&packed).cloned()?;
        if m.params.len() != args.len() {
            return None;
        }
        // Which parameters the body calls, and with how many arguments.
        let mut called: Vec<(String, usize)> = Vec::new();
        let mut exprs: Vec<&Spanned<Expr>> = Vec::new();
        for stmt in &m.body {
            match &stmt.value {
                Stmt::Return(values) => exprs.push(&values[0]),
                Stmt::If {
                    cond, then_block, ..
                } => {
                    exprs.push(cond);
                    if let Stmt::Return(values) = &then_block[0].value {
                        exprs.push(&values[0]);
                    }
                }
                _ => return None,
            }
        }
        for e in &exprs {
            Self::calls_in(e, &mut called);
        }
        // A parameter that is matched on has to be what its type says and
        // nothing less: a nullable one may be nil, which has no tag.
        let mut matched: Vec<String> = Vec::new();
        for e in &exprs {
            Self::matched_in(e, &mut matched);
        }
        for p in &m.params {
            let an_enum = matches!(&p.ty, saule_ast::Type::Named(n)
                if self.layouts.enum_of(n).is_some());
            if matched.contains(&p.name) && !an_enum {
                return None;
            }
        }
        // Each called parameter has to be given a lambda that can be
        // compiled in place; the others are values.
        let mut values: Vec<String> = Vec::new();
        let mut lambdas: Vec<(String, u16, u32, InlineBody)> = Vec::new();
        for (p, arg) in m.params.iter().zip(args.iter().copied()) {
            let arity = called.iter().find(|(n, _)| n == &p.name).map(|(_, n)| *n);
            let Some(arity) = arity else {
                values.push(p.name.clone());
                continue;
            };
            // What is called is a function, not something to match on.
            if matched.contains(&p.name) {
                return None;
            }
            let Expr::Ident(given) = &arg.value else {
                return None;
            };
            let (Some(reg), Some((proto, Some(body)))) =
                (self.f.lookup(given), self.static_lambda(given, arity))
            else {
                return None;
            };
            lambdas.push((p.name.clone(), reg, proto, body));
        }
        let names: Vec<&str> = lambdas.iter().map(|(n, ..)| n.as_str()).collect();
        if !exprs.iter().all(|e| self.only_chooses(e, &values, &names)) {
            return None;
        }
        Some(StaticPlan { m, values, lambdas })
    }

    /// The static function `obj.name` names, as `CALLSTAT` packs it, when
    /// `obj` is a class and nothing between the call and the function has
    /// yet to be declared.
    fn static_target(&self, obj: &Spanned<Expr>, name: &str) -> Option<u32> {
        let Expr::Ident(class_name) = &obj.value else {
            return None;
        };
        if self.layouts.enum_of(class_name).is_some() || self.reaches_undeclared(class_name) {
            return None;
        }
        let class = self.class_named_by(obj)?;
        let s = self.chunk.classes[class as usize].smindex.get(name)?;
        Some((s.class << 16) | s.slot as u32)
    }

    /// Whether `e` is a call of a static function that is compiled in
    /// place of the call, with arguments that run nothing: an operand
    /// beside it can then be read where it is
    /// ([`operand_is_pure`](Self::operand_is_pure)). `total = total +
    /// Main.handle(e)` copied `total` aside before every call that was
    /// never made.
    ///
    /// The promise is kept or the program does not compile: see
    /// `in_place_last`.
    pub(crate) fn runs_in_place(&self, e: &Spanned<Expr>) -> bool {
        let Expr::Call { callee, args, .. } = &e.value else {
            return false;
        };
        let Expr::Member { obj, name } = &callee.value else {
            return false;
        };
        let mut given = Vec::with_capacity(args.len());
        for a in args {
            match a {
                saule_ast::CallArg::Positional(v) if !a.is_trailing_block() => given.push(v),
                _ => return false,
            }
        }
        let Some(packed) = self.static_target(obj, name) else {
            return false;
        };
        let Some(plan) = self.static_call_plan(packed, &given) else {
            return false;
        };
        plan.m
            .params
            .iter()
            .zip(&given)
            .filter(|(p, _)| plan.values.contains(&p.name))
            .all(|(_, a)| self.operand_is_pure(a))
    }

    fn inline_static_call(
        &mut self,
        packed: u32,
        args: &[&Spanned<Expr>],
        dst: u16,
        span: &std::ops::Range<usize>,
    ) -> Result<Option<Results>, CompileError> {
        use saule_ast::Stmt;
        let Some(StaticPlan { m, values, lambdas }) = self.static_call_plan(packed, args) else {
            return Ok(None);
        };

        let mark = self.mark();
        // A destination above everything in use is kept out of the
        // arguments' way, so that the body's value goes straight to it.
        while self.f.regs.top() <= dst {
            self.alloc(span)?;
        }
        // The value arguments, in the order they are written: read where
        // they are when none of them can disturb another.
        let given: Vec<&Spanned<Expr>> = m
            .params
            .iter()
            .zip(args.iter().copied())
            .filter(|(p, _)| values.contains(&p.name))
            .map(|(_, a)| a)
            .collect();
        let in_place = given.iter().all(|a| self.operand_is_pure(a));
        let mut regs = Vec::with_capacity(given.len());
        for a in &given {
            regs.push(self.operand_to_reg(a, in_place)?);
        }
        let target = if regs.contains(&dst) {
            self.alloc(span)?
        } else {
            dst
        };

        // The body sees its parameters and nothing of this function's:
        // every name in it was checked to be one of them. A lambda
        // parameter is the caller's local under the parameter's name.
        let kept = std::mem::take(&mut self.f.static_lambdas);
        self.f.enter_scope();
        for (name, reg) in values.iter().zip(&regs) {
            self.f.declare(name, *reg);
        }
        for (name, reg, proto, body) in &lambdas {
            self.f.declare(name, *reg);
            self.f.static_lambdas.push(StaticLambda {
                name: std::rc::Rc::from(name.as_str()),
                reg: *reg,
                proto: *proto,
                inline: Some(body.clone()),
            });
        }
        let mut to_end = Vec::new();
        let mut out = Ok(());
        for stmt in &m.body {
            out = (|| {
                match &stmt.value {
                    Stmt::If {
                        cond, then_block, ..
                    } => {
                        let fails = self.cond_jumps_if_false(cond)?;
                        if let Stmt::Return(values) = &then_block[0].value {
                            self.expr_to(&values[0], target)?;
                        }
                        to_end.push(self.emit_jump(Op::JMP, 0, span));
                        for l in fails {
                            self.patch_here(l)?;
                        }
                    }
                    Stmt::Return(values) => {
                        self.expr_to(&values[0], target)?;
                    }
                    _ => {}
                }
                Ok(())
            })();
            if out.is_err() {
                break;
            }
        }
        self.f.leave_scope();
        self.f.static_lambdas = kept;
        out?;
        for l in to_end {
            self.patch_here(l)?;
        }
        if target != dst {
            let (a, b) = (self.reg8(dst, span)?, self.reg8(target, span)?);
            self.emit(Instruction::abc(Op::MOVE, a, b, 0), span);
        }
        self.free_to(mark);
        Ok(Some(Results {
            base: dst,
            count: Some(1),
            terminated: false,
        }))
    }

    /// The names `e` calls as functions, each with how many arguments,
    /// added to `out`. A name called with two different counts is given
    /// one no lambda has, so that it is taken for no lambda.
    fn calls_in(e: &Spanned<Expr>, out: &mut Vec<(String, usize)>) {
        match &e.value {
            Expr::Call { callee, args, .. } => {
                if let Expr::Ident(name) = &callee.value {
                    match out.iter_mut().find(|(n, _)| n == name) {
                        Some((_, n)) if *n != args.len() => *n = usize::MAX,
                        Some(_) => {}
                        None => out.push((name.clone(), args.len())),
                    }
                }
                for a in args {
                    if let saule_ast::CallArg::Positional(v) = a {
                        Self::calls_in(v, out);
                    }
                }
            }
            Expr::Binary { lhs, rhs, .. } => {
                Self::calls_in(lhs, out);
                Self::calls_in(rhs, out);
            }
            Expr::Unary { rhs, .. } => Self::calls_in(rhs, out),
            Expr::Match { scrutinee, arms } => {
                Self::calls_in(scrutinee, out);
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        Self::calls_in(guard, out);
                    }
                    if let saule_ast::MatchBody::Expr(body) = &arm.body {
                        Self::calls_in(body, out);
                    }
                }
            }
            _ => {}
        }
    }

    /// The names `e` matches on, at any depth, added to `out`.
    fn matched_in(e: &Spanned<Expr>, out: &mut Vec<String>) {
        match &e.value {
            Expr::Match { scrutinee, arms } => {
                if let Expr::Ident(n) = &scrutinee.value {
                    out.push(n.clone());
                }
                for arm in arms {
                    if let saule_ast::MatchBody::Expr(body) = &arm.body {
                        Self::matched_in(body, out);
                    }
                }
            }
            Expr::Binary { lhs, rhs, .. } => {
                Self::matched_in(lhs, out);
                Self::matched_in(rhs, out);
            }
            Expr::Unary { rhs, .. } => Self::matched_in(rhs, out),
            _ => {}
        }
    }

    /// Whether `e` only reads the parameters called `values`, calls the
    /// ones called `lambdas` with such expressions, and otherwise does
    /// what [`cannot_fail`](Self::cannot_fail) allows.
    fn only_chooses(&self, e: &Spanned<Expr>, values: &[String], lambdas: &[&str]) -> bool {
        match &e.value {
            Expr::Ident(n) => values.contains(n),
            Expr::Call {
                callee,
                args,
                type_args: None,
            } => {
                matches!(&callee.value, Expr::Ident(n) if lambdas.contains(&n.as_str()))
                    && args.iter().all(|a| match a {
                        saule_ast::CallArg::Positional(v) => self.only_chooses(v, values, lambdas),
                        _ => false,
                    })
            }
            // `match v case E.A(x, y) then … end` over a parameter: each
            // arm a variant with its fields named, or `_`, and a guard and
            // a body that only choose in their turn, the fields being
            // values to them. Reading a tag and a field of a value the
            // type checker proved an enum's cannot fail.
            Expr::Match { scrutinee, arms } => {
                use saule_ast::{MatchBody, Pattern};
                matches!(&scrutinee.value, Expr::Ident(n) if values.contains(n))
                    && arms.iter().all(|arm| {
                        let mut seen = values.to_vec();
                        let plain = match &arm.pattern.value {
                            Pattern::Wildcard => true,
                            Pattern::Variant { fields, .. } => {
                                fields.iter().all(|f| match &f.value {
                                    Pattern::Bind(n) if !lambdas.contains(&n.as_str()) => {
                                        seen.push(n.clone());
                                        true
                                    }
                                    Pattern::Wildcard => true,
                                    _ => false,
                                })
                            }
                            _ => false,
                        };
                        plain
                            && arm
                                .guard
                                .as_ref()
                                .is_none_or(|g| self.only_chooses(g, &seen, lambdas))
                            && matches!(&arm.body, MatchBody::Expr(body)
                                if self.only_chooses(body, &seen, lambdas))
                    })
            }
            _ => self.cannot_fail(e, values),
        }
    }

    fn cannot_fail(&self, e: &Spanned<Expr>, params: &[String]) -> bool {
        use crate::compile::ctx::Num;
        use saule_ast::{BinOp::*, UnaryOp};
        match &e.value {
            Expr::Int(_) | Expr::Float(_) | Expr::Bool(_) => true,
            Expr::Ident(n) => params.contains(n),
            Expr::Unary {
                op: UnaryOp::Neg,
                rhs,
            } => self.num_of_node(rhs).is_some() && self.cannot_fail(rhs, params),
            Expr::Binary {
                op: And | Or,
                lhs,
                rhs,
            } => self.cannot_fail(lhs, params) && self.cannot_fail(rhs, params),
            Expr::Binary { op, lhs, rhs } => {
                let kind = match (self.num_of_node(lhs), self.num_of_node(rhs)) {
                    (Some(l), Some(r)) if l == r => l,
                    _ => return false,
                };
                // Integer `//` and `%` fail on zero; float division does not.
                let ok = match op {
                    Add | Sub | Mul | Lt | LtEq | Gt | GtEq | Eq | NotEq => true,
                    Div => kind == Num::Float,
                    _ => false,
                };
                ok && self.cannot_fail(lhs, params) && self.cannot_fail(rhs, params)
            }
            _ => false,
        }
    }
}

/// See [`Compiler::static_call_plan`].
struct StaticPlan {
    m: std::rc::Rc<saule_ast::Method>,
    /// The parameters that are values, by name.
    values: Vec<String>,
    /// The parameters that are called: name, the register of the lambda
    /// given, its proto, and its body.
    lambdas: Vec<(String, u16, u32, InlineBody)>,
}

impl Compiler<'_> {
    /// `CALLSTAT` to `packed` — or `CALLSELF` when that is the static method
    /// being compiled, which needs no lookup at all: the callee is the frame
    /// already running. `fib`'s two calls are this.
    fn emit_static_call(
        &mut self,
        a: u8,
        b: u8,
        c: u8,
        packed: u32,
        span: &std::ops::Range<usize>,
    ) {
        if self.f.self_static == Some(packed) {
            self.emit(Instruction::abc(Op::CALLSELF, a, b, c), span);
            return;
        }
        self.emit(Instruction::abc(Op::CALLSTAT, a, b, c), span);
        self.emit(Instruction::ax_of(Op::EXTRAARG, packed), span);
    }
}
