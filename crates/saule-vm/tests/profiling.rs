//! `--profile-bytecode`'s collector, driven through the real dispatch loop
//! (`VM_DESIGN.md` §16).
//!
//! Everything here needs the `profile` feature, because without it the
//! counting copy of the loop is not compiled — see `Vm::execute`. Run them
//! with:
//!
//! ```text
//! cargo test -p saule-vm --features profile --test profiling
//! ```
//!
//! The unit tests in `profile.rs` cover the counters themselves and run in
//! every configuration; these cover the wiring, which is the part that can
//! silently stop counting.
#![cfg(feature = "profile")]

use std::rc::Rc;

use saule_lexer::Lexer;
use saule_parser::parse;
use saule_vm::op::Op;
use saule_vm::profile;

/// Compile and run `src` with the histogram on, and return it.
fn profile_of(src: &str) -> profile::Report {
    saule_interpreter::init();
    let toks = Lexer::new(src).tokenize().expect("lex");
    let module = parse(toks).expect("parse");
    let errs = saule_interpreter::analyze_and_prepare(&module, saule_semantic::ModuleSeed::default());
    assert!(errs.is_empty(), "semantic errors: {errs:?}");
    let chunk = saule_vm::compile(&module, "prof.sau", src).expect("compiles");
    profile::enable();
    saule_vm::run_chunk(Rc::new(chunk)).expect("runs");
    profile::take().expect("profiling was enabled")
}

fn count(r: &profile::Report, op: Op) -> u64 {
    r.ops.iter().find(|(o, _)| *o == op).map(|(_, n)| *n).unwrap_or(0)
}

fn pair(r: &profile::Report, first: Op, second: Op) -> u64 {
    r.pairs
        .iter()
        .find(|(a, b, _)| *a == first && *b == second)
        .map(|(_, _, n)| *n)
        .unwrap_or(0)
}

#[test]
fn a_counted_loop_counts_its_body_once_per_iteration() {
    // The arithmetic is the assertion: 10 iterations, one `ADDI` each, and
    // a histogram that says 9 or 11 is a histogram no optimisation decision
    // should be made from.
    let r = profile_of(
        "local acc: integer = 0\n\
         for i = 1, 10 do\n\
        \x20 acc = acc + i\n\
         end\n\
         acc",
    );
    assert_eq!(count(&r, Op::ADDI), 10);
    assert_eq!(count(&r, Op::FORLOOP_I), 10);
    assert_eq!(count(&r, Op::FORPREP_I), 1);
    assert_eq!(r.total, r.ops.iter().map(|(_, n)| n).sum::<u64>());
}

#[test]
fn pairs_are_only_counted_when_the_two_are_neighbours() {
    // `FORLOOP_I` jumps back to the top of the body, so the body's first
    // instruction follows it on every iteration — dynamically. It is not
    // the next *word*, and the emitter could not fuse the two, so it must
    // not appear as a pair. This is the distinction the whole histogram
    // rests on: a pair count is a fusion candidate, not a successor count.
    let r = profile_of(
        "local acc: integer = 0\n\
         for i = 1, 10 do\n\
        \x20 acc = acc + i\n\
         end\n\
         acc",
    );
    // The body compiles to `GETMOD, MOVE, ADDI, SETMOD, FORLOOP_I`, and the
    // back-edge lands on that `GETMOD` — 10 times. It contributes **no**
    // pairs. The one count here is the eleventh arrival, when the loop
    // finishes and `FORLOOP_I` falls through to the `GETMOD` that reads
    // `acc` back out — which is the next word, is adjacent, and is fusable.
    // 1 rather than 11 is the whole distinction, in one number.
    assert_eq!(
        pair(&r, Op::FORLOOP_I, Op::GETMOD),
        1,
        "a back-edge is not adjacency; only the fall-through exit is"
    );
    assert_eq!(
        pair(&r, Op::ADDI, Op::SETMOD),
        10,
        "consecutive words in the body are"
    );
    assert_eq!(pair(&r, Op::SETMOD, Op::FORLOOP_I), 10);
}

#[test]
fn a_call_breaks_the_pair_across_the_frame_boundary() {
    // The callee's first instruction is not adjacent to the call: they are
    // in different protos, where `pc` is not even the same counter.
    let r = profile_of(
        "fn twice(n: integer) -> integer\n\
        \x20 return n + n\n\
         end\n\
         twice(21)",
    );
    assert!(count(&r, Op::ADDI) >= 1, "the callee body ran");
    for (first, second, n) in &r.pairs {
        assert!(
            !matches!(first, Op::CALLK | Op::CALL | Op::CALLSTAT),
            "a call was paired with the callee's first instruction: \
             {} {} x{n}",
            first.name(),
            second.name()
        );
    }
}

#[test]
fn a_re_entrant_call_adds_to_the_same_histogram() {
    // `Table.sort`'s comparator runs on a second `Vm` built for the
    // callback (see `VmShared`'s `reentry_pool`). A profile that missed it
    // would understate exactly the comparator-heavy code `sort.sau` exists
    // to measure.
    let r = profile_of(
        "local t: table<integer, integer> = {5, 2, 9, 1, 7, 3}\n\
         Table.sort(t, (a: integer, b: integer) => a < b)\n\
         t[1]",
    );
    assert!(
        count(&r, Op::LTI) > 0,
        "the comparator's compare never reached the histogram: {:?}",
        r.ops
    );
}

#[test]
fn the_report_renders_what_it_counted() {
    let r = profile_of("local x: integer = 1\nx + 1");
    let out = r.render(20);
    assert!(out.contains("instructions executed"), "{out}");
    assert!(out.contains("ADDI"), "{out}");
    assert!(out.contains("adjacent pair"), "{out}");
}
