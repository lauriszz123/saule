//! Pass 4: chunk verification (`VM_DESIGN.md` §17).
//!
//! Every register index below `max_regs`, every jump in range and landing on
//! an instruction boundary, every constant / proto / upvalue index valid.
//!
//! ## What this is for
//!
//! Two things, and the second is the one that matters long-term.
//!
//! **Now:** it turns a compiler bug into a diagnostic instead of a wrong
//! answer. A codegen mistake that emits a register the frame does not own
//! reads whatever the previous call left there — the VM cannot tell, and the
//! program quietly computes nonsense. Verification is the only place that
//! can catch it.
//!
//! **Later:** it is what would license `get_unchecked` in the dispatch loop
//! (§5.3). That optimisation is *unsound* without a verifier, which is why
//! the loop indexes safely today and says so.
//!
//! Run under `debug_assertions` and in the test suite; skipped in release
//! for chunks this compiler just produced. A chunk read back from a bytecode
//! cache (§14) would have to be verified always, since it is untrusted.

use crate::chunk::Chunk;
use crate::op::{Fmt, Op};

/// A malformed chunk. Never returned for a program the compiler accepted —
/// if one ever is, that is a compiler bug and this is the message describing
/// it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("internal: malformed chunk in `{proto}` at pc {pc}: {problem}")]
pub struct VerifyError {
    pub proto: String,
    pub pc: usize,
    pub problem: String,
}

fn bad(proto: &str, pc: usize, problem: impl Into<String>) -> VerifyError {
    VerifyError {
        proto: proto.to_string(),
        pc,
        problem: problem.into(),
    }
}

/// Check every proto in `chunk`.
pub fn verify(chunk: &Chunk) -> Result<(), VerifyError> {
    if chunk.main as usize >= chunk.protos.len() {
        return Err(bad("<chunk>", 0, "`main` does not name a proto"));
    }
    for proto in &chunk.protos {
        verify_proto(chunk, proto)?;
    }
    Ok(())
}

