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
                    Op::LOADK | Op::GETMAPK | Op::SETMAPK => chunk.constants.len(),
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
                if target < 0 || target as usize > n {
                    return Err(bad(
                        name,
                        pc,
                        format!("{op} jumps to {target}, outside 0..={n}"),
                    ));
                }
            }
            _ => {}
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
}
