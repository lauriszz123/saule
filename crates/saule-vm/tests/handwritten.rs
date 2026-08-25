//! Hand-assembled chunks, executed end to end.
//!
//! This is the Phase 1 exit criterion (`VM_DESIGN.md` §21.2) and then some:
//! until the compiler exists, hand-assembly is the only way to reach the
//! dispatch loop, so these double as the conformance tests for every opcode
//! that is implemented.
//!
//! `sum_loop` is Appendix B.1 verbatim — the six-instruction arithmetic loop
//! the whole design is arguing for.

use std::rc::Rc;

use saule_interpreter::Value;
use saule_vm::chunk::{Chunk, Proto, UpvalDesc};
use saule_vm::op::{Instruction as I, Op};
use saule_vm::{disasm, run_chunk};

fn run(chunk: Chunk) -> Vec<Value> {
    run_chunk(Rc::new(chunk)).expect("chunk ran")
}

fn int(v: &Value) -> i64 {
    match v {
        Value::Int(n) => *n,
        other => panic!("expected an integer, got {other:?}"),
    }
}

#[test]
fn adds_one_and_two() {
    let mut c = Chunk::empty("add.sau");
    let main = Proto::new(
        Some("main"),
        0,
        3,
        vec![
            I::asbx(Op::LOADI, 0, 1),
            I::asbx(Op::LOADI, 1, 2),
            I::abc(Op::ADDI, 2, 0, 1),
            I::abc(Op::RET1, 2, 0, 0),
        ],
    );
    c.main = c.add_proto(main);
    assert_eq!(int(&run(c)[0]), 3);
}

#[test]
fn appendix_b1_sum_loop() {
    // fn sum(n) local total = 0; for i = 1 to n do total = total + i end;
    // return total end   -- called with n = 100
    //
    // r0 = n, r1 = total, r2..r4 = loop control, r5 = i
    let mut c = Chunk::empty("sum.sau");
    let sum = Proto::new(
        Some("sum"),
        1,
        6,
        vec![
            I::asbx(Op::LOADI, 1, 0),      // total = 0
            I::asbx(Op::LOADI, 2, 1),      // counter = 1
            I::abc(Op::MOVE, 3, 0, 0),     // limit = n
            I::asbx(Op::LOADI, 4, 1),      // step = 1
            I::asbx(Op::FORPREP_I, 2, 2),  // -> exit
            I::abc(Op::ADDI, 1, 1, 5),     // body: total = total + i
            I::asbx(Op::FORLOOP_I, 2, -2), // -> body
            I::abc(Op::RET1, 1, 0, 0),     // exit
        ],
    );
    let sum_idx = c.add_proto(sum);

    let main = Proto::new(
        Some("main"),
        0,
        3,
        vec![
            I::asbx(Op::LOADI, 1, 100),
            I::abc(Op::CALLK, 1, 2, 2),
            I::ax_of(Op::EXTRAARG, sum_idx),
            I::abc(Op::RET1, 1, 0, 0),
        ],
    );
    c.main = c.add_proto(main);

    assert_eq!(int(&run(c)[0]), 5050);
}

#[test]
fn recursive_fib_through_callk() {
    // fn fib(n) if n < 2 then return n end return fib(n-1) + fib(n-2) end
    let mut c = Chunk::empty("fib.sau");
    let fib_idx = c.protos.len() as u32;
    let fib = Proto::new(
        Some("fib"),
        1,
        6,
        vec![
            I::asbx(Op::LOADI, 1, 2),
            I::abc(Op::JGEI, 0, 1, 0),     // n >= 2? skip the return
            I::abc(Op::RET1, 0, 0, 0),
            I::abc(Op::SUBII, 2, 0, 1),    // r2 = n - 1
            I::abc(Op::CALLK, 2, 2, 2),
            I::ax_of(Op::EXTRAARG, fib_idx),
            I::abc(Op::SUBII, 3, 0, 2),    // r3 = n - 2
            I::abc(Op::CALLK, 3, 2, 2),
            I::ax_of(Op::EXTRAARG, fib_idx),
            I::abc(Op::ADDI, 2, 2, 3),
            I::abc(Op::RET1, 2, 0, 0),
        ],
    );
    assert_eq!(c.add_proto(fib), fib_idx);

    let main = Proto::new(
        Some("main"),
        0,
        3,
        vec![
            I::asbx(Op::LOADI, 1, 20),
            I::abc(Op::CALLK, 1, 2, 2),
            I::ax_of(Op::EXTRAARG, fib_idx),
            I::abc(Op::RET1, 1, 0, 0),
        ],
    );
    c.main = c.add_proto(main);

    assert_eq!(int(&run(c)[0]), 6765);
}

