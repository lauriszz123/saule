//! Binding arguments to parameters, and values to declared types (§19).
//!
//! Saule resolves named and defaulted arguments at *compile* time, so a
//! call site emits a plain positional call. Declared types are the other
//! half: an `integer` parameter handed a `float` gets its conversion
//! emitted here rather than tested for at run time.

use std::ops::Range;
use std::rc::Rc;

use crate::compile::CompileError;
use crate::op::{Instruction, Op};

use super::Compiler;

impl Compiler<'_> {

    /// Emit per-arity entry stubs for a callee's defaulted parameters
    /// (§19), and hand back the `entries` table.
    ///
    /// Call sites do nothing for a default: `CALLK` pushes a frame and jumps
    /// to `proto.entry_for(n_args)`, and the stub the arity lands on fills in
    /// what was not passed. Zero cost when every argument was supplied,
    /// because that entry point *is* the body.
    ///
    /// **Defaults are evaluated in the callee's frame**, which §19 calls the
    /// one genuine correctness trap here. Stubs get that right by
    /// construction: parameter `k`'s default compiles into register `k` of
    /// the callee, so a default that mentions an earlier parameter —
    /// `fn f(a: integer, b: integer = a * 2)` — reads the register holding
    /// `a`, exactly as `bind_params` reads the callee's scope. Compiling the
    /// default at the *call site* instead would have resolved `a` against
    /// the caller.
    ///
    /// The stubs fall through into one another, so entering at arity `k`
    /// fills `k`, `k+1`, … and then runs the body — one instruction stream,
    /// no jumps.
    /// `base` is the register (and argument index) the *first* written
    /// parameter occupies: 1 for an instance method, whose register 0 is
    /// `self` and whose `self` counts as an argument, and 0 otherwise.
    pub fn param_entries(
        &mut self,
        params: &[saule_ast::Param],
        base: u16,
        span: &Range<usize>,
    ) -> Result<Vec<u32>, CompileError> {
        // A variadic parameter gathers the surplus arguments into a table,
        // and it has to happen however the frame was entered — so it is the
        // proto's *first* instruction, before any default stub.
        if let Some(i) = params.iter().position(|p| p.variadic) {
            // `bind_params` rejects both of these at run time; refusing here
            // keeps the compiler from emitting code for a signature the
            // other engine would not accept.
            if i + 1 != params.len() || params.iter().filter(|p| p.variadic).count() > 1 {
                return Err(CompileError::unsupported(
                    "a variadic parameter that is not the last one",
                    span.clone(),
                ));
            }
            // Mixing the two would need an entry stub per arity that also
            // re-gathers, and nothing in the corpus does it.
            if params.iter().any(|p| p.default.is_some()) {
                return Err(CompileError::unsupported(
                    "a signature with both a default and a variadic parameter",
                    span.clone(),
                ));
            }
            let a = self.reg8(base + i as u16, span)?;
            self.emit(Instruction::abc(Op::VARARG, a, 0, 0), span);
            self.f.variadic_param = Some(Rc::from(params[i].name.as_str()));
            // No defaults, so every arity enters at 0 — which is the
            // `VARARG` just emitted.
            return Ok(Vec::new());
        }
        let Some(first_default) = params.iter().position(|p| p.default.is_some()) else {
            // Nothing defaulted: `entry_for` falls back to 0 for every
            // arity, which is the body.
            return Ok(Vec::new());
        };
        let base = base as usize;
        let mut entries = vec![0u32; base + params.len() + 1];
        for (k, p) in params.iter().enumerate().skip(first_default) {
            entries[base + k] = self.f.label_here() as u32;
            // A parameter after the first defaulted one with no default of
            // its own keeps whatever `push_frame` left — `nil` — which is
            // what a nullable parameter should get anyway (§19 step 5).
            if let Some(d) = &p.default {
                self.expr_to(d, (base + k) as u16)?;
            }
        }
        // Every arity at or below the first default enters the same place:
        // a call missing a *required* argument is a typecheck error, so this
        // only decides what an unchecked caller sees, and "fill every
        // default" is the sane answer.
        let first_pc = entries[base + first_default];
        for e in entries.iter_mut().take(base + first_default) {
            *e = first_pc;
        }
        // Full arity: straight to the body.
        entries[base + params.len()] = self.f.label_here() as u32;
        let _ = span;
        Ok(entries)
    }


    /// Coerce every parameter whose declared type asks for it (§`coerce.rs`).
    ///
    /// **Emitted after the default entry stubs, and that placement is the
    /// point.** `entries[n_params]` is recorded at the end of
    /// `param_entries`, so a full-arity call enters exactly here, and the
    /// stubs for the lower arities fall through into it. One copy therefore
    /// covers every way the frame can be entered — a coercion at pc 0 would
    /// be jumped straight over by any call that lands on a stub.
    ///
    /// `base` matches `param_entries`: 1 for an instance method, whose
    /// register 0 is `self`.
    pub fn coerce_params(
        &mut self,
        params: &[saule_ast::Param],
        base: u16,
        span: &Range<usize>,
    ) -> Result<(), CompileError> {
        for (i, p) in params.iter().enumerate() {
            // A variadic parameter is a table of the surplus arguments, not
            // a slot with a declared element type, so it never coerces.
            if p.variadic {
                continue;
            }
            self.coerce_to_declared(base + i as u16, Some(&p.ty), span)?;
        }
        Ok(())
    }

    /// `Assignable<T>`: build the declared class from a bare value, in place.
    ///
    /// The tree-walker does this in `eval/coerce.rs::to_declared`, and this
    /// is that function expressed as code rather than a call — the one place
    /// the "reuse rather than reimplement" rule cannot be followed, because
    /// `to_declared` needs an `Environment` to resolve the class name and the
    /// VM has a class *table* instead. So the decisions it makes at run time
    /// are made here at compile time, and only the two it cannot make
    /// statically are emitted as branches:
    ///
    /// * a declared type that is not a `Named` class, a class that does not
    ///   implement `Assignable`, or one with no `of` static — decided here,
    ///   emitting nothing at all;
    /// * `nil`, which fills a nullable slot on its own terms, and a value
    ///   that is *already* an instance of the class — the overwhelmingly
    ///   common case, and the one `to_declared` also keeps free.
    ///
    /// `Type::Nullable` is stripped first: `Text?` names the same target for
    /// a non-nil value.
    ///
    /// Emits nothing when no conversion can apply, so callers wrap a binding
    /// unconditionally, exactly as they do in the tree-walker.
    pub fn coerce_to_declared(
        &mut self,
        r: u16,
        declared: Option<&saule_ast::Type>,
        span: &Range<usize>,
    ) -> Result<(), CompileError> {
        fn strip(t: &saule_ast::Type) -> &saule_ast::Type {
            match t {
                saule_ast::Type::Nullable(inner) => strip(inner),
                other => other,
            }
        }
        let Some(saule_ast::Type::Named(name)) = declared.map(strip) else {
            return Ok(());
        };
        // A module-level `local Text = {…}` must not make the class's `of`
        // fire on a slot declared as that local's type. Trap 1.
        if !self.not_shadowed(name) {
            return Ok(());
        }
        let Some(idx) = self.layouts.get(name) else {
            return Ok(());
        };
        if !self.chunk.classes[idx as usize].assignable {
            return Ok(());
        }
        let Some((sclass, sslot)) = self.static_method_of(name, saule_ast::ops::ASSIGNABLE.method)
        else {
            return Ok(());
        };

        let ra = self.reg8(r, span)?;
        // nil → leave it alone. `JNIL` skips the next instruction when the
        // value is nil, so the jump past the bail-out is what runs for a
        // *non*-nil value.
        self.emit(Instruction::abc(Op::JNIL, ra, 0, 0), span);
        let not_nil = self.emit_jump(Op::JMP, 0, span);
        let keep_nil = self.emit_jump(Op::JMP, 0, span);
        self.patch_here(not_nil)?;

        // Already an instance of the class → leave it alone.
        let m = self.mark();
        let isa = self.alloc(span)?;
        let ia = self.reg8(isa, span)?;
        let cidx = u8::try_from(idx).map_err(|_| CompileError::Unsupported {
            thing: "an `Assignable` class past the 256th in a program",
            span: span.clone(),
        })?;
        self.emit(Instruction::abc(Op::ISA, ia, ra, cidx), span);
        // `TEST` skips the next instruction when truthiness matches `C`, so
        // with `C = 0` an instance skips the jump into the conversion.
        self.emit(Instruction::abc(Op::TEST, ia, 0, 0), span);
        let convert = self.emit_jump(Op::JMP, 0, span);
        let keep_isa = self.emit_jump(Op::JMP, 0, span);
        self.free_to(m);
        self.patch_here(convert)?;

        // `C.of(value)` — an ordinary static call, into a fresh window whose
        // single result lands back in `r`.
        let m2 = self.mark();
        let base = self.alloc_n(1, span)?;
        self.move_result(r, base, span)?;
        let ba = self.reg8(base, span)?;
        self.emit(Instruction::abc(Op::CALLSTAT, ba, 2, 2), span);
        self.emit(
            Instruction::ax_of(Op::EXTRAARG, (sclass << 16) | sslot as u32),
            span,
        );
        self.move_result(base, r, span)?;
        self.free_to(m2);

        self.patch_here(keep_nil)?;
        self.patch_here(keep_isa)?;
        Ok(())
    }

}
