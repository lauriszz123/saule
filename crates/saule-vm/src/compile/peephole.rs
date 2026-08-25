//! Pass 3.5: a peephole over the finished instruction stream (§17).
//!
//! Everything the task list called a "peephole" before this file existed was
//! a decision made *during* emission — `binary_to` reading an operand where
//! it already sits, `cast_to` folding a `!` into the cast. Those are worth
//! more than this pass and they are not going anywhere. What they cannot do
//! is see a pair whose two halves are emitted by **different** parts of the
//! compiler, because neither half knows the other is coming:
//!
//! ```text
//!   MOVE  1  0        ; `ret_stmt` moved the value into the return temp
//!   RET1  1           ; …and then returned it
//! ```
//!
//! `fib`, and every function in the language that ends `return x`.
//!
//! ## Deleting an instruction is a relocation
//!
//! Nothing here rewrites operands in place and stops. Removing a word
//! renumbers every instruction after it, and five separate tables name
//! instructions by index:
//!
//! * jump displacements, which are relative and so change when a deletion
//!   falls *between* a jump and its target,
//! * `try`/`catch` handler ranges and their catch entry,
//! * the per-arity entry points of a defaulted callee (§19),
//! * `SWITCH` jump tables, which live in the **chunk** and are reached
//!   through the indices the function recorded while compiling,
//! * the line table, which is what turns a fault into a source span.
//!
//! Miss one and the failure is a jump into the middle of a `match` arm, or
//! an error blaming the wrong line — both of which look like a codegen bug
//! anywhere but here. [`relocate`] does all five from one `old -> new` map,
//! and the debug-build verifier (Pass 4) runs afterwards over the result.
//!
//! ## What is not deleted
//!
//! Three things are off limits, and the rules below never have to think
//! about them because [`protected`] answers first:
//!
//! * **A jump target.** Remapping a target onto the next surviving
//!   instruction is usually right, but "usually" is not a standard this pass
//!   can be held to for a saving of one word. It declines instead.
//! * **The word after a conditional skip.** Every comparison opcode skips
//!   *the next instruction* (§15.7), so deleting that word silently
//!   re-points the skip at whatever followed it.
//! * **An `EXTRAARG`, or the instruction that owns one.** The payload is not
//!   an instruction and the pair is not separable.

use crate::chunk::{Handler, JumpTable, LineEntry};
use crate::op::{Instruction, Op};

/// Run every rule over one finished function body, deleting what they agree
/// is dead and relocating the tables that name instructions by index.
///
/// `tables` is the chunk's whole `jump_tables` vector and `owned` the
/// indices *this* function put in it — a `SWITCH` in a sibling function
/// names its own protos' instructions and must not be renumbered here.
///
/// Returns how many words were removed, which is what the tests assert on.
pub(crate) fn run(
    code: &mut Vec<Instruction>,
    lines: &mut Vec<LineEntry>,
    handlers: &mut [Handler],
    entries: &mut [u32],
    tables: &mut [JumpTable],
    owned: &[u16],
) -> usize {
    let mut removed = 0;
    // Rules feed each other: fusing a `MOVE` into a `RET1` can leave the
    // `JMP` that jumped over it with a displacement of zero. Iterating to a
    // fixed point costs a scan per round over a function-sized array, and
    // the bound is there because a rule that failed to shrink the code
    // would otherwise spin rather than produce a bad chunk.
    for _ in 0..8 {
        let n = pass(code, lines, handlers, entries, tables, owned);
        removed += n;
        if n == 0 {
            break;
        }
    }
    removed
}