#[test]
fn closure_captures_a_live_register_then_closes_over_it() {
    // local n = 41; local f = fn() return n + 1 end; n = 41; return f()
    //
    // The upvalue is open while `main` is live, so the closure reads the
    // register directly — the live-binding semantics of §7.1.
    let mut c = Chunk::empty("closure.sau");

    let mut inner = Proto::new(Some("bump"), 0, 2, vec![
        I::abc(Op::GETUPVAL, 0, 0, 0),
        I::abc(Op::ADDII, 0, 0, 1),
        I::abc(Op::RET1, 0, 0, 0),
    ]);
    inner.upvals.push(UpvalDesc {
        from_parent_stack: true,
        index: 0,
        name: Rc::from("n"),
    });
    let inner_idx = c.add_proto(inner);

    let mut main = Proto::new(
        Some("main"),
        0,
        4,
        vec![
            I::asbx(Op::LOADI, 0, 40),
            I::abx(Op::CLOSURE, 1, 0),
            I::asbx(Op::LOADI, 0, 41),  // written *after* capture
            I::abc(Op::CALL, 1, 1, 2),
            I::abc(Op::RET1, 1, 0, 0),
        ],
    );
    main.protos.push(inner_idx);
    c.main = c.add_proto(main);

    assert_eq!(int(&run(c)[0]), 42);
}

#[test]
fn tables_and_concat() {
    let mut c = Chunk::empty("table.sau");
    let k = c.add_constant(Value::Str(SauleStr::new("!".to_string())));
    let main = Proto::new(
        Some("main"),
        0,
        6,
        vec![
            I::abc(Op::NEWT, 0, 2, 0),
            I::asbx(Op::LOADI, 1, 10),
            I::asbx(Op::LOADI, 2, 20),
            I::abc(Op::SETLIST, 0, 2, 0),
            I::asbx(Op::LOADI, 3, 2),
            I::abc(Op::GETARR, 4, 0, 3),   // r4 = t[2] = 20
            I::abc(Op::TOSTR, 4, 4, 0),
            I::abx(Op::LOADK, 5, k as u16),
            I::abc(Op::CONCAT, 4, 4, 5),   // "20" .. "!"
            I::abc(Op::RET1, 4, 0, 0),
        ],
    );
    c.main = c.add_proto(main);

    match &run(c)[0] {
        Value::Str(s) => assert_eq!(s.as_str(), "20!"),
        other => panic!("expected a string, got {other:?}"),
    }
}

#[test]
fn integer_division_by_zero_is_a_runtime_error() {
    let mut c = Chunk::empty("div.sau");
    let main = Proto::new(
        Some("main"),
        0,
        3,
        vec![
            I::asbx(Op::LOADI, 0, 1),
            I::asbx(Op::LOADI, 1, 0),
            I::abc(Op::DIVI, 2, 0, 1),
            I::abc(Op::RET1, 2, 0, 0),
        ],
    );
    c.main = c.add_proto(main);

    let err = run_chunk(Rc::new(c)).expect_err("division by zero must fail");
    assert!(
        matches!(err, saule_interpreter::RuntimeError::DivisionByZero { .. }),
        "got {err:?}"
    );
}

#[test]
fn unimplemented_opcodes_report_rather_than_panic() {
    // Repointed each time the named opcode gains a body — the assertion is
    // about the *shape* of the answer, not about which opcode is missing:
    // an unimplemented instruction must report by name, never panic and
    // never silently do nothing.
    let mut c = Chunk::empty("todo.sau");
    let main = Proto::new(Some("main"), 0, 2, vec![
        I::abc(Op::SUPER, 0, 1, 0),
        I::abc(Op::RET1, 0, 0, 0),
    ]);
    c.main = c.add_proto(main);

    let err = run_chunk(Rc::new(c)).expect_err("SUPER has no body yet");
    assert!(
        matches!(err, saule_interpreter::RuntimeError::Unsupported { thing: "SUPER", .. }),
        "got {err:?}"
    );
}

#[test]
fn disassembly_is_readable() {
    let mut c = Chunk::empty("disasm.sau");
    let main = Proto::new(
        Some("main"),
        0,
        3,
        vec![
            I::asbx(Op::LOADI, 0, 1),
            I::asbx(Op::JMP, 0, 1),
            I::abc(Op::ADDI, 2, 0, 1),
            I::abc(Op::RET1, 0, 0, 0),
        ],
    );
    c.main = c.add_proto(main);

    let text = disasm::chunk(&c);
    assert!(text.contains("LOADI"), "{text}");
    assert!(text.contains("JMP"), "{text}");
    // The jump target must be resolved to an absolute pc, not left as a
    // displacement — that is the whole point of the annotation column.
    assert!(text.contains("-> 0003"), "{text}");
}