fn verify_proto(chunk: &Chunk, proto: &crate::chunk::Proto) -> Result<(), VerifyError> {
    let name = proto.label();
    let n = proto.code.len();
    let regs = proto.max_regs as u16;

    if n == 0 {
        return Err(bad(name, 0, "a proto with no instructions"));
    }
    if proto.n_params as u16 > regs {
        return Err(bad(
            name,
            0,
            format!(
                "{} parameters but a frame of only {} registers",
                proto.n_params, regs
            ),
        ));
    }

    // An `EXTRAARG` belongs to the instruction before it and is never
    // executed on its own, so it is tracked rather than decoded as one.
    let mut expect_extra = false;

    for (pc, ins) in proto.code.iter().enumerate() {
        let Some(op) = ins.op() else {
            return Err(bad(
                name,
                pc,
                format!("{:#04x} is not an opcode", ins.raw_op()),
            ));
        };

        if expect_extra {
            if op != Op::EXTRAARG {
                return Err(bad(name, pc, "an EXTRAARG was required here"));
            }
            expect_extra = false;
            continue;
        }
        if op == Op::EXTRAARG {
            return Err(bad(name, pc, "an EXTRAARG with no instruction before it"));
        }
        expect_extra = matches!(
            op,
            Op::CALLK | Op::CALLNAT | Op::CALLM_MR | Op::CALLMX | Op::CALLIF | Op::CALLSTAT | Op::NEWVAR | Op::ARITHX | Op::UNARYX
                | Op::TAILCALLK | Op::TAILCALLS
        );

        // Register operands. `A` is a register for every format that has
        // one except `Ax`; `B` and `C` are registers only in `ABC`, and even
        // then not always — `LOADBOOL`'s `B` is a flag, `GETF`'s `C` is a
        // field slot. Over-checking would reject valid chunks, so only the
        // unambiguous `A` is checked structurally, plus `B`/`C` for the
        // opcodes whose operands are known to be registers.
        if op.fmt() != Fmt::Ax && (ins.a() as u16) >= regs.max(1) && needs_register_a(op) {
            return Err(bad(
                name,
                pc,
                format!("register {} is outside a {}-register frame", ins.a(), regs),
            ));
        }

        match op.fmt() {
            Fmt::ABx => {
                let bx = ins.bx() as usize;
                let limit = match op {
                    Op::LOADK => chunk.constants.len(),
                    Op::CLOSURE => proto.protos.len(),
                    Op::GETMOD | Op::SETMOD => chunk.module_slot_base + chunk.module_slots,
                    Op::NEW => chunk.classes.len(),
                    Op::VARIANT => chunk.variant_refs.len(),
                    Op::SWITCH => chunk.jump_tables.len(),
                    _ => usize::MAX,
                };
                if bx >= limit {
                    return Err(bad(
                        name,
                        pc,
                        format!("{op} operand {bx} is out of range (limit {limit})"),
                    ));
                }
            }
            Fmt::AsBx if op.is_jump() => {
                // Displacements are relative to the *next* instruction,
                // because the dispatch loop has already advanced `pc`.
                let target = pc as i64 + 1 + ins.sbx() as i64;
                // **Strictly inside the code**, not `0..=n`. A jump landing
                // exactly one past the last instruction used to pass, and
                // the only thing standing between that and a read off the
                // end of the code array was the dispatch loop's own `pc >=
                // code.len()` test — which is now gone, because this is
                // what replaces it (§17). Every proto ends in a terminator,
                // so a jump to `n` could only ever have fallen off anyway.
                if target < 0 || target as usize >= n {
                    return Err(bad(
                        name,
                        pc,
                        format!("{op} jumps to {target}, outside 0..{n}"),
                    ));
                }
            }
            _ => {}
        }

        // An 8-bit table index carried in `B` or `C`.
        //
        // **`GETMAPK` and `SETMAPK` were listed in the `ABx` arm above and
        // are `Abc`**, so that arm never ran for them and their constant
        // index went unchecked for the life of the verifier — a listing that
        // reads as coverage and is not. Found by writing the test that was
        // supposed to confirm the existing behaviour.
        //
        // The operand position differs per opcode and is not guessable from
        // the format, so each is named with the table it indexes rather than
        // grouped by shape. **`CALLMX` is the proof that guessing does not
        // work**: it looks like `GETFX`'s sibling, its `C` is the *result
        // count*, and its member name rides in the `EXTRAARG`. Adding it
        // here rejected three perfectly good chunks before the test suite
        // caught it.
        //
        // **`EXTRAARG` payloads are deliberately not verified.** Each of the
        // eleven opcodes that take one packs something different — a module
        // and proto packed 8/16, a constant index, a `dynop` code — and a
        // wrong guess about any of them rejects valid chunks, which is a
        // worse failure than a gap. Verifying them wants a table on `Op`
        // saying what its `EXTRAARG` means, not a `match` written from the
        // doc comments.
        let table: Option<(usize, usize, &str)> = match op {
            // `R[A] := R[B].map[K[C]]`
            Op::GETMAPK => Some((ins.c() as usize, chunk.constants.len(), "constant")),
            // `R[A].map[K[B]] := R[C]`
            Op::SETMAPK => Some((ins.b() as usize, chunk.constants.len(), "constant")),
            // `R[A] == K[C]`
            Op::JEQK => Some((ins.c() as usize, chunk.constants.len(), "constant")),
            // `R[A] := R[B].<K[C]>`
            Op::GETFX => Some((ins.c() as usize, chunk.constants.len(), "constant")),
            // `R[A].<K[B]> := R[C]`
            Op::SETFX => Some((ins.b() as usize, chunk.constants.len(), "constant")),
            // `cast_types[C]`
            Op::CASTCHK | Op::CASTUNWRAP => {
                Some((ins.c() as usize, chunk.cast_types.len(), "cast type"))
            }
            // `type descriptor C`
            Op::CHKTY => Some((ins.c() as usize, chunk.type_descs.len(), "type descriptor")),
            _ => None,
        };
        if let Some((idx, limit, what)) = table
            && idx >= limit
        {
            return Err(bad(
                name,
                pc,
                format!("{op} {what} {idx} is out of range (limit {limit})"),
            ));
        }

        if op == Op::GETUPVAL || op == Op::SETUPVAL {
            let idx = ins.b() as usize;
            if idx >= proto.upvals.len() {
                return Err(bad(
                    name,
                    pc,
                    format!("upvalue {idx} but only {} declared", proto.upvals.len()),
                ));
            }
        }
    }

    if expect_extra {
        return Err(bad(name, n, "the proto ends before a required EXTRAARG"));
    }
    // A tail call never comes back, so it terminates a proto exactly as a
    // return does — but the statically-resolved forms carry an `EXTRAARG`,
    // which is then the physically last word. Step back over it, or a
    // perfectly good proto looks like one that runs off its end.
    let terminator = match proto.code.last().and_then(|i| i.op()) {
        Some(Op::EXTRAARG) => proto.code.get(n - 2).and_then(|i| i.op()),
        other => other,
    };
    match terminator {
        Some(
            Op::RET | Op::RET0 | Op::RET1 | Op::JMP
            | Op::TAILCALL | Op::TAILCALLK | Op::TAILCALLS,
        ) => Ok(()),
        _ => Err(bad(
            name,
            n - 1,
            "a proto whose last instruction is not a return or a jump — \
             execution would run off the end",
        )),
    }
}

