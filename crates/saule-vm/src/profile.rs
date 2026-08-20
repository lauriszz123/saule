//! Bytecode profiling — the opcode and opcode-pair histogram `VM_DESIGN.md`
//! §16 asks for before any superinstruction is added.
//!
//! §16's rule is that "each must be justified by a profile before it is
//! added; every one of them is a maintenance cost", and it names the
//! collector: "a per-opcode-pair histogram collected under a
//! `--profile-bytecode` flag". This module is that collector.
//!
//! ## What a pair means here
//!
//! Only **statically adjacent** executions are counted as a pair: the two
//! instructions sat next to each other in the same proto and ran in that
//! order. That is deliberately narrower than "whatever ran next", because a
//! superinstruction is emitted by the *compiler*, which can only fuse two
//! words it is emitting one after the other. A `FORLOOP_I` that jumps back
//! to the top of a loop body is dynamically followed by that body's first
//! instruction on every iteration, and fusing them is not something the
//! emitter can do — counting it would make the histogram argue for work
//! that cannot be done.
//!
//! Two consequences worth knowing when reading a report:
//!
//! * A pair separated by an [`Op::EXTRAARG`] word is not counted, since the
//!   handler consumes that word and `pc` advances by two.
//! * Pairs are counted across a jump *into* the second instruction too —
//!   the count says the pair ran adjacently, not that the second is only
//!   ever reached from the first. A fusion candidate still has to be
//!   checked against the branch targets, which is the emitter's job.
//!
//! ## Cost when it is off
//!
//! None. The dispatch loop is generic over a `const PROFILE: bool` and
//! [`is_enabled`] is read once per frame activation, not per instruction,
//! so the non-profiling copy contains no counter, no branch and no
//! thread-local access at all.

use std::cell::RefCell;

use crate::op::Op;

/// Whether this build can actually collect a profile.
///
/// `false` unless the crate's `profile` feature is on, because the counting
/// copy of the dispatch loop is not compiled without it — see
/// `Vm::execute`, and the feature's comment in `Cargo.toml`, for the
/// measurement that put it behind a feature rather than a flag alone. A
/// caller must check this and say so: a silently empty report reads as
/// "your program executed no bytecode", which is a different and alarming
/// claim.
pub const SUPPORTED: bool = cfg!(feature = "profile");

/// Number of opcodes — the histogram's dimension.
const N: usize = Op::ALL.len();

// Per-thread counters. `None` until `enable` is called, which is what makes
// "was profiling ever on?" answerable at report time.
//
// Thread-local rather than global because a `Vm` is single-threaded by
// construction (`Rc` throughout), and because a re-entrant `Vm` — the one
// built for a `Table.sort` comparator — has to add to the same counters as
// the loop that called it without either of them holding a reference to the
// other.
thread_local! {
    static COUNTERS: RefCell<Option<Box<Counters>>> = const { RefCell::new(None) };
}

struct Counters {
    /// Executions per opcode, indexed by discriminant.
    ops: [u64; N],
    /// Executions per statically adjacent pair, indexed `prev * N + next`.
    /// Boxed: 122×122 `u64` is ~119 KiB, which does not belong on a stack.
    pairs: Box<[u64]>,
    total: u64,
}

impl Counters {
    fn new() -> Self {
        Self { ops: [0; N], pairs: vec![0; N * N].into_boxed_slice(), total: 0 }
    }
}

/// Start collecting on this thread. Idempotent; a second call keeps the
/// counts already gathered.
///
/// Enabling on a build where [`SUPPORTED`] is `false` is harmless and
/// useless: nothing calls [`record`], so the report comes back empty.
pub fn enable() {
    COUNTERS.with(|c| {
        let mut c = c.borrow_mut();
        if c.is_none() {
            *c = Some(Box::new(Counters::new()));
        }
    });
}

/// Whether collection is on for this thread.
///
/// Read once per frame activation by the dispatch loop, which is what picks
/// between its two monomorphised copies.
#[inline]
pub fn is_enabled() -> bool {
    COUNTERS.with(|c| c.borrow().is_some())
}

/// Record one executed instruction, and the pair it completes.
///
/// `prev` is `Some` only when the previous instruction was the immediately
/// preceding word of the same proto — see the module docs on what a pair
/// means. Called only from the profiling copy of the dispatch loop.
#[inline]
pub fn record(prev: Option<Op>, op: Op) {
    COUNTERS.with(|c| {
        if let Some(c) = c.borrow_mut().as_mut() {
            c.total += 1;
            c.ops[op as usize] += 1;
            if let Some(p) = prev {
                c.pairs[p as usize * N + op as usize] += 1;
            }
        }
    });
}

/// Stop collecting and take the counts, leaving the thread unprofiled.
///
/// `None` when [`enable`] was never called, which is how the CLI tells "the
/// program ran without profiling" from "the program ran no bytecode" — the
/// second happens whenever the compiler falls back to the tree-walker, and
/// an empty report is the honest answer there rather than a missing one.
pub fn take() -> Option<Report> {
    let counters = COUNTERS.with(|c| c.borrow_mut().take())?;
    let mut ops: Vec<(Op, u64)> = Op::ALL
        .iter()
        .enumerate()
        .filter(|&(i, _)| counters.ops[i] > 0)
        .map(|(i, &op)| (op, counters.ops[i]))
        .collect();
    ops.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.name().cmp(b.0.name())));

    let mut pairs: Vec<(Op, Op, u64)> = counters
        .pairs
        .iter()
        .enumerate()
        .filter(|&(_, &n)| n > 0)
        .map(|(i, &n)| (Op::ALL[i / N], Op::ALL[i % N], n))
        .collect();
    pairs.sort_by(|a, b| {
        b.2.cmp(&a.2)
            .then_with(|| a.0.name().cmp(b.0.name()))
            .then_with(|| a.1.name().cmp(b.1.name()))
    });

    Some(Report { total: counters.total, ops, pairs })
}