/// One round: mark, then relocate. Returns how many words were removed.
fn pass(
    code: &mut Vec<Instruction>,
    lines: &mut Vec<LineEntry>,
    handlers: &mut [Handler],
    entries: &mut [u32],
    tables: &mut [JumpTable],
    owned: &[u16],
) -> usize {
    let n = code.len();
    let targets = jump_targets(code, handlers, entries, tables, owned);
    let mut dead = vec![false; n];
    let mut removed = 0;

    // Whether anything in this body can leave an **open upvalue** pointing
    // at one of its own registers, which is what makes returning that
    // register different from returning a copy of it — see the `MOVE`/`RET1`
    // rule. Only a `CLOSURE` binds a parent register into a cell, so its
    // absence is the whole test.
    let captures = code
        .iter()
        .any(|i| i.op() == Some(Op::CLOSURE));

    for pc in 0..n {
        if protected(code, &targets, pc) {
            continue;
        }
        let Some(op) = code[pc].op() else { continue };
        let ins = code[pc];

        match op {
            // `MOVE d, s` immediately before `RET1 d` — return the source
            // and drop the copy. The frame ends at the `RET1`, so `d`'s
            // value afterwards is not a question anyone can ask, which is
            // what makes this sound without liveness analysis.
            //
            // **Except when the frame captures.** `pop_frame` closes this
            // frame's upvalues *before* it moves the result out, and closing
            // one **moves** the value out of the register it was open on and
            // leaves `Nil` behind. So `RET1 s` on a captured `s` returns nil
            // where `MOVE d, s` + `RET1 d` returned the value: the copy the
            // fusion looks like waste is what carried it past the close.
            // Two differential tests caught this, which is the argument for
            // running them before believing a peephole.
            Op::MOVE => {
                if ins.a() == ins.b() {
                    // `MOVE d, d`: a register copied onto itself. The task
                    // list has wanted a debug assertion for this since
                    // Phase 5 on the grounds that it is never emitted —
                    // deleting it is the same statement, and holds even if
                    // some future rule starts producing one.
                    dead[pc] = true;
                    removed += 1;
                    continue;
                }
                if captures {
                    continue;
                }
                let Some(next) = code.get(pc + 1).copied() else { continue };
                // The `RET1` must not be reachable except by falling into
                // it. Jumping straight to it skips the `MOVE`, so it returns
                // whatever `d` held on that path — which is exactly what
                // rewriting it to read `s` would change.
                if targets[pc + 1] {
                    continue;
                }
                if next.op() == Some(Op::RET1) && next.a() == ins.a() {
                    code[pc + 1] = Instruction::abc(Op::RET1, ins.b(), 0, 0);
                    dead[pc] = true;
                    removed += 1;
                }
            }
            // A jump to the instruction that already follows it. `A > 0`
            // means it closes upvalues on the way, which is work rather than
            // control flow, so only `A == 0` is actually dead.
            Op::JMP if ins.sbx() == 0 && ins.a() == 0 => {
                dead[pc] = true;
                removed += 1;
            }
            _ => {}
        }
    }

    if removed > 0 {
        relocate(code, lines, handlers, entries, tables, owned, &dead);
    }
    removed
}

/// Whether `pc` is a word no rule may delete, whatever it holds.
fn protected(code: &[Instruction], targets: &[bool], pc: usize) -> bool {
    if targets[pc] {
        return true;
    }
    // An `EXTRAARG` payload, or the instruction that owns one. Both are
    // recognised from the payload itself: it carries the `EXTRAARG` opcode
    // in its own op byte, so the stream says which words are not
    // instructions without the reader having to know which opcodes take a
    // payload.
    if code[pc].op() == Some(Op::EXTRAARG) {
        return true;
    }
    if code.get(pc + 1).and_then(|i| i.op()) == Some(Op::EXTRAARG) {
        return true;
    }
    // The word a conditional skip skips (§15.7).
    pc > 0 && code[pc - 1].op().is_some_and(skips_next)
}

/// Whether this opcode conditionally skips the instruction after it.
///
/// Listed rather than derived: "is a comparison" is not the property that
/// matters — `LTI` compares and does not skip, `TESTSET` skips and also
/// assigns — and a `_ => false` over a `match` on `Op` is what makes adding
/// an opcode without thinking about this a compile error in the arm below
/// rather than a wrong answer here.
fn skips_next(op: Op) -> bool {
    matches!(
        op,
        Op::JLTI
            | Op::JLEI
            | Op::JGTI
            | Op::JGEI
            | Op::JLTF
            | Op::JLEF
            | Op::JGTF
            | Op::JGEF
            | Op::JEQI
            | Op::JNEI
            | Op::JLTII
            | Op::JLEII
            | Op::JGTII
            | Op::JGEII
            | Op::JEQII
            | Op::JNEII
            | Op::JEQ
            | Op::JNE
            | Op::JEQK
            | Op::TEST
            | Op::TESTSET
            | Op::JNIL
            | Op::JNOTNIL
            | Op::JIFTAG
    )
}

