//! A name in a value position.
//!
//! Every name has already been classified by the resolver; this is where
//! that classification becomes an opcode — a register move for a local, a
//! `GETUPVAL`, a `GETMOD`, a `GETSTAT`, or a folded constant for a prelude
//! value that is known at compile time.

use saule_ast::{Expr, Spanned};
use saule_semantic::Binding;

use super::CompileError;
use super::super::ctx::Compiler;
use crate::op::{Instruction, Op};

impl Compiler<'_> {

    pub(crate) fn ident_to(
        &mut self,
        e: &Spanned<Expr>,
        name: &str,
        dst: u16,
    ) -> Result<(), CompileError> {
        let span = &e.span;
        let a = self.reg8(dst, span)?;
        // The resolver already decided what this name is; the compiler only
        // decides which register holds it (see `ctx`'s module docs).
        match self.binding(e.id) {
            Some(Binding::Local { .. }) => {
                let src = self.f.lookup(name).ok_or_else(|| CompileError::Unsupported {
                    thing: "a local the compiler has not seen declared",
                    span: span.clone(),
                })?;
                let b = self.reg8(src, span)?;
                if a != b {
                    self.emit(Instruction::abc(Op::MOVE, a, b, 0), span);
                }
                Ok(())
            }
            Some(Binding::Module { slot }) => {
                let slot = *slot;
                // A top-level `local` inside a block is an ordinary local of
                // the module body, and the resolver says so — but a name it
                // classified as a module slot may still be one this function
                // holds in a register, when we *are* the module body.
                match self.f.lookup(name) {
                    Some(src) => {
                        let b = self.reg8(src, span)?;
                        if a != b {
                            self.emit(Instruction::abc(Op::MOVE, a, b, 0), span);
                        }
                    }
                    None => {
                        // Reading a module slot the body has not reached
                        // yet. `GETMOD` would load whatever the slot holds
                        // — `nil`, since no `SETMOD` has run — where the
                        // tree-walker finds the name undefined and errors.
                        // Exact, not conservative: the declaration either
                        // has been passed or it has not.
                        if self.enclosing.is_empty() && !self.module_decls_seen.contains(name) {
                            return Err(CompileError::unsupported(
                                "a module-level read of a name declared further down",
                                span.clone(),
                            ));
                        }
                        let g = self.mod_slot(slot, span)?;
                        self.emit(Instruction::abx(Op::GETMOD, a, g), span)
                    }
                }
                Ok(())
            }
            Some(Binding::Prelude { .. }) => {
                // A prelude name in a *value* position — `Io.stdout`, where
                // the member is an object rather than a scalar and so is not
                // one `prelude_member` folds, or a bare `print` assigned to
                // a local. The prelude is fixed before a program runs, so
                // the entity itself is a constant: one `LOADK`, and every
                // read or call through it is then the ordinary dynamic
                // member path (`GETFX` / `CALLMX`), which defers to the
                // tree-walker's own `read_member`. The same compile-time
                // resolution `CALLNAT` gets, applied to the name rather than
                // to the call.
                //
                // `static_value` is what declines a shadowed name, so a
                // module-level `local Io = {…}` still reads the program's
                // own table — trap 1, and the reason this does not test for
                // `Binding::Prelude` and stop there.
                //
                // Declined for a receiver this module **assigns through**,
                // the same guard `prelude_member`'s fold carries and for the
                // same reason: the compiler and the tree-walker install
                // their own prelude, so `install` builds a *different*
                // `ClassObject` for each. Reads and writes that all go
                // through the folded constant stay consistent with one
                // another, but `Math.pi = 3.0` would then be writing into an
                // object only the VM can see. Refusing the module keeps the
                // two engines agreeing —
                // `a_reassigned_stdlib_constant_is_not_folded` pins it.
                let folded = (!self.mutated_receivers.contains(name))
                    .then(|| self.static_value(e.id, name))
                    .flatten();
                match folded {
                    Some(v) => {
                        let k = self.constant(v, span)?;
                        self.emit(Instruction::abx(Op::LOADK, a, k), span);
                        Ok(())
                    }
                    None => Err(CompileError::unsupported(
                        "a prelude name outside a call",
                        span.clone(),
                    )),
                }
            }
            Some(Binding::Upvalue { .. }) => {
                // A lambda calling itself by the name it is being bound to.
                // The enclosing frame has no register for it yet — `local`
                // declares one only *after* the initializer is compiled —
                // so there is nothing to capture, and capturing it once
                // there were would close an `Rc` cycle per call. `SELFFUNC`
                // reads the running closure off the frame instead, which is
                // what the tree-walker's `FunctionObject::self_name` does.
                if self.f.self_fn_name.as_deref() == Some(name) {
                    self.emit(Instruction::abc(Op::SELFFUNC, a, 0, 0), span);
                    return Ok(());
                }
                // The resolver proved this crosses a function boundary; the
                // index is ours to assign, and asking for it builds the
                // capture chain lazily.
                let idx = self.capture_upvalue(name).ok_or_else(|| CompileError::Unsupported {
                    thing: "a captured variable the compiler could not locate",
                    span: span.clone(),
                })?;
                let b = self.reg8(idx, span)?;
                self.emit(Instruction::abc(Op::GETUPVAL, a, b, 0), span);
                Ok(())
            }
            // A static of the enclosing class, reached by its bare name from
            // inside a method. The resolver carries the *class* name here
            // rather than a slot, because the answer has to survive a lambda
            // nested inside the method — which is a different `FuncCtx` with
            // no `current_class` of its own.
            Some(Binding::ClassStatic { class, name: field }) => {
                let (class, field) = (class.clone(), field.clone());
                let Some(s) = self.static_slot_of(&class, &field) else {
                    return Err(CompileError::unsupported(
                        "a class static the compiler could not resolve",
                        span.clone(),
                    ));
                };
                // `s.class`, not the class we are *in*: an inherited static
                // lives in the cell its declaring class owns, so a sibling
                // reading the bare name sees the same value
                // (`declaring_static_field`).
                self.emit(
                    Instruction::abc(Op::GETSTAT, a, s.class as u8, s.slot as u8),
                    span,
                );
                Ok(())
            }
            Some(Binding::SelfRef) => {
                Err(CompileError::unsupported("`self`", span.clone()))
            }
            Some(Binding::WildcardImport) | None => Err(CompileError::unsupported(
                "a name the resolver could not classify",
                span.clone(),
            )),
        }
    }
}
