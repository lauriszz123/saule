//! A small method compiled where it is called, behind a test that it is the
//! method the call would have found.
//!
//! A call is a frame pushed and popped, and for a method of three or four
//! instructions that is most of what it costs: `stack.push(x)` in a loop
//! spends more on arriving in `push` and leaving it than on what `push`
//! does. The compiler knows the receiver's class, as the type checker
//! proved it, and has the method's body in hand when the method was
//! compiled first, so it can lay the body down in the caller's own
//! registers.
//!
//! What it cannot know is that the object will be of exactly that class
//! when the program runs: a subclass may override the method. That one
//! thing is asked as the program runs, by [`Op::ISREADY`], and a receiver
//! that is anything else takes the ordinary `CALLM` kept beside the body:
//!
//! ```text
//!   ISREADY  g  recv  slot     ; exactly this class, its table field a table?
//!   TEST     g  0  1           ; no: on to the call
//!   JMP      body
//!   CALLM    recv  n  slot
//!   JMP      end
//! body:
//!   …the method's instructions, its registers counted from `recv`…
//! end:
//! ```
//!
//! ## Only a body that cannot fail
//!
//! A call that never happened must not be told from one that did. A
//! failure inside the body would be reported with one frame missing from
//! its stack trace, so only instructions that cannot fail once the test
//! has passed are taken ([`Compiler::inline_plan`]): moves and constants,
//! typed arithmetic that cannot divide, comparisons and jumps, fields of
//! `self` read and written (the class is exact, so the slot is there), and
//! an element of a table that `self` holds in a field, which is why the
//! test also asks that the field holds a table. Anything else, a call
//! above all, keeps the method a call.
//!
//! ## Registers
//!
//! A method's frame starts at its receiver (§6.2), so its register `r` is
//! the caller's `recv + r` and the body is copied with that added: no
//! shuffling, and the result is where a call would have left it. The two
//! instructions that name register 0 without saying so, `GETFIDX` and
//! `SETFIDX`, are written out as the `GETF` and `GETIDX` or `SETIDX` they
//! stand for, through a register past the method's own.

use std::ops::Range;

use super::CompileError;
use super::ctx::{Compiler, Label};
use crate::chunk::{ClassIdx, Proto};
use crate::op::{Instruction, Op};

/// The longest body copied. Past a few instructions the frame is no longer
/// most of the call, and every copy is that much more code.
const LONGEST: usize = 12;

/// `ISREADY`'s `C` when the body indexes no table field.
pub(crate) const NO_SLOT: u8 = 0xff;

/// What [`Compiler::inline_plan`] found: the body can be copied, and the
/// field of `self` it indexes as a table, if any.
struct Plan {
    table_slot: u8,
}

/// How many words `ins` of a body becomes where it is copied to. `wanted`
/// is whether the call's result is used.
fn width(ins: Instruction, last: bool, wanted: bool) -> usize {
    match ins.op() {
        // The answer moved to where a call leaves it, and a jump to the
        // end, which the last instruction is already at.
        Some(Op::RET1) => usize::from(ins.a() != 0) + usize::from(!last),
        Some(Op::RET0) => usize::from(wanted) + usize::from(!last),
        Some(Op::GETFIDX) | Some(Op::SETFIDX) => 2,
        _ => 1,
    }
}

/// Whether `op` skips the instruction after it when its test holds.
fn skips(op: Op) -> bool {
    matches!(
        op,
        Op::JLTI
            | Op::JLEI
            | Op::JGTI
            | Op::JGEI
            | Op::JEQI
            | Op::JNEI
            | Op::JLTF
            | Op::JLEF
            | Op::JGTF
            | Op::JGEF
            | Op::JLTII
            | Op::JLEII
            | Op::JGTII
            | Op::JGEII
            | Op::JEQII
            | Op::JNEII
            | Op::JNIL
            | Op::JNOTNIL
            | Op::TEST
    )
}

