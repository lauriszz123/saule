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

/// What a `match`'s scrutinee produced, from the pattern matcher's side.
///
/// The oracle matches against a `Vec<Value>`; this is the compile-time
/// shadow of that list. Only a **top-level** tuple pattern can see more than
/// the first value, which is why every other `match` still evaluates its
/// scrutinee into a single register.
enum ValueList {
    /// Exactly one value, in this register. Any tuple pattern of arity other
    /// than 1 is then decidable at compile time.
    One(u16),
    /// A window of values whose length is only known at run time, with the
    /// length already materialized into `count` by `NVALS`.
    Counted { base: u16, count: u16 },
}
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

        // A **top-level** tuple pattern destructures the scrutinee's whole
        // value list, so the scrutinee has to be evaluated as one — which is
        // what `eval_values` does in the oracle. Every other shape wants a
        // single value, and takes the cheaper path unchanged: no existing
        // program's code moves because of this.
        //
        // A tuple *nested* inside a payload does not need this. The oracle
        // recurses with `from_ref(val)`, a one-element list, so a nested
        // tuple is matched against a single value rather than against the
        // scrutinee's list.
        let tuple_arity = arms
            .iter()
            .filter_map(|a| match &a.pattern.value {
                Pattern::Tuple(elems) => Some(elems.len()),
                _ => None,
            })
            .max();

        let (sc, values) = match tuple_arity {
            None => {
                let sc = self.expr_tmp(scrutinee)?;
                (sc, ValueList::One(sc))
            }
            Some(arity) => {
                // **Every register this needs is reserved before the
                // scrutinee is evaluated, and that ordering is the whole
                // correctness argument.**
                //
                // A `Want::All` call writes as many results as the callee
                // returned, which can be *more* than the window the register
                // allocator sized for its arguments — `store_results` grows
                // the stack and the allocator never hears about it. So a
                // register allocated after the call can alias `values[1]`,
                // and the first cut of this did exactly that: `NVALS` wrote
                // the count over the second result, and `case (q, r)` on
                // `return 4, 0` bound `r` to 2.
                //
                // Reserving below the window makes the two ranges disjoint
                // by construction: these registers all sit under the mark
                // the call's window is allocated from.
                let n = u16::try_from(arity.max(1)).map_err(|_| CompileError::Unsupported {
                    thing: "a tuple pattern with more than 65536 elements",
                    span: span.clone(),
                })?;
                let cnt = self.alloc(span)?;
                let zero = self.alloc(span)?;
                let vals = self.alloc_n(n, span)?;

                let m2 = self.mark();
                let dst0 = self.alloc(span)?;
                let r = self.expr_results(scrutinee, dst0, super::expr::Want::All)?;

                let list = match r.count {
                    // Not a call: exactly one value, and the length test is
                    // decidable at compile time, so no `NVALS` is emitted.
                    Some(_) => {
                        self.move_result(r.base, vals, span)?;
                        ValueList::One(vals)
                    }
                    // A call: the count is whatever the callee returned.
                    None => {
                        // The copies below read `r.base .. r.base + n`, and a
                        // pattern may ask for more elements than the callee
                        // returns — `case (a, b, c)` on a two-value call is
                        // a legal program that simply does not match. Those
                        // registers are past the window the allocator sized,
                        // so the frame has to be grown to cover them or the
                        // read runs off the end of the stack. Allocating
                        // above the window raises the high-water mark that
                        // becomes `max_regs`; the registers themselves are
                        // never used and go back with the window.
                        let _pad = self.alloc_n(n, span)?;
                        let (ca, ba) = (self.reg8(cnt, span)?, self.reg8(r.base, span)?);
                        self.emit(Instruction::abc(Op::NVALS, ca, ba, 0), span);
                        // Copy down the elements any arm could read. A copy
                        // past what the callee returned takes a stale
                        // register, which is harmless: every arm that reads
                        // element `i` is guarded by `count >= i + 1`.
                        for i in 0..n {
                            self.move_result(r.base + i, vals + i, span)?;
                        }
                        // The oracle reads `values[0]` as nil when the list
                        // is empty, and the copy above took a stale register
                        // in that case.
                        let (za, va) = (self.reg8(zero, span)?, self.reg8(vals, span)?);
                        self.emit(Instruction::asbx(Op::LOADI, za, 0), span);
                        self.emit(Instruction::abc(Op::JGTI, ca, za, 0), span);
                        self.emit(Instruction::abc(Op::LOADNIL, va, 0, 0), span);
                        ValueList::Counted {
                            base: vals,
                            count: cnt,
                        }
                    }
                };
                // The window has been read out of, so it can go back now.
                self.free_to(m2);
                (vals, list)
            }
        };

        let result = match self.switchable(arms) {
            Some(e_idx) => self.match_switch(e_idx, sc, arms, dst, span),
            None => self.match_chain(sc, &values, arms, dst, span),
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
                Pattern::Variant {
                    enum_name, fields, ..
                } => {
                    // Every payload sub-pattern must be **irrefutable**. The
                    // jump table dispatches straight into an arm's body, so
                    // there is no "next arm" for a sub-pattern to fail to —
                    // `case Event.Click(0, y)` needs the chain, where a
                    // failure has somewhere to jump.
                    //
                    // This guard used to be unnecessary and is now load
                    // bearing: `bind_payload` refused every non-`Bind` field
                    // outright, so a refutable payload could not reach here.
                    // Widening it to accept nested patterns is exactly the
                    // shape of trap 2 — an inert gap that a later widening
                    // turns into a live divergence — so the condition the
                    // refusal used to enforce is now enforced here.
                    if !fields.iter().all(|f| irrefutable(&f.value)) {
                        return None;
                    }
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
                    // `switchable` admitted this arm only because every
                    // payload sub-pattern is irrefutable, so nothing here can
                    // produce a failure label. Asserted rather than assumed:
                    // if that guard is ever loosened, this fires instead of
                    // silently dropping a jump that had nowhere to go.
                    let mut none = Vec::new();
                    self.bind_payload(sc, fields, &mut none, &arm.span)?;
                    debug_assert!(
                        none.is_empty(),
                        "a refutable payload pattern reached the jump-table path"
                    );
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
        values: &ValueList,
        arms: &[MatchArm],
        dst: u16,
        span: &Range<usize>,
    ) -> Result<(), CompileError> {
        let mut to_end = Vec::new();
        for arm in arms {
            // Three things happen in this order, and the order is the whole
            // of the correctness argument:
            //
            //   1. test the pattern — on failure jump to the next arm;
            //   2. bind what the pattern introduces, so the guard can see it;
            //   3. test the guard — on failure jump to the next arm too.
            //
            // Doing 3 before 2 is what made `case x when x < 0` refuse: the
            // binding was not in a register yet, so the guard's `x` looked
            // like a local the compiler had never seen. And folding 1 and 3
            // into one step is what made a *failed* pattern land inside the
            // arm's body, because the pattern's jump was patched to just
            // past the guard's — which is where the body starts.
            //
            // The scope is entered *before* the test now, because testing
            // and binding are interleaved: a tuple's first element binds
            // before its second is tested, exactly as the oracle collects
            // bindings as it walks. Step 1 and step 2 are therefore one
            // recursive walk — but every binding still lands before the
            // guard is emitted, which is the property the ordering rule
            // above is actually about.
            self.f.enter_scope();
            let mut pattern_failed = Vec::new();
            self.test_and_bind(sc, values, &arm.pattern, &mut pattern_failed, span)?;
            let guard_failed = self.arm_guard(arm, span)?;
            self.arm_body(&arm.body, dst)?;
            self.f.leave_scope();
            to_end.push(self.emit_jump(Op::JMP, 0, span));
            // Every failure path lands here: the next arm.
            for l in pattern_failed.into_iter().chain(guard_failed) {
                self.patch_here(l)?;
            }
        }
        for l in to_end {
            self.patch_here(l)?;
        }
        Ok(())
    }

    /// The dense tag of `Enum.Variant`, from whichever table knows it.
    ///
    /// Two sources. A Saule enum is in this module's layout table. A
    /// **prelude** enum — `FsKind`, `OsPlatform` — is defined in Rust and is
    /// therefore in no module's table at all, which is why matching on one
    /// used to refuse as `a variant of an unknown enum` and sent
    /// `examples/fs-info-example` to the tree-walker.
    ///
    /// Reading the prelude's is sound for the reason the stdlib constant fold
    /// is sound: the prelude is fixed before a program runs, and
    /// `install_*_enum` numbers variants by declaration order, so their tags
    /// are dense and stable exactly like a compiled enum's.
    ///
    /// `not_shadowed` is the guard that makes it safe — a module-level
    /// `local FsKind = {…}` must not resolve to the stdlib's. That is trap 1,
    /// and this compiler has shipped it once already.
    fn variant_tag(
        &self,
        enum_name: &str,
        variant: &str,
        span: &Range<usize>,
    ) -> Result<u32, CompileError> {
        if let Some(e) = self.layouts.enum_of(enum_name) {
            return self.chunk.enums[e as usize]
                .by_name
                .get(variant)
                .copied()
                .ok_or_else(|| CompileError::Unsupported {
                    thing: "a variant the enum does not declare",
                    span: span.clone(),
                });
        }
        if self.not_shadowed(enum_name)
            && let Some(saule_interpreter::Value::Enum(e)) = self.prelude_value(enum_name)
        {
            // The enum *is* known; a name it does not declare is a different
            // complaint, and reporting it as an unknown enum sent me looking
            // in the wrong place once already.
            return e
                .tags
                .get(variant)
                .copied()
                .ok_or_else(|| CompileError::Unsupported {
                    thing: "a variant the enum does not declare",
                    span: span.clone(),
                });
        }
        Err(CompileError::unsupported(
            "a variant of an unknown enum",
            span.clone(),
        ))
    }

    /// Test one pattern against a value, binding what it introduces.
    ///
    /// One recursive walk, mirroring the oracle's `match_pattern`: every
    /// failure pushes a label onto `fails`, and every label in `fails` is
    /// patched to the next arm by the caller. Bindings are declared as they
    /// are met, which is safe because a failed sub-pattern jumps out before
    /// anything can read them — the oracle's `out.clear()` expressed as
    /// control flow rather than as a list.
    ///
    /// `values` is the scrutinee's whole value list and is consulted **only**
    /// by a top-level tuple pattern; every recursive call passes
    /// `ValueList::One(r)`, which is the compile-time image of the oracle's
    /// `std::slice::from_ref(val)`.
    fn test_and_bind(
        &mut self,
        r: u16,
        values: &ValueList,
        pat: &Spanned<Pattern>,
        fails: &mut Vec<super::ctx::Label>,
        span: &Range<usize>,
    ) -> Result<(), CompileError> {
        let ra = self.reg8(r, span)?;
        match &pat.value {
            Pattern::Wildcard => {}
            Pattern::Bind(name) => {
                let d = self.alloc(span)?;
                let da = self.reg8(d, span)?;
                self.emit(Instruction::abc(Op::MOVE, da, ra, 0), span);
                self.f.declare(name, d);
            }
            Pattern::Int(_)
            | Pattern::Float(_)
            | Pattern::Bool(_)
            | Pattern::Str(_)
            | Pattern::Nil => {
                let m = self.mark();
                let lit = self.alloc(span)?;
                self.pattern_literal_to(&pat.value, lit, span)?;
                let la = self.reg8(lit, span)?;
                self.emit(Instruction::abc(Op::JEQ, ra, la, 0), span);
                fails.push(self.emit_jump(Op::JMP, 0, span));
                self.free_to(m);
            }
            Pattern::Variant {
                enum_name,
                variant,
                fields,
            } => {
                let tag = self.variant_tag(enum_name, variant, span)?;
                // `JIFTAG` skips the following jump when the tag matches, so
                // the jump is taken exactly when this arm does not apply.
                self.emit(Instruction::abc(Op::JIFTAG, ra, tag as u8, 0), span);
                fails.push(self.emit_jump(Op::JMP, 0, span));
                self.bind_payload(r, fields, fails, span)?;
            }
            Pattern::Tuple(elems) => {
                self.tuple_test_and_bind(values, elems, fails, span)?;
            }
        }
        Ok(())
    }

    /// A tuple pattern, against whatever list the scrutinee produced.
    ///
    /// The oracle's rule is `values.len() < elems.len()` fails, then each
    /// element is matched positionally. Both halves of that are reproduced
    /// here; which one costs an instruction depends on whether the length is
    /// known at compile time.
    fn tuple_test_and_bind(
        &mut self,
        values: &ValueList,
        elems: &[Spanned<Pattern>],
        fails: &mut Vec<super::ctx::Label>,
        span: &Range<usize>,
    ) -> Result<(), CompileError> {
        match *values {
            // One value: the length test is decidable here.
            ValueList::One(v) => {
                if elems.len() > 1 {
                    // `values.len() < elems.len()` is *always* true, so this
                    // arm can never match. An unconditional jump to the next
                    // arm is the whole of it — and it is reached, rather than
                    // being dead code, whenever a one-value scrutinee meets a
                    // multi-element tuple pattern.
                    fails.push(self.emit_jump(Op::JMP, 0, span));
                    return Ok(());
                }
                if let Some(only) = elems.first() {
                    self.test_and_bind(v, &ValueList::One(v), only, fails, span)?;
                }
            }
            // A window whose length `NVALS` put in `count`.
            ValueList::Counted { base, count } => {
                if !elems.is_empty() {
                    let m = self.mark();
                    let need = self.alloc(span)?;
                    let (na, ca) = (self.reg8(need, span)?, self.reg8(count, span)?);
                    self.emit(Instruction::asbx(Op::LOADI, na, elems.len() as i32), span);
                    // `JGEI` skips the jump when `count >= elems.len()`, so
                    // the jump is taken exactly on the oracle's `<`.
                    self.emit(Instruction::abc(Op::JGEI, ca, na, 0), span);
                    fails.push(self.emit_jump(Op::JMP, 0, span));
                    self.free_to(m);
                }
                for (i, sub) in elems.iter().enumerate() {
                    let v = base + i as u16;
                    self.test_and_bind(v, &ValueList::One(v), sub, fails, span)?;
                }
            }
        }
        Ok(())
    }

    /// Emit an arm's guard, if it has one.
    ///
    /// Called **after** the pattern's bindings are declared, because a guard
    /// is allowed to mention them — `case x when x < 0` reads `x`, and the
    /// resolver binds it exactly as the arm body's `x` is bound. The
    /// returned label jumps to the next arm when the guard is false, which
    /// is the same fate a failed pattern gets.
    fn arm_guard(
        &mut self,
        arm: &MatchArm,
        span: &Range<usize>,
    ) -> Result<Option<super::ctx::Label>, CompileError> {
        let Some(g) = &arm.guard else { return Ok(None) };
        let m = self.mark();
        let r = self.expr_tmp(g)?;
        let a = self.reg8(r, span)?;
        self.emit(Instruction::abc(Op::TEST, a, 0, 0), span);
        let l = self.emit_jump(Op::JMP, 0, span);
        self.free_to(m);
        Ok(Some(l))
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

    /// Match a variant's payload positionally.
    ///
    /// Each field is a full sub-pattern rather than only a name: a literal,
    /// a nested variant and a wildcard are all legal there, and each is
    /// handled by recursing into [`Self::test_and_bind`] with the payload
    /// element as a one-value list — the oracle's `from_ref(val)`.
    ///
    /// Previously this required every field to be a `Pattern::Bind` and
    /// refused anything else as `a nested pattern in a variant payload`.
    fn bind_payload(
        &mut self,
        sc: u16,
        fields: &[Spanned<Pattern>],
        fails: &mut Vec<super::ctx::Label>,
        span: &Range<usize>,
    ) -> Result<(), CompileError> {
        if fields.is_empty() {
            return Ok(());
        }
        let payload = self.alloc(span)?;
        let (pa, sa) = (self.reg8(payload, span)?, self.reg8(sc, span)?);
        self.emit(Instruction::abc(Op::UNWRAP, pa, sa, 0), span);

        for (i, f) in fields.iter().enumerate() {
            let idx = self.alloc(span)?;
            let ia = self.reg8(idx, span)?;
            self.emit(Instruction::asbx(Op::LOADI, ia, i as i32 + 1), span);
            let slot = self.alloc(span)?;
            let sa2 = self.reg8(slot, span)?;
            self.emit(Instruction::abc(Op::GETARR, sa2, pa, ia), span);
            self.test_and_bind(slot, &ValueList::One(slot), f, fails, span)?;
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

/// Whether a pattern always matches, so it needs no failure branch.
///
/// Only these two can appear in a payload on the jump-table path; see
/// `switchable`.
fn irrefutable(p: &Pattern) -> bool {
    matches!(p, Pattern::Wildcard | Pattern::Bind(_))
}