/// Every instruction index something jumps, unwinds or enters at.
///
/// One entry longer than the code: a forward jump may legitimately name
/// `code.len()`, and `pop_function` relies on that to decide whether the end
/// of the body is reachable.
fn jump_targets(
    code: &[Instruction],
    handlers: &[Handler],
    entries: &[u32],
    tables: &[JumpTable],
    owned: &[u16],
) -> Vec<bool> {
    let mut t = vec![false; code.len() + 1];
    let mut mark = |i: usize| {
        if i < t.len() {
            t[i] = true;
        }
    };
    for (pc, ins) in code.iter().enumerate() {
        let Some(op) = ins.op() else { continue };
        if op == Op::ITERPREP || op == Op::ITERPREPX {
            mark(pc + 1 + ins.bx() as usize);
        } else if op.is_jump() {
            let target = pc as i64 + 1 + ins.sbx() as i64;
            if target >= 0 {
                mark(target as usize);
            }
        }
    }
    for h in handlers {
        mark(h.pc_start as usize);
        mark(h.pc_end as usize);
        mark(h.target as usize);
    }
    for e in entries {
        mark(*e as usize);
    }
    for &i in owned {
        let table = &tables[i as usize];
        mark(table.default as usize);
        for &target in &table.targets {
            mark(target as usize);
        }
    }
    t
}

/// Compact `code`, then rewrite every index that named a deleted word.
///
/// The map is built first and everything is rewritten through it, rather
/// than each table being fixed up as the deletion happens — a deletion moves
/// instructions that other deletions have already been accounted for, and
/// doing it in one step is what keeps that from having to be reasoned about.
fn relocate(
    code: &mut Vec<Instruction>,
    lines: &mut Vec<LineEntry>,
    handlers: &mut [Handler],
    entries: &mut [u32],
    tables: &mut [JumpTable],
    owned: &[u16],
    dead: &[bool],
) {
    let n = code.len();
    // `new[i]` is where the word at `i` ends up — and for a deleted word,
    // where the next surviving one does. No rule deletes a jump target, so
    // that fallback is never the answer to a real jump; it is what makes
    // the line table land on the instruction that replaced the deleted one.
    let mut new = vec![0u32; n + 1];
    let mut k = 0u32;
    for i in 0..n {
        new[i] = k;
        if !dead[i] {
            k += 1;
        }
    }
    new[n] = k;

    // Jumps first, while the old indices still mean something.
    for pc in 0..n {
        if dead[pc] {
            continue;
        }
        let ins = code[pc];
        let Some(op) = ins.op() else { continue };
        if op == Op::ITERPREP || op == Op::ITERPREPX {
            let target = pc + 1 + ins.bx() as usize;
            let disp = new[target] - (new[pc] + 1);
            code[pc] = Instruction::abx(op, ins.a(), disp as u16);
        } else if op.is_jump() {
            let target = (pc as i64 + 1 + ins.sbx() as i64) as usize;
            let disp = new[target] as i64 - (new[pc] as i64 + 1);
            // The displacement only ever shrinks — a deletion between a jump
            // and its target moves them closer — so a value that fitted
            // before still fits, and `expect` is a statement about that
            // rather than a hope.
            code[pc] = Instruction::try_asbx(op, ins.a(), disp as i32)
                .expect("a peephole only shortens a displacement");
        }
    }

    for h in handlers.iter_mut() {
        h.pc_start = new[h.pc_start as usize];
        h.pc_end = new[h.pc_end as usize];
        h.target = new[h.target as usize];
    }
    for e in entries.iter_mut() {
        *e = new[*e as usize];
    }
    for &i in owned {
        let table = &mut tables[i as usize];
        table.default = new[table.default as usize];
        for target in table.targets.iter_mut() {
            *target = new[*target as usize];
        }
    }

    // The line table maps a pc to the span that emitted it, and two entries
    // can now land on one pc — the deleted word's and its successor's. The
    // **later** one wins: the instruction now living there is the successor,
    // so its span is the one a fault at that pc should blame.
    for l in lines.iter_mut() {
        l.pc = new[l.pc as usize];
    }
    lines.dedup_by(|later, earlier| {
        if later.pc == earlier.pc {
            *earlier = *later;
            true
        } else {
            false
        }
    });

    let mut i = 0;
    code.retain(|_| {
        let keep = !dead[i];
        i += 1;
        keep
    });
}
