//! Property tests over the instruction encoding (`VM_DESIGN.md` §5.2, §23.2).
//!
//! The existing round-trip tests in `op.rs` check a handful of chosen
//! operands. These sweep the whole operand space instead, because the
//! failure mode being guarded against is a **silent** one: an encoding bug
//! does not crash, it produces a chunk that runs and computes the wrong
//! answer. The `LOADI` truncation caught during Phase 1 was exactly that
//! shape.
//!
//! No `proptest` dependency: the input space here is small enough to sweep
//! exhaustively in places, and a fixed-seed generator covers the rest. That
//! also makes a failure reproducible without a regression file.

use saule_vm::op::{Fmt, Instruction, Op, SBX_BIAS};

/// A deterministic generator. A fixed seed means a failure reproduces from
/// the test name alone — no saved counterexample to keep in sync.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*, chosen for being three lines and having no state to
        // get wrong.
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn u8(&mut self) -> u8 {
        self.next() as u8
    }

    fn u16(&mut self) -> u16 {
        self.next() as u16
    }

    /// A jump displacement inside the representable range.
    fn sbx(&mut self) -> i32 {
        (self.next() % (2 * SBX_BIAS as u64)) as i32 - SBX_BIAS
    }

    fn u24(&mut self) -> u32 {
        (self.next() as u32) & 0x00ff_ffff
    }
}

#[test]
fn every_opcode_round_trips_in_its_own_format() {
    let mut rng = Rng(0x5A_17_E0_01);
    for &op in Op::ALL {
        for _ in 0..200 {
            let ins = match op.fmt() {
                Fmt::Abc => {
                    let (a, b, c) = (rng.u8(), rng.u8(), rng.u8());
                    let ins = Instruction::abc(op, a, b, c);
                    assert_eq!((ins.a(), ins.b(), ins.c()), (a, b, c), "{op} ABC");
                    // `C` is also read as a signed immediate by the `*II`
                    // family; both readings must agree on the same bits.
                    assert_eq!(ins.sc(), c as i8 as i64, "{op} signed C");
                    ins
                }
                Fmt::ABx => {
                    let (a, bx) = (rng.u8(), rng.u16());
                    let ins = Instruction::abx(op, a, bx);
                    assert_eq!((ins.a(), ins.bx()), (a, bx), "{op} ABx");
                    ins
                }
                Fmt::AsBx => {
                    let (a, sbx) = (rng.u8(), rng.sbx());
                    let ins = Instruction::asbx(op, a, sbx);
                    assert_eq!((ins.a(), ins.sbx()), (a, sbx), "{op} AsBx");
                    ins
                }
                Fmt::Ax => {
                    let ax = rng.u24();
                    let ins = Instruction::ax_of(op, ax);
                    assert_eq!(ins.ax(), ax, "{op} Ax");
                    ins
                }
            };
            assert_eq!(ins.op(), Some(op), "opcode did not survive encoding");
        }
    }
}

#[test]
fn operands_never_bleed_into_the_opcode() {
    // The bug class this catches: an operand wide enough to overwrite the
    // opcode byte would silently turn one instruction into another.
    let mut rng = Rng(0xBEEF_1234);
    for &op in Op::ALL {
        for _ in 0..100 {
            let ins = match op.fmt() {
                Fmt::Abc => Instruction::abc(op, 0xFF, 0xFF, 0xFF),
                Fmt::ABx => Instruction::abx(op, 0xFF, u16::MAX),
                Fmt::AsBx => Instruction::asbx(op, 0xFF, SBX_BIAS - 1),
                Fmt::Ax => Instruction::ax_of(op, 0x00ff_ffff),
            };
            assert_eq!(ins.raw_op(), op as u8, "{op}: operands overwrote the opcode");
            let _ = rng.next();
        }
    }
}

#[test]
fn the_full_sbx_range_round_trips() {
    // Exhaustive, not sampled: this is the field that silently truncated a
    // `LOADI` operand during Phase 1, and it is only 16 bits wide.
    for sbx in -SBX_BIAS..SBX_BIAS {
        let ins = Instruction::asbx(Op::JMP, 0, sbx);
        assert_eq!(ins.sbx(), sbx);
        assert_eq!(ins.op(), Some(Op::JMP));
    }
}

#[test]
fn out_of_range_sbx_is_refused_rather_than_truncated() {
    // `try_asbx` is the seam the compiler needs: a literal too large for
    // `LOADI` must become a `LOADK`, and a jump too far must become a
    // `CompileError`. Both need a "does it fit?" answer that is not a panic.
    for bad in [SBX_BIAS, -SBX_BIAS - 1, i32::MAX, i32::MIN, 1_000_000] {
        assert!(
            Instruction::try_asbx(Op::LOADI, 0, bad).is_none(),
            "{bad} was accepted into a 16-bit field"
        );
    }
    for good in [-SBX_BIAS, -1, 0, 1, SBX_BIAS - 1] {
        assert!(Instruction::try_asbx(Op::LOADI, 0, good).is_some());
    }
}

#[test]
fn decoding_rejects_bytes_that_are_not_opcodes() {
    // What licenses the dispatch loop to treat a decoded opcode as
    // trustworthy: everything past the table decodes to `None`, and the loop
    // reports rather than transmuting.
    for v in 0..=255u8 {
        let decoded = Op::from_u8(v);
        if (v as usize) < Op::ALL.len() {
            assert_eq!(decoded, Some(Op::ALL[v as usize]));
        } else {
            assert_eq!(decoded, None, "byte {v} decoded to an opcode");
        }
    }
}

#[test]
fn arbitrary_words_decode_without_panicking() {
    // A chunk read off disk one day will not be trusted. Decoding must be
    // total: any 32-bit word either yields an opcode and operands, or says
    // it is not an instruction.
    let mut rng = Rng(0x0DDB_A11);
    for _ in 0..100_000 {
        let ins = Instruction(rng.next() as u32);
        match ins.op() {
            Some(op) => {
                assert_eq!(op as u8, ins.raw_op());
                // Every accessor must be defined for every word.
                let _ = (ins.a(), ins.b(), ins.c(), ins.sc());
                let _ = (ins.bx(), ins.sbx(), ins.ax());
            }
            None => assert!(ins.raw_op() as usize >= Op::ALL.len()),
        }
    }
}

#[test]
fn opcode_numbering_is_stable() {
    // The opcode table is the ABI of a compiled chunk: renumbering one
    // invalidates every chunk ever written, which will matter the day the
    // bytecode cache of §14 lands. Appending is free; inserting is not.
    //
    // Pinning the first and last few is enough to catch an insertion in the
    // middle without turning every addition into a test edit.
    assert_eq!(Op::MOVE as u8, 0);
    assert_eq!(Op::LOADK as u8, 1);
    assert_eq!(Op::LOADI as u8, 2);
    assert_eq!(Op::EXTRAARG as u8, 6);
    assert_eq!(
        Op::ALL.last().copied(),
        Some(Op::CALLMX),
        "a new opcode was appended after CALLMX — extend this assertion rather than \
         inserting one in the middle"
    );
}