/// A finished histogram, sorted hottest first.
#[derive(Debug, Clone)]
pub struct Report {
    /// Instructions executed, across every frame and every re-entrant `Vm`.
    pub total: u64,
    /// `(opcode, executions)`, descending, zero counts dropped.
    pub ops: Vec<(Op, u64)>,
    /// `(first, second, executions)` for statically adjacent pairs,
    /// descending, zero counts dropped.
    pub pairs: Vec<(Op, Op, u64)>,
}

impl Report {
    /// Render the report the `--profile-bytecode` flag prints.
    ///
    /// `top` caps each table; the tail of a bytecode histogram is long and
    /// uninteresting, and the whole point is to pick the few candidates
    /// worth fusing. Both tables carry a **cumulative** share, because the
    /// question §16 asks is "how much of the run do the top N cover?".
    pub fn render(&self, top: usize) -> String {
        use std::fmt::Write as _;
        let mut s = String::new();

        if self.total == 0 {
            return "bytecode profile: no instructions executed — \
                    the program ran on the tree-walker\n"
                .into();
        }

        let _ = writeln!(
            s,
            "bytecode profile — {} instructions executed",
            group(self.total)
        );

        let _ = writeln!(s, "\n  opcode                    executions    share    cum");
        let mut cum = 0u64;
        for (op, n) in self.ops.iter().take(top) {
            cum += n;
            let _ = writeln!(
                s,
                "  {:<20} {:>14}  {:>6}  {:>5}",
                op.name(),
                group(*n),
                pct(*n, self.total),
                pct(cum, self.total)
            );
        }
        if self.ops.len() > top {
            let _ = writeln!(s, "  … {} more opcodes", self.ops.len() - top);
        }

        let _ = writeln!(
            s,
            "\n  adjacent pair                executions    share    cum"
        );
        // Against `total`, not against the number of pairs: a pair's share
        // is the fraction of the *run* a fused instruction would cover, and
        // that is the number a fusion decision turns on.
        let mut cum = 0u64;
        for (first, second, n) in self.pairs.iter().take(top) {
            cum += n;
            let _ = writeln!(
                s,
                "  {:<25} {:>12}  {:>6}  {:>5}",
                format!("{} {}", first.name(), second.name()),
                group(*n),
                pct(*n, self.total),
                pct(cum, self.total)
            );
        }
        if self.pairs.len() > top {
            let _ = writeln!(s, "  … {} more pairs", self.pairs.len() - top);
        }
        s
    }
}

/// `1234567` -> `1,234,567`. A bytecode count runs to nine digits and is
/// unreadable without separators.
fn group(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn pct(n: u64, total: u64) -> String {
    format!("{:.1}%", n as f64 * 100.0 / total as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Each `#[test]` gets its own thread and the counters are thread-local,
    // so these do not interfere with one another — the same property
    // `run.rs`'s engine-selection tests rely on.

    #[test]
    fn nothing_is_collected_until_enabled() {
        assert!(!is_enabled());
        record(None, Op::MOVE);
        assert!(take().is_none(), "recording without enabling must not start collection");
    }

    #[test]
    fn ops_and_pairs_are_counted_separately() {
        enable();
        record(None, Op::MOVE);
        record(Some(Op::MOVE), Op::ADDII);
        record(Some(Op::ADDII), Op::ADDII);
        let r = take().expect("enabled");
        assert_eq!(r.total, 3);
        assert_eq!(r.ops, vec![(Op::ADDII, 2), (Op::MOVE, 1)]);
        assert_eq!(r.pairs, vec![(Op::ADDII, Op::ADDII, 1), (Op::MOVE, Op::ADDII, 1)]);
    }

    #[test]
    fn taking_a_report_turns_collection_off() {
        enable();
        record(None, Op::MOVE);
        assert!(take().is_some());
        assert!(!is_enabled());
        assert!(take().is_none());
    }

    #[test]
    fn enabling_twice_keeps_what_was_counted() {
        enable();
        record(None, Op::MOVE);
        enable();
        record(None, Op::MOVE);
        assert_eq!(take().expect("enabled").total, 2);
    }

    #[test]
    fn a_report_with_no_instructions_says_the_tree_walker_ran_it() {
        enable();
        let r = take().expect("enabled");
        assert_eq!(r.total, 0);
        assert!(r.render(10).contains("tree-walker"));
    }

    #[test]
    fn support_tracks_the_feature() {
        // The one thing that must never drift: a build that says it can
        // profile has the counting loop compiled in, and one that says it
        // cannot has the flag refuse instead of reporting an empty run.
        assert_eq!(SUPPORTED, cfg!(feature = "profile"));
    }

    #[test]
    fn digits_are_grouped() {
        assert_eq!(group(0), "0");
        assert_eq!(group(999), "999");
        assert_eq!(group(1_000), "1,000");
        assert_eq!(group(1_234_567), "1,234,567");
    }
}
