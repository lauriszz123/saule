//! `cargo run -p saule-vm --example sum`
//!
//! Hand-assembles Appendix B.1 of `VM_DESIGN.md` — the six-instruction
//! arithmetic loop the whole design is arguing for — prints its
//! disassembly, and runs it. Until the compiler exists this is the shortest
//! path to seeing the machine work end to end.

use std::rc::Rc;

use saule_interpreter::Value;
use saule_vm::chunk::{Chunk, Proto};
use saule_vm::op::{Instruction as I, Op};
use saule_vm::{disasm, run_chunk};

fn main() {
    // fn sum(n: integer) -> integer
    //   local total: integer = 0
    //   for i = 1 to n do total = total + i end
    //   return total
    // end
    //
    // r0 = n, r1 = total, r2..r4 = loop control, r5 = i
    let mut c = Chunk::empty("sum.sau");
    let sum = Proto::new(
        Some("sum"),
        1,
        6,
        vec![
            I::asbx(Op::LOADI, 1, 0),
            I::asbx(Op::LOADI, 2, 1),
            I::abc(Op::MOVE, 3, 0, 0),
            I::asbx(Op::LOADI, 4, 1),
            I::asbx(Op::FORPREP_I, 2, 2),
            I::abc(Op::ADDI, 1, 1, 5),
            I::asbx(Op::FORLOOP_I, 2, -2),
            I::abc(Op::RET1, 1, 0, 0),
        ],
    );
    let sum_idx = c.add_proto(sum);

    // 1_000_000 does not fit in `LOADI`'s 16-bit sBx, so it comes from the
    // constant pool — the fallback §15.3 describes for large literals.
    let n = c.add_constant(Value::Int(1_000_000)) as u16;
    let main = Proto::new(
        Some("main"),
        0,
        3,
        vec![
            I::abx(Op::LOADK, 1, n),
            I::abc(Op::CALLK, 1, 2, 2),
            I::ax_of(Op::EXTRAARG, sum_idx),
            I::abc(Op::RET1, 1, 0, 0),
        ],
    );
    c.main = c.add_proto(main);

    print!("{}", disasm::chunk(&c));

    let started = std::time::Instant::now();
    let out = run_chunk(Rc::new(c)).expect("chunk ran");
    let elapsed = started.elapsed();

    println!("\nsum(1_000_000) = {}", out[0].to_display_string());
    println!("ran in {elapsed:.2?}");
}