/// Whether `A` names a register for this opcode.
///
/// `JMP`'s `A` is a close-upvalues threshold, not a register to read, and it
/// is legitimately 0 when nothing needs closing.
fn needs_register_a(op: Op) -> bool {
    !matches!(op, Op::JMP | Op::RET0 | Op::EXTRAARG)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::Proto;
    use crate::op::Instruction as I;

    fn chunk_with(code: Vec<I>, max_regs: u8) -> Chunk {
        let mut c = Chunk::empty("v.sau");
        c.main = c.add_proto(Proto::new(Some("main"), 0, max_regs, code));
        c
    }

    #[test]
    fn a_well_formed_chunk_verifies() {
        let c = chunk_with(
            vec![
                I::asbx(Op::LOADI, 0, 1),
                I::asbx(Op::LOADI, 1, 2),
                I::abc(Op::ADDI, 0, 0, 1),
                I::abc(Op::RET1, 0, 0, 0),
            ],
            2,
        );
        assert_eq!(verify(&c), Ok(()));
    }

    #[test]
    fn a_register_past_the_frame_is_caught() {
        let c = chunk_with(vec![I::abc(Op::RET1, 9, 0, 0)], 2);
        let e = verify(&c).unwrap_err();
        assert!(e.problem.contains("outside a 2-register frame"), "{e}");
    }

    #[test]
    fn a_jump_off_the_end_is_caught() {
        let c = chunk_with(vec![I::asbx(Op::JMP, 0, 99), I::abc(Op::RET0, 0, 0, 0)], 1);
        let e = verify(&c).unwrap_err();
        assert!(e.problem.contains("outside"), "{e}");
    }

    #[test]
    fn a_constant_index_past_the_pool_is_caught() {
        let c = chunk_with(vec![I::abx(Op::LOADK, 0, 7), I::abc(Op::RET1, 0, 0, 0)], 1);
        let e = verify(&c).unwrap_err();
        assert!(e.problem.contains("out of range"), "{e}");
    }

    #[test]
    fn a_missing_extraarg_is_caught() {
        // `CALLK` without its operand would make the VM read the next
        // instruction as a proto index.
        let c = chunk_with(vec![I::abc(Op::CALLK, 0, 1, 2), I::abc(Op::RET0, 0, 0, 0)], 1);
        let e = verify(&c).unwrap_err();
        assert!(e.problem.contains("EXTRAARG"), "{e}");
    }

    #[test]
    fn falling_off_the_end_is_caught() {
        let c = chunk_with(vec![I::asbx(Op::LOADI, 0, 1)], 1);
        let e = verify(&c).unwrap_err();
        assert!(e.problem.contains("run off the end"), "{e}");
    }

    #[test]
    fn an_undeclared_upvalue_is_caught() {
        let c = chunk_with(
            vec![I::abc(Op::GETUPVAL, 0, 3, 0), I::abc(Op::RET1, 0, 0, 0)],
            1,
        );
        let e = verify(&c).unwrap_err();
        assert!(e.problem.contains("upvalue"), "{e}");
    }

    // ---- structural checks that had no test ----------------------------

    #[test]
    fn an_unassigned_opcode_byte_is_caught() {
        // The first byte past the assigned range. Written through the
        // `Instruction` newtype rather than an `Op`, because the whole point
        // is a word no `Op` can produce — which is exactly what a chunk read
        // back from a cache or a corrupted file can contain, and the reason
        // §17 says a cached chunk must always be verified.
        let unassigned = Op::ALL.len() as u32;
        assert!(unassigned <= u8::MAX as u32, "the opcode space is full");
        let c = chunk_with(
            vec![I(unassigned << 24), I::abc(Op::RET0, 0, 0, 0)],
            1,
        );
        let e = verify(&c).unwrap_err();
        assert!(e.problem.contains("is not an opcode"), "{e}");
    }

    #[test]
    fn an_orphan_extraarg_is_caught() {
        // The mirror of `a_missing_extraarg_is_caught`: an `EXTRAARG` is
        // never executed on its own, so one that no instruction claims means
        // the stream is misaligned — and the dispatch loop would decode its
        // 24-bit payload as an opcode plus operands.
        let c = chunk_with(
            vec![I::ax_of(Op::EXTRAARG, 0), I::abc(Op::RET0, 0, 0, 0)],
            1,
        );
        let e = verify(&c).unwrap_err();
        assert!(e.problem.contains("no instruction before it"), "{e}");
    }

    #[test]
    fn a_proto_with_no_instructions_is_caught() {
        let c = chunk_with(vec![], 1);
        let e = verify(&c).unwrap_err();
        assert!(e.problem.contains("no instructions"), "{e}");
    }

    #[test]
    fn more_parameters_than_registers_is_caught() {
        let mut c = Chunk::empty("v.sau");
        c.main = c.add_proto(Proto::new(
            Some("main"),
            /* n_params */ 4,
            /* max_regs */ 2,
            vec![I::abc(Op::RET0, 0, 0, 0)],
        ));
        let e = verify(&c).unwrap_err();
        assert!(e.problem.contains("parameters but a frame"), "{e}");
    }

    // ---- one out-of-range `Bx` per table `verify_proto` limits ----------
    //
    // Each of these indexes a *different* chunk-level table, and each was
    // added to `verify_proto` separately. A test per table is what stops the
    // next table from being added to the `match` with no bound at all: the
    // `_ => usize::MAX` arm means an unlisted opcode is silently unchecked,
    // which reads as "verified" rather than as "not verified".

    /// Every table an `ABx` operand can index, and an opcode that indexes it.
    fn out_of_range_is_caught(ins: I, what: &str) {
        let c = chunk_with(vec![ins, I::abc(Op::RET0, 0, 0, 0)], 1);
        let Err(e) = verify(&c) else {
            panic!("an out-of-range {what} operand verified");
        };
        assert!(
            e.problem.contains("out of range"),
            "{what}: wrong diagnostic: {e}"
        );
    }

    #[test]
    fn a_proto_index_past_the_nested_list_is_caught() {
        out_of_range_is_caught(I::abx(Op::CLOSURE, 0, 3), "CLOSURE");
    }

    #[test]
    fn a_module_slot_past_the_programs_slots_is_caught() {
        // An empty chunk has `module_slot_base + module_slots == 0`, so slot
        // 0 is already past the end.
        out_of_range_is_caught(I::abx(Op::GETMOD, 0, 0), "GETMOD");
        out_of_range_is_caught(I::abx(Op::SETMOD, 0, 0), "SETMOD");
    }

    #[test]
    fn a_class_index_past_the_class_table_is_caught() {
        out_of_range_is_caught(I::abx(Op::NEW, 0, 1), "NEW");
    }

    #[test]
    fn a_variant_ref_past_the_table_is_caught() {
        out_of_range_is_caught(I::abx(Op::VARIANT, 0, 1), "VARIANT");
    }

    #[test]
    fn a_jump_table_index_past_the_table_is_caught() {
        out_of_range_is_caught(I::abx(Op::SWITCH, 0, 1), "SWITCH");
    }

    // ---- 8-bit table indices in `B` / `C` ------------------------------
    //
    // **These found a real hole.** `GETMAPK` and `SETMAPK` were listed in
    // the `ABx` limit match and are `Abc`, so that arm never ran for them
    // and their constant index went unchecked for the life of the verifier.
    // The listing read as coverage. Every opcode below carries a table index
    // in an 8-bit operand whose position is not guessable from the format,
    // which is why each gets its own case rather than a shape-based rule.

    #[test]
    fn a_map_key_constant_past_the_pool_is_caught() {
        out_of_range_is_caught(I::abc(Op::GETMAPK, 0, 0, 1), "GETMAPK");
        out_of_range_is_caught(I::abc(Op::SETMAPK, 0, 1, 0), "SETMAPK");
    }

    #[test]
    fn a_match_chain_constant_past_the_pool_is_caught() {
        out_of_range_is_caught(I::abc(Op::JEQK, 0, 0, 1), "JEQK");
    }

    #[test]
    fn a_dynamic_member_name_past_the_pool_is_caught() {
        out_of_range_is_caught(I::abc(Op::GETFX, 0, 0, 1), "GETFX");
        out_of_range_is_caught(I::abc(Op::SETFX, 0, 1, 0), "SETFX");
    }

    #[test]
    fn a_dynamic_member_call_is_not_mistaken_for_one() {
        // `CALLMX` reads like `GETFX`'s sibling and is not: its `C` is the
        // **result count**, and the member name rides in the `EXTRAARG`.
        // Bounding `C` against the constant pool rejected three valid
        // chunks. This pins the shape that broke, so the next person
        // extending the table above has a reason not to add it back.
        let mut c = Chunk::empty("v.sau");
        c.constants.push(saule_interpreter::Value::Int(0));
        c.main = c.add_proto(Proto::new(
            Some("main"),
            0,
            2,
            vec![
                // one argument, two results — `C` is 2, and there is no
                // second constant for it to be an index into.
                I::abc(Op::CALLMX, 0, 2, 2),
                I::ax_of(Op::EXTRAARG, 0),
                I::abc(Op::RET0, 0, 0, 0),
            ],
        ));
        assert_eq!(verify(&c), Ok(()));
    }

    #[test]
    fn a_cast_type_past_the_table_is_caught() {
        out_of_range_is_caught(I::abc(Op::CASTCHK, 0, 0, 1), "CASTCHK");
        out_of_range_is_caught(I::abc(Op::CASTUNWRAP, 0, 0, 1), "CASTUNWRAP");
    }

    #[test]
    fn a_type_descriptor_past_the_table_is_caught() {
        out_of_range_is_caught(I::abc(Op::CHKTY, 0, 0, 1), "CHKTY");
    }

    // ---- things that must *not* be rejected ----------------------------
    //
    // A verifier is only useful if it is quiet on valid chunks. Both of
    // these encode a rule that reads like a bug at a glance, so both are
    // worth pinning against a future "tightening" that breaks real code.

    #[test]
    fn a_tail_call_terminates_a_proto_even_though_extraarg_is_last() {
        // `TAILCALLK` carries an `EXTRAARG`, so the physically last word is
        // not the terminator. Without the step-back in `verify_proto` every
        // tail-recursive function looks like one that runs off its end.
        let mut c = Chunk::empty("v.sau");
        let target = c.add_proto(Proto::new(
            Some("callee"),
            0,
            1,
            vec![I::abc(Op::RET0, 0, 0, 0)],
        ));
        c.main = c.add_proto(Proto::new(
            Some("main"),
            0,
            1,
            vec![I::abc(Op::TAILCALLK, 0, 1, 0), I::ax_of(Op::EXTRAARG, target)],
        ));
        assert_eq!(verify(&c), Ok(()));
    }

    #[test]
    fn a_jump_with_a_zero_close_threshold_is_not_a_bad_register() {
        // `JMP`'s `A` is a close-upvalues threshold, not a register to read,
        // and 0 means "nothing to close" — the common case. Treating it as a
        // register would reject almost every real proto.
        let c = chunk_with(
            vec![I::asbx(Op::JMP, 0, 0), I::abc(Op::RET0, 0, 0, 0)],
            1,
        );
        assert_eq!(verify(&c), Ok(()));
    }
}
