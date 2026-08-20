//! `a.b` and `a[b]`: reads, writes, and what a member turned out to be.
//!
//! A member access is only an indexed load when the receiver's class was
//! proved. Everything else — an unproved receiver, a table key, a static
//! promoted out of a field default — takes a different opcode, and
//! [`MemberAccess`] is the resolved answer that picks it.

use std::ops::Range;

use saule_ast::{Expr, Spanned};
use saule_interpreter::Value;

use super::CompileError;
use super::super::ctx::Compiler;
use crate::op::{Instruction, Op};

/// What `obj.name` turned out to be, once resolved against a proved class.
///
/// A class-level field with a default and no `init` is promoted to a static
/// by both engines, so "member read" covers both shapes and the difference
/// is which opcode loads it.
#[derive(Debug, Clone, Copy)]
pub(crate) enum MemberAccess {
    Field(u16),
    Static(crate::chunk::StaticSlot),
}

impl Compiler<'_> {

    /// `obj.name` — a field read, or a static read off a class name.
    pub(crate) fn member_to(
        &mut self,
        e: &Spanned<Expr>,
        obj: &Spanned<Expr>,
        name: &str,
        dst: u16,
    ) -> Result<(), CompileError> {
        let span = &e.span;
        let a = self.reg8(dst, span)?;

        // `self.count` inside a `static fn`. There, `self` denotes the
        // **class** — `call_static_method_multi` binds it to
        // `Value::Class` — so this is a static read, resolved at compile
        // time. Which also means the VM never needs a class in a register:
        // `self` bare in a static method is still refused, and no fixture
        // asks for it.
        if matches!(obj.value, Expr::Self_)
            && !self.f.in_method
            && let Some(class) = self.f.current_class
            && let Some(&s) = self.chunk.classes[class as usize].sindex.get(name)
        {
            self.emit(
                Instruction::abc(Op::GETSTAT, a, s.class as u8, s.slot as u8),
                span,
            );
            return Ok(());
        }

        // `Status.Alive` — a singleton variant reference.
        if let Expr::Ident(en) = &obj.value
            && self.not_shadowed(en)
            && let Some(e_idx) = self.layouts.enum_of(en)
            && let Some(&tag) = self.chunk.enums[e_idx as usize].by_name.get(name)
        {
            return self.variant_ref_to(e_idx, tag, dst, span);
        }

        // `Counter.total` — the receiver is a type name, not a value. No
        // subclass ambiguity here: the class is named outright.
        if let Some(class) = self.class_named_by(obj)
            && let Some(&s) = self.chunk.classes[class as usize].sindex.get(name)
        {
            self.emit(
                Instruction::abc(Op::GETSTAT, a, s.class as u8, s.slot as u8),
                span,
            );
            return Ok(());
        }

        // `Math.pi`, `Os.sep`, `IoMode.Write` — a stdlib member holding a
        // value. The prelude is fixed before a program runs, so this is
        // resolved to the value itself and the read costs one `LOADK` —
        // the same compile-time resolution `String.len` already gets on the
        // call path, applied to members that are not functions.
        //
        // Gated on the **resolver's** classification, not on a hand-rolled
        // "is it a local, a class, an enum" check. A top-level `local Math`
        // is a *module slot* rather than a frame local, so `f.lookup` misses
        // it — and folding then read the stdlib's `pi` where the program
        // meant its own table. The resolver already answers "what is this
        // name", and `Prelude` is the only answer that may be folded.
        if let Expr::Ident(recv) = &obj.value
            && !self.mutated_receivers.contains(recv)
            && let Some(v) = self.prelude_member(obj.id, recv, name)
        {
            let k = self.constant(v, span)?;
            self.emit(Instruction::abx(Op::LOADK, a, k), span);
            return Ok(());
        }

        // `variant.value` — the payload a valued variant carries. Not a
        // field: an enum variant has no layout, so this is `UNWRAP`.
        if name == "value"
            && self
                .ty_name(obj.id)
                .is_some_and(|t| self.layouts.enum_of(t).is_some())
        {
            let m = self.mark();
            let r = self.expr_tmp(obj)?;
            let b = self.reg8(r, span)?;
            self.emit(Instruction::abc(Op::UNWRAP, a, b, 0), span);
            self.free_to(m);
            return Ok(());
        }

        // `p.health` where the front end proved `p`'s class: one indexed
        // load. Without a proved class there is no safe slot to use — a
        // wrong one reads a different field silently — so it is refused
        // rather than guessed.
        // `t.foo` on a table is `t["foo"]` — Lua-style sugar, and a miss is
        // `nil` rather than an error, so it is a safe probe (`members.rs`).
        // Nothing to do with field slots.
        if matches!(self.types.get(&obj.id), Some(saule_ast::Type::Table { .. })) {
            let m = self.mark();
            let r = self.expr_tmp(obj)?;
            let key = self.constant(Value::Str(std::rc::Rc::new(name.to_string())), span)?;
            self.map_key_read(dst, r, key, span)?;
            self.free_to(m);
            return Ok(());
        }

        // No proved class: `GETFX` asks the tree-walker's own `read_member`
        // at run time (§8.5). A missing type selects the dynamic form; it is
        // never a wrong opcode.
        let Some(class) = self.class_of_expr(obj) else {
            let m = self.mark();
            let r = self.expr_tmp(obj)?;
            let key = self.constant(Value::Str(std::rc::Rc::new(name.to_string())), span)?;
            let Ok(kc) = u8::try_from(key) else {
                return Err(CompileError::unsupported(
                    "a dynamic member name past the 256-constant window",
                    span.clone(),
                ));
            };
            let (a, b) = (self.reg8(dst, span)?, self.reg8(r, span)?);
            self.emit(Instruction::abc(Op::GETFX, a, b, kc), span);
            self.free_to(m);
            return Ok(());
        };
        let Some(access) = self.member_access(class, name) else {
            return Err(CompileError::unsupported(
                "a member that is neither an instance field nor a static",
                span.clone(),
            ));
        };
        // The receiver is evaluated even when the answer is a static: the
        // tree-walker evaluates it too, and it may have side effects.
        //
        // A receiver already in a register is read where it is. One operand,
        // so there is nothing evaluated between the read and the load and
        // the purity question `binary_to` has to ask does not arise here.
        // This is `oop`'s hottest pair: `MOVE r 0` then `GETF`, 2,000,002
        // times, for a `self` that never left register 0.
        let in_place = self.operand_is_pure(obj);
        let m = self.mark();
        let r = self.operand_to_reg(obj, in_place)?;
        let b = self.reg8(r, span)?;
        self.emit(self.member_load(access, a, b), span);
        self.free_to(m);
        Ok(())
    }

    /// `R[dst] := R[table][K[key]]` — a table read on a constant key.
    ///
    /// `GETMAPK` carries the constant index in its 8-bit `C`, so a key
    /// interned past 255 does not fit. Rather than cap a module at 256
    /// constants — which any real program reaches, on an operation as
    /// ordinary as `t.name` — the key is materialised into a register and
    /// the general `GETIDX` does the read. One extra instruction, no cliff.
    pub(crate) fn map_key_read(
        &mut self,
        dst: u16,
        table: u16,
        key: u16,
        span: &Range<usize>,
    ) -> Result<(), CompileError> {
        let (a, b) = (self.reg8(dst, span)?, self.reg8(table, span)?);
        if let Ok(c) = u8::try_from(key) {
            self.emit(Instruction::abc(Op::GETMAPK, a, b, c), span);
            return Ok(());
        }
        let m = self.mark();
        let k = self.alloc(span)?;
        let kr = self.reg8(k, span)?;
        self.emit(Instruction::abx(Op::LOADK, kr, key), span);
        self.emit(Instruction::abc(Op::GETIDX, a, b, kr), span);
        self.free_to(m);
        Ok(())
    }

    /// `R[table][K[key]] := R[value]`, with the same 8-bit escape hatch as
    /// [`Self::map_key_read`] — `SETMAPK` holds the key index in `B`.
    pub(crate) fn map_key_write(
        &mut self,
        table: u16,
        key: u16,
        value: u16,
        span: &Range<usize>,
    ) -> Result<(), CompileError> {
        let (a, c) = (self.reg8(table, span)?, self.reg8(value, span)?);
        if let Ok(b) = u8::try_from(key) {
            self.emit(Instruction::abc(Op::SETMAPK, a, b, c), span);
            return Ok(());
        }
        let m = self.mark();
        let k = self.alloc(span)?;
        let kr = self.reg8(k, span)?;
        self.emit(Instruction::abx(Op::LOADK, kr, key), span);
        self.emit(Instruction::abc(Op::SETIDX, a, kr, c), span);
        self.free_to(m);
        Ok(())
    }

    /// How `obj.name` resolves against a class the front end proved.
    ///
    /// `None` means "refuse", never "guess" — including the one case where
    /// the static answer would depend on the receiver's *runtime* class
    /// rather than its declared one.
    pub(crate) fn member_access(
        &self,
        class: crate::chunk::ClassIdx,
        name: &str,
    ) -> Option<MemberAccess> {
        let proto = &self.chunk.classes[class as usize];
        if let Some(slot) = proto.layout.slot(name) {
            return Some(MemberAccess::Field(slot));
        }
        let s = *proto.sindex.get(name)?;
        // A class-level default with no `init` is a *static*, so `b.label`
        // is a legitimate static read through an instance. The slot is
        // resolved against the receiver's declared class — which is the
        // right cell unless some subclass redeclares the name, since then
        // the answer depends on what the receiver actually is at run time.
        if self.a_subclass_shadows_static(class, name, s) {
            return None;
        }
        Some(MemberAccess::Static(s))
    }

    /// Whether any descendant of `class` declares its own `name`, making a
    /// statically resolved static read ambiguous.
    fn a_subclass_shadows_static(
        &self,
        class: crate::chunk::ClassIdx,
        name: &str,
        resolved: crate::chunk::StaticSlot,
    ) -> bool {
        self.chunk.classes.iter().enumerate().any(|(i, c)| {
            i as u32 != class
                && c.sindex.get(name).is_some_and(|s| *s != resolved)
                && self.descends_from(i as u32, class)
        })
    }

    fn descends_from(&self, mut who: crate::chunk::ClassIdx, ancestor: crate::chunk::ClassIdx) -> bool {
        while let Some(p) = self.chunk.classes[who as usize].parent {
            if p == ancestor {
                return true;
            }
            who = p;
        }
        false
    }

    /// The instruction that loads a resolved member into `a` from a
    /// receiver in `b`.
    pub(crate) fn member_load(&self, access: MemberAccess, a: u8, b: u8) -> Instruction {
        match access {
            MemberAccess::Field(slot) => Instruction::abc(Op::GETF, a, b, slot as u8),
            MemberAccess::Static(s) => {
                Instruction::abc(Op::GETSTAT, a, s.class as u8, s.slot as u8)
            }
        }
    }

    /// `obj[index]`.
    ///
    /// `GETIDX` is a *table* read despite §15.9 calling it the dynamic form,
    /// so an instance receiver needs its `OpIndex` overload resolved here —
    /// for the same reason the arithmetic overloads are (§8.7): a bytecode
    /// method never reaches the runtime `ClassObject`'s method map, so no
    /// run-time lookup could find it.
    pub(crate) fn index_to(
        &mut self,
        e: &Spanned<Expr>,
        obj: &Spanned<Expr>,
        index: &Spanned<Expr>,
        dst: u16,
    ) -> Result<(), CompileError> {
        let span = &e.span;

        if let Some(class) = self.class_of_expr(obj) {
            let contract = saule_ast::ops::OP_INDEX;
            let Some(&slot) = self.chunk.classes[class as usize]
                .vindex
                .get(contract.method)
            else {
                // A class receiver with no `index` method: `GETIDX` would
                // report "expected `table`", blaming the compiler for a
                // program error. The tree-walker has the right message.
                return Err(CompileError::unsupported(
                    "an index read on a class with no `OpIndex` overload",
                    span.clone(),
                ));
            };
            let m = self.mark();
            let base = self.alloc_n(2, span)?;
            self.expr_to(obj, base)?;
            self.expr_to(index, base + 1)?;
            let a = self.reg8(base, span)?;
            self.emit(Instruction::abc(Op::CALLM, a, 2, slot as u8), span);
            self.move_result(base, dst, span)?;
            self.free_to(m);
            return Ok(());
        }

        // `t[i]` on two things already in registers is the shape every
        // array-style loop is made of, and it emitted two `MOVE`s per read.
        let in_place = self.operand_is_pure(obj) && self.operand_is_pure(index);
        let m = self.mark();
        let o = self.operand_to_reg(obj, in_place)?;
        let i = self.operand_to_reg(index, in_place)?;
        let (a, b, c) = (
            self.reg8(dst, span)?,
            self.reg8(o, span)?,
            self.reg8(i, span)?,
        );
        self.emit(Instruction::abc(Op::GETIDX, a, b, c), span);
        self.free_to(m);
        Ok(())
    }
}