impl Compiler<'_> {
    /// Whether `proto`, called with `n_args` arguments (the receiver one of
    /// them), can be copied into its caller: see the module's notes.
    fn inline_plan(proto: &Proto, n_args: usize, wanted: bool) -> Option<Plan> {
        let code = &proto.code;
        let plain = proto.n_params as usize == n_args
            && !proto.is_variadic
            && proto.upvals.is_empty()
            && proto.handlers.is_empty()
            && proto.entries.is_empty()
            && proto.protos.is_empty();
        if !plain || code.is_empty() || code.len() > LONGEST {
            return None;
        }
        if !matches!(code.last()?.op()?, Op::RET0 | Op::RET1) {
            return None;
        }
        let mut table_slot = NO_SLOT;
        let mut index = |slot: u8| {
            let same = table_slot == NO_SLOT || table_slot == slot;
            table_slot = slot;
            same && slot != NO_SLOT
        };
        for (pc, ins) in code.iter().enumerate() {
            let op = ins.op()?;
            // Register 0 is the receiver throughout: nothing writes it.
            let writes_self = ins.a() == 0;
            let ok = match op {
                Op::MOVE | Op::LOADK | Op::LOADI | Op::LOADF | Op::LOADBOOL | Op::LOADNIL => {
                    !writes_self
                }
                Op::ADDI | Op::SUBI | Op::MULI | Op::NEGI | Op::ADDII | Op::SUBII | Op::MULII => {
                    !writes_self
                }
                Op::LTI | Op::LEI | Op::EQI => !writes_self,
                // Floats: nothing of theirs fails, division included.
                Op::ADDF | Op::SUBF | Op::MULF | Op::DIVF | Op::NEGF => !writes_self,
                Op::LTF | Op::LEF | Op::EQF => !writes_self,
                // A field of the receiver, whose class the test has settled.
                Op::GETF => !writes_self && ins.b() == 0,
                Op::SETF => ins.a() == 0,
                Op::GETFIDX => !writes_self && index(ins.b()),
                Op::SETFIDX => index(ins.a()),
                Op::JMP => {
                    let target = pc as i64 + 1 + ins.sbx() as i64;
                    ins.a() == 0 && target >= 0 && (target as usize) < code.len()
                }
                Op::RET0 | Op::RET1 => true,
                _ => skips(op),
            };
            if !ok {
                return None;
            }
            // What a test skips has to stay one word.
            if pc > 0 && skips(code[pc - 1].op()?) && width(*ins, pc + 1 == code.len(), wanted) != 1
            {
                return None;
            }
        }
        Some(Plan { table_slot })
    }

    /// Compile the method in `slot` of `class` in place of a call of it,
    /// when that can be done: the receiver is in `base`, the arguments after
    /// it, `n_args` of them counting the receiver, as `CALLM` wants them.
    /// `wanted` is whether the call's result is used. `false` when nothing
    /// was emitted and the call is still to be made.
    ///
    /// `exact` says the receiver is known to be of `class` and no other,
    /// as the object a `NEW` has just made is: no test is emitted then, and
    /// so no body that needs the test's second half, a table in a field.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn inline_method(
        &mut self,
        class: ClassIdx,
        slot: u16,
        base: u16,
        n_args: usize,
        wanted: bool,
        exact: bool,
        span: &Range<usize>,
    ) -> Result<bool, CompileError> {
        let Some(row) = self.chunk.classes.get(class as usize) else {
            return Ok(false);
        };
        // The body has to be this chunk's, and compiled already: a slot
        // inherited from another module names a proto of that module's.
        let declared_here = row.vowner.get(slot as usize).is_some_and(|owner| {
            self.chunk
                .classes
                .get(*owner as usize)
                .is_some_and(|c| c.module == self.chunk.module_index)
        });
        let Some(&proto_idx) = row.vtable.get(slot as usize) else {
            return Ok(false);
        };
        if !declared_here || proto_idx as usize >= self.chunk.protos.len() || class > 0xff_ffff {
            return Ok(false);
        }
        let callee = std::rc::Rc::clone(self.chunk.proto(proto_idx));
        let Some(plan) = Self::inline_plan(&callee, n_args, wanted) else {
            return Ok(false);
        };
        if exact && plan.table_slot != NO_SLOT {
            return Ok(false);
        }

        // The method's registers are the caller's from `base`; past them,
        // one for a table read out of a field and one for the test.
        let frame = (base + callee.max_regs as u16).max(self.f.regs.top());
        if frame + 2 > 255 || slot > 0xff || n_args > 0xff {
            return Ok(false);
        }
        let m = self.mark();
        let top = self.f.regs.top();
        if frame + 2 > top {
            self.alloc_n(frame + 2 - top, span)?;
        }
        let (table, ready) = (frame as u8, (frame + 1) as u8);
        let recv = base as u8;
        let at = |r: u8| recv + r;

        let mut to_end = Vec::new();
        if !exact {
            self.emit(
                Instruction::abc(Op::ISREADY, ready, recv, plan.table_slot),
                span,
            );
            self.emit(Instruction::ax_of(Op::EXTRAARG, class), span);
            self.emit(Instruction::abc(Op::TEST, ready, 0, 1), span);
            let to_body = self.emit_jump(Op::JMP, 0, span);
            self.emit(
                Instruction::abc(Op::CALLM, recv, n_args as u8, slot as u8),
                span,
            );
            to_end.push(self.emit_jump(Op::JMP, 0, span));
            self.patch_here(to_body)?;
        }

        let n = callee.code.len();
        let mut starts = Vec::with_capacity(n);
        let mut jumps: Vec<(Label, usize)> = Vec::new();
        for (pc, ins) in callee.code.iter().enumerate() {
            starts.push(self.f.label_here());
            let last = pc + 1 == n;
            let (a, b, c) = (ins.a(), ins.b(), ins.c());
            let Some(op) = ins.op() else {
                continue;
            };
            match op {
                Op::MOVE | Op::NEGI | Op::NEGF => {
                    self.emit(Instruction::abc(op, at(a), at(b), 0), span);
                }
                Op::LOADK => self.emit(Instruction::abx(op, at(a), ins.bx()), span),
                Op::LOADI | Op::LOADF => {
                    self.emit(Instruction::asbx(op, at(a), ins.sbx()), span);
                }
                Op::LOADBOOL | Op::LOADNIL => {
                    self.emit(Instruction::abc(op, at(a), b, c), span);
                }
                Op::ADDI | Op::SUBI | Op::MULI | Op::LTI | Op::LEI | Op::EQI => {
                    self.emit(Instruction::abc(op, at(a), at(b), at(c)), span);
                }
                Op::ADDF | Op::SUBF | Op::MULF | Op::DIVF | Op::LTF | Op::LEF | Op::EQF => {
                    self.emit(Instruction::abc(op, at(a), at(b), at(c)), span);
                }
                Op::ADDII | Op::SUBII | Op::MULII => {
                    self.emit(Instruction::abc(op, at(a), at(b), c), span);
                }
                Op::GETF => self.emit(Instruction::abc(op, at(a), at(b), c), span),
                Op::SETF => self.emit(Instruction::abc(op, at(a), b, at(c)), span),
                Op::GETFIDX => {
                    self.emit(Instruction::abc(Op::GETF, table, recv, b), span);
                    self.emit(Instruction::abc(Op::GETIDX, at(a), table, at(c)), span);
                }
                Op::SETFIDX => {
                    self.emit(Instruction::abc(Op::GETF, table, recv, a), span);
                    self.emit(Instruction::abc(Op::SETIDX, table, at(b), at(c)), span);
                }
                Op::JMP => {
                    let target = (pc as i64 + 1 + ins.sbx() as i64) as usize;
                    jumps.push((self.emit_jump(Op::JMP, 0, span), target));
                }
                Op::JLTI | Op::JLEI | Op::JGTI | Op::JGEI | Op::JEQI | Op::JNEI => {
                    self.emit(Instruction::abc(op, at(a), at(b), c), span);
                }
                Op::JLTF | Op::JLEF | Op::JGTF | Op::JGEF => {
                    self.emit(Instruction::abc(op, at(a), at(b), c), span);
                }
                Op::RET1 => {
                    if a != 0 {
                        self.emit(Instruction::abc(Op::MOVE, recv, at(a), 0), span);
                    }
                    if !last {
                        to_end.push(self.emit_jump(Op::JMP, 0, span));
                    }
                }
                Op::RET0 => {
                    // What a call that returns nothing leaves, where
                    // anything reads it: nil.
                    if wanted {
                        self.emit(Instruction::abc(Op::LOADNIL, recv, 0, 0), span);
                    }
                    if !last {
                        to_end.push(self.emit_jump(Op::JMP, 0, span));
                    }
                }
                // The tests against an immediate, nil and truth: `A` alone
                // is a register.
                _ => self.emit(Instruction::abc(op, at(a), b, c), span),
            }
        }
        for (label, target) in jumps {
            self.patch_to(label, starts[target])?;
        }
        for label in to_end {
            self.patch_here(label)?;
        }
        self.free_to(m);
        Ok(true)
    }
}
