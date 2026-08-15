//! Enum references and `match` compilation (`VM_DESIGN.md` §9).
//!
//! ## Why `match` is worth a jump table
//!
//! `match` is a primary control structure in Saule, and the tree-walker
//! evaluates it as a linear chain of pattern tests that compare the enum
//! *name* and the variant *name* as strings, once per arm. When every arm is
//! a variant of one enum — the dominant shape — this compiles instead to:
//!
//! ```text
//!   GETTAG  r4 r2
//!   SWITCH  r4 T0
//! ```
//!
//! O(1) instead of O(arms), and no string ever compared. That is only
//! possible because Phase 0.4 gave every variant a **dense** tag in
//! declaration order.
//!
//! Anything that does not fit that shape — literal patterns, guards, a
//! wildcard mixed in — falls back to a test chain. Correctness never depends
//! on the jump table firing.

use std::ops::Range;

use saule_ast::{Expr, MatchArm, MatchBody, Pattern, Spanned};

use super::CompileError;
use super::ctx::Compiler;
use crate::chunk::JumpTable;
use crate::op::{Instruction, Op};

impl Compiler<'_> {
    /// `Enum.Variant` — a singleton reference.
    pub fn variant_ref_to(
        &mut self,
        e_idx: u32,
        tag: u32,
        dst: u16,
        span: &Range<usize>,
    ) -> Result<(), CompileError> {
        let a = self.reg8(dst, span)?;
        let idx = match self
            .chunk
            .variant_refs
            .iter()
            .position(|r| *r == (e_idx, tag))
        {
            Some(i) => i,
            None => {
                self.chunk.variant_refs.push((e_idx, tag));
                self.chunk.variant_refs.len() - 1
            }
        };
        let bx = u16::try_from(idx).map_err(|_| CompileError::Unsupported {
            thing: "a module with more than 65536 variant references",
            span: span.clone(),
        })?;
        self.emit(Instruction::abx(Op::VARIANT, a, bx), span);
        Ok(())
    }

    /// `Enum.Variant(args)` — a fresh tuple variant.
    pub fn variant_ctor_to(
        &mut self,
        e_idx: u32,
        tag: u32,
        args: &[&Spanned<Expr>],
        dst: u16,
        span: &Range<usize>,
    ) -> Result<(), CompileError> {
        let m = self.mark();
        let base = self.alloc_n(args.len() as u16 + 1, span)?;
        for (i, arg) in args.iter().enumerate() {
            self.expr_to(arg, base + 1 + i as u16)?;
        }
        let a = self.reg8(base, span)?;
        self.emit(
            Instruction::abc(Op::NEWVAR, a, args.len() as u8 + 1, 0),
            span,
        );
        self.emit(
            Instruction::ax_of(Op::EXTRAARG, (e_idx << 16) | tag),
            span,
        );
        self.move_result(base, dst, span)?;
        self.free_to(m);
        Ok(())
    }

    /// A `match` expression.
    pub fn match_to(
        &mut self,
        e: &Spanned<Expr>,
        scrutinee: &Spanned<Expr>,
        arms: &[MatchArm],
        dst: u16,
    ) -> Result<(), CompileError> {
        let span = &e.span;
        let m = self.mark();
        let sc = self.expr_tmp(scrutinee)?;

        let result = match self.switchable(arms) {
            Some(e_idx) => self.match_switch(e_idx, sc, arms, dst, span),
            None => self.match_chain(sc, arms, dst, span),
        };
        self.free_to(m);
        result
    }

    /// The enum every arm dispatches on, when the whole `match` is a plain
    /// variant switch: no guards, no literals, and every arm a distinct
    /// variant of one enum. A trailing wildcard is allowed and becomes the
    /// table's default.
    fn switchable(&self, arms: &[MatchArm]) -> Option<u32> {
        let mut found: Option<u32> = None;
        for (i, arm) in arms.iter().enumerate() {
            if arm.guard.is_some() {
                return None;
            }
            match &arm.pattern.value {
                Pattern::Variant { enum_name, .. } => {
                    let e = self.layouts.enum_of(enum_name)?;
                    match found {
                        Some(prev) if prev != e => return None,
                        _ => found = Some(e),
                    }
                }
                // A wildcard is fine only as the last arm, where it is the
                // default; anywhere else it would shadow the arms after it.
                Pattern::Wildcard if i + 1 == arms.len() => {}
                _ => return None,
            }
        }
        found
    }

    fn match_switch(
        &mut self,
        e_idx: u32,
        sc: u16,
        arms: &[MatchArm],
        dst: u16,
        span: &Range<usize>,
    ) -> Result<(), CompileError> {
        let n_variants = self.chunk.enums[e_idx as usize].variants.len();
        let tag_reg = self.alloc(span)?;
        let (ta, sa) = (self.reg8(tag_reg, span)?, self.reg8(sc, span)?);
        self.emit(Instruction::abc(Op::GETTAG, ta, sa, 0), span);

        let table_idx = self.chunk.jump_tables.len() as u16;
        self.chunk.jump_tables.push(JumpTable {
            targets: vec![0; n_variants],
            default: 0,
        });
        self.emit(Instruction::abx(Op::SWITCH, ta, table_idx), span);
        // The switch always jumps, so nothing falls through to here; the
        // placeholder is replaced once every arm's entry point is known.
        let mut to_end = Vec::new();
        let mut targets = vec![usize::MAX; n_variants];
        let mut default = usize::MAX;

        for arm in arms {
            let entry = self.f.label_here();
            match &arm.pattern.value {
                Pattern::Variant {
                    variant, fields, ..
                } => {
                    let tag = self.chunk.enums[e_idx as usize]
                        .by_name
                        .get(variant.as_str())
                        .copied()
                        .ok_or_else(|| CompileError::Unsupported {
                            thing: "a variant the enum does not declare",
                            span: arm.span.clone(),
                        })?;
                    targets[tag as usize] = entry;
                    self.f.enter_scope();
                    self.bind_payload(sc, fields, &arm.span)?;
                    self.arm_body(&arm.body, dst)?;
                    self.f.leave_scope();
                }
                _ => {
                    default = entry;
                    self.f.enter_scope();
                    self.arm_body(&arm.body, dst)?;
                    self.f.leave_scope();
                }
            }
            to_end.push(self.emit_jump(Op::JMP, 0, span));
        }

        let end = self.f.label_here();
        for l in to_end {
            self.patch_here(l)?;
        }
        // An unmatched tag with no wildcard falls past the whole `match`.
        // `saule-typeck` already proved exhaustiveness, so this is only
        // reachable for an unanalysed module.
        let default = if default == usize::MAX { end } else { default };
        let table = &mut self.chunk.jump_tables[table_idx as usize];
        table.default = default as u32;
        for (i, t) in targets.iter().enumerate() {
            table.targets[i] = if *t == usize::MAX { default } else { *t } as u32;
        }
        Ok(())
    }

    /// The general form: test each arm in turn.
    fn match_chain(
        &mut self,
        sc: u16,
        arms: &[MatchArm],
        dst: u16,
        span: &Range<usize>,
    ) -> Result<(), CompileError> {
        let mut to_end = Vec::new();
        for arm in arms {
            let next = self.arm_test(sc, arm, span)?;
            self.f.enter_scope();
            if let Pattern::Variant { fields, .. } = &arm.pattern.value {
                self.bind_payload(sc, fields, &arm.span)?;
            }
            if let Pattern::Bind(name) = &arm.pattern.value {
                let r = self.alloc(span)?;
                let (a, b) = (self.reg8(r, span)?, self.reg8(sc, span)?);
                self.emit(Instruction::abc(Op::MOVE, a, b, 0), span);
                self.f.declare(name, r);
            }
            self.arm_body(&arm.body, dst)?;
            self.f.leave_scope();
            to_end.push(self.emit_jump(Op::JMP, 0, span));
            if let Some(l) = next {
                self.patch_here(l)?;
            }
        }
        for l in to_end {
            self.patch_here(l)?;
        }
        Ok(())
    }

    /// Emit the test for one arm; the returned label jumps to the next arm
    /// when it does not match. `None` for a pattern that always matches.
    fn arm_test(
        &mut self,
        sc: u16,
        arm: &MatchArm,
        span: &Range<usize>,
    ) -> Result<Option<super::ctx::Label>, CompileError> {
        let sa = self.reg8(sc, span)?;
        let jump = match &arm.pattern.value {
            Pattern::Wildcard | Pattern::Bind(_) => None,
            Pattern::Variant {
                enum_name, variant, ..
            } => {
                let e = self.layouts.enum_of(enum_name).ok_or_else(|| {
                    CompileError::unsupported("a variant of an unknown enum", span.clone())
                })?;
                let tag = self.chunk.enums[e as usize]
                    .by_name
                    .get(variant.as_str())
                    .copied()
                    .ok_or_else(|| CompileError::Unsupported {
                        thing: "a variant the enum does not declare",
                        span: span.clone(),
                    })?;
                // `JIFTAG` skips the following jump when the tag matches, so
                // the jump is taken exactly when this arm does not apply.
                self.emit(Instruction::abc(Op::JIFTAG, sa, tag as u8, 0), span);
                Some(self.emit_jump(Op::JMP, 0, span))
            }
            Pattern::Int(_) | Pattern::Float(_) | Pattern::Bool(_) | Pattern::Str(_)
            | Pattern::Nil => {
                let m = self.mark();
                let lit = self.alloc(span)?;
                self.pattern_literal_to(&arm.pattern.value, lit, span)?;
                let (la, _) = (self.reg8(lit, span)?, ());
                self.emit(Instruction::abc(Op::JEQ, sa, la, 0), span);
                let l = self.emit_jump(Op::JMP, 0, span);
                self.free_to(m);
                Some(l)
            }
            Pattern::Tuple(_) => {
                return Err(CompileError::unsupported(
                    "a tuple pattern",
                    span.clone(),
                ));
            }
        };

        // A guard runs only after the pattern matched, and failing it moves
        // on to the next arm exactly as a failed pattern does.
        if let Some(g) = &arm.guard {
            let m = self.mark();
            let r = self.expr_tmp(g)?;
            let a = self.reg8(r, span)?;
            self.emit(Instruction::abc(Op::TEST, a, 0, 0), span);
            let l = self.emit_jump(Op::JMP, 0, span);
            self.free_to(m);
            // Two ways to reach the next arm; the pattern's jump is patched
            // to the same place by the caller, so chain them.
            if let Some(first) = jump {
                self.patch_here(first)?;
            }
            return Ok(Some(l));
        }
        Ok(jump)
    }

    fn pattern_literal_to(
        &mut self,
        p: &Pattern,
        dst: u16,
        span: &Range<usize>,
    ) -> Result<(), CompileError> {
        use saule_interpreter::Value;
        let v = match p {
            Pattern::Int(n) => Value::Int(*n),
            Pattern::Float(f) => Value::Float(*f),
            Pattern::Bool(b) => Value::Bool(*b),
            Pattern::Str(s) => Value::Str(std::rc::Rc::new(s.clone())),
            Pattern::Nil => Value::Nil,
            _ => unreachable!("only literal patterns reach here"),
        };
        let k = self.constant(v, span)?;
        let a = self.reg8(dst, span)?;
        self.emit(Instruction::abx(Op::LOADK, a, k), span);
        Ok(())
    }

    /// Bind a variant's payload positionally into the arm's scope.
    fn bind_payload(
        &mut self,
        sc: u16,
        fields: &[Spanned<Pattern>],
        span: &Range<usize>,
    ) -> Result<(), CompileError> {
        if fields.is_empty() {
            return Ok(());
        }
        let payload = self.alloc(span)?;
        let (pa, sa) = (self.reg8(payload, span)?, self.reg8(sc, span)?);
        self.emit(Instruction::abc(Op::UNWRAP, pa, sa, 0), span);

        for (i, f) in fields.iter().enumerate() {
            let Pattern::Bind(name) = &f.value else {
                return Err(CompileError::unsupported(
                    "a nested pattern in a variant payload",
                    span.clone(),
                ));
            };
            let idx = self.alloc(span)?;
            let ia = self.reg8(idx, span)?;
            self.emit(Instruction::asbx(Op::LOADI, ia, i as i32 + 1), span);
            let slot = self.alloc(span)?;
            let sa2 = self.reg8(slot, span)?;
            self.emit(Instruction::abc(Op::GETARR, sa2, pa, ia), span);
            self.f.declare(name, slot);
        }
        Ok(())
    }

    fn arm_body(&mut self, body: &MatchBody, dst: u16) -> Result<(), CompileError> {
        match body {
            MatchBody::Expr(e) => self.expr_to(e, dst),
            MatchBody::Block(stmts) => {
                // A block-bodied arm's value is its last expression
                // statement, matching the module body's rule.
                let mut last = None;
                for s in stmts {
                    last = self.stmt(s)?.or(last);
                }
                match last {
                    Some(r) => {
                        let span = stmts.last().map(|s| s.span.clone()).unwrap_or(0..0);
                        self.move_result(r, dst, &span)
                    }
                    None => {
                        let a = self.reg8(dst, &(0..0))?;
                        self.emit(Instruction::abc(Op::LOADNIL, a, 0, 0), &(0..0));
                        Ok(())
                    }
                }
            }
        }
    }
}
