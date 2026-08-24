//! The interpreter loop.
//!
//! One function, and it stays one function. The arms borrow state held in
//! loop locals across the whole body — `pc`, `base`, the decoded `code`
//! slice, the constant and proto pools re-derived per frame activation
//! (§5.3) — and the loop is monomorphised twice over `PROFILE` so that a
//! build without profiling pays nothing for it. The Cargo.toml comment
//! records what that costs: the *second copy alone* was worth 2-3% on the
//! call-heavy benchmarks through code layout, without a single profiling
//! instruction executing. A function this sensitive to its own shape is not
//! one to refactor for tidiness — if an arm has to move, measure it.

use std::cell::RefCell;
use std::rc::Rc;

use saule_interpreter::value::{TableObject, VmFunctionRef};
use saule_interpreter::{RuntimeError, Value};

use crate::op::{Instruction, Op};

use super::ops::{
    field_slot_err, float_in_range, index_array, int_in_range, jump, operand_err, shift,
    snapshot_pairs,
};
use super::{ALL_RESULTS, Closure, Upvalue, Vm};

impl Vm {

    // ---- the loop ------------------------------------------------------

    /// Run until the frame this was entered on returns.
    ///
    /// The dispatch loop itself is [`Vm::execute_loop`], generic over
    /// whether it counts what it runs (§16). Picking the copy *here*, once
    /// per entry, keeps the profiling branch out of the loop entirely: the
    /// `PROFILE = false` copy monomorphises every counter and every
    /// thread-local read away.
    ///
    /// **Why the `cfg` as well as the const generic.** Compiling away the
    /// profiling code is not enough — the counting copy merely *existing*
    /// costs 2-3% on `loop_arith`, `fib`, `array`, `closure` and `sort`,
    /// measured against the same tree built without it, with the histogram
    /// switched off and not one counter running. That is code layout, not
    /// work, and it is exactly the size of the wins Phase 5 is chasing. So
    /// the second copy is not compiled at all unless the `profile` feature
    /// asks for it, and `--profile-bytecode` refuses on a binary built
    /// without it rather than reporting nothing and letting the user
    /// conclude their program executed no bytecode.
    ///
    /// The alternative measured worse: one loop with a runtime `bool` costs
    /// up to 8.7% (`loop_arith`), a branch per instruction being precisely
    /// the thing a dispatch loop cannot afford.
    pub(crate) fn execute(&mut self) -> Result<Vec<Value>, RuntimeError> {
        #[cfg(feature = "profile")]
        if crate::profile::is_enabled() {
            return self.execute_loop::<true>();
        }
        self.execute_loop::<false>()
    }

    fn execute_loop<const PROFILE: bool>(&mut self) -> Result<Vec<Value>, RuntimeError> {
        let entry_depth = self.frames.len();

        'reentry: loop {
            // Re-derived once per frame activation, then held in locals
            // across the whole inner loop (§5.3). The clones are what let
            // `code` borrow the proto while `self` is mutated underneath.
            //
            // Read straight off the frame: this runs on every call *and*
            // every return, so the downcast that used to stand here ran
            // about two million times in `fib(28)` to recover two pointers
            // the frame can simply carry. Constants, protos, jump tables and
            // cast types are per chunk, so the chunk follows the frame too —
            // a closure built in one module and called from another must
            // read its own module's pools.
            let (proto, chunk, base, mut pc) = {
                let f = self.frames.last().expect("frame");
                (Rc::clone(&f.proto), Rc::clone(&f.chunk), f.base as usize, f.pc as usize)
            };
            let code: &[Instruction] = &proto.code;
            // The previous instruction of *this* activation, for the pair
            // histogram. Reset on every `continue 'reentry` — a call or a
            // return — because a pair only means something within one
            // proto, and only the emitter's own neighbours are fusable.
            let mut prev: Option<(u32, Op)> = None;

            loop {
                if pc >= code.len() {
                    return Err(RuntimeError::TypeError {
                        message: format!(
                            "internal: ran off the end of `{}` — proto has no terminating RET",
                            proto.label()
                        ),
                        span: proto.span_at(pc.saturating_sub(1) as u32),
                    });
                }
                let ins = code[pc];
                pc += 1;
                let here = (pc - 1) as u32;

                let Some(op) = ins.op() else {
                    return Err(RuntimeError::TypeError {
                        message: format!("internal: unknown opcode {:#04x}", ins.raw_op()),
                        span: proto.span_at(here),
                    });
                };

                if PROFILE {
                    // `Some` only when the last instruction executed was
                    // the word immediately before this one — which is the
                    // adjacency a superinstruction needs. An `EXTRAARG`
                    // consumed by its handler advances `pc` past this test
                    // and correctly breaks the pair.
                    let adjacent = prev.and_then(|(at, op)| (at + 1 == here).then_some(op));
                    crate::profile::record(adjacent, op);
                    prev = Some((here, op));
                }

                let a = ins.a() as usize;

                match op {
                    // ---- §15.1 moves and constants -----------------------
                    Op::MOVE => {
                        self.stack[base + a] = self.stack[base + ins.b() as usize].clone();
                    }
                    Op::LOADK => {
                        self.stack[base + a] = chunk.constants[ins.bx() as usize].clone();
                    }
                    Op::LOADI => self.stack[base + a] = Value::Int(ins.sbx() as i64),
                    Op::LOADF => self.stack[base + a] = Value::Float(ins.sbx() as f64),
                    Op::LOADBOOL => self.stack[base + a] = Value::Bool(ins.b() != 0),
                    Op::LOADNIL => {
                        for i in 0..=ins.b() as usize {
                            self.stack[base + a + i] = Value::Nil;
                        }
                    }
                    Op::EXTRAARG => {
                        return Err(RuntimeError::TypeError {
                            message: "internal: EXTRAARG executed as an instruction".into(),
                            span: proto.span_at(here),
                        });
                    }

                    // ---- §15.2 upvalues, module slots, closures ----------
                    Op::GETUPVAL => {
                        let cell = self.upvalue(ins.b() as usize);
                        let v = match &*cell.borrow() {
                            Upvalue::Open(i) => self.stack[*i as usize].clone(),
                            Upvalue::Closed(v) => v.clone(),
                        };
                        self.stack[base + a] = v;
                    }
                    Op::SETUPVAL => {
                        let v = self.stack[base + a].clone();
                        let cell = self.upvalue(ins.b() as usize);
                        let target = cell.borrow().stack_index();
                        match target {
                            Some(i) => self.stack[i as usize] = v,
                            None => *cell.borrow_mut() = Upvalue::Closed(v),
                        }
                    }
                    Op::CLOSEUP => self.close_upvalues((base + a) as u32),
                    Op::GETMOD => {
                        let v = self.shared.modules.borrow()[ins.bx() as usize].clone();
                        self.stack[base + a] = v;
                    }
                    Op::SETMOD => {
                        self.shared.modules.borrow_mut()[ins.bx() as usize] =
                            self.stack[base + a].clone();
                    }
                    Op::CLOSURE => {
                        let child_idx = proto.protos[ins.bx() as usize];
                        let child = Rc::clone(chunk.proto(child_idx));
                        let mut upvals = Vec::with_capacity(child.upvals.len());
                        for desc in &child.upvals {
                            upvals.push(if desc.from_parent_stack {
                                self.capture_upvalue((base + desc.index as usize) as u32)
                            } else {
                                self.upvalue(desc.index as usize)
                            });
                        }
                        // Bound to the engine state, so a closure handed to a
                        // native — a sort comparator, an iterator step — can
                        // run itself when the native calls it back.
                        let cl = VmFunctionRef::new(Closure::bound(child, Rc::clone(&chunk), upvals, &self.shared));
                        self.stack[base + a] = Value::VmFunction(cl);
                    }

                    // ---- §15.3 integer arithmetic ------------------------
                    Op::ADDI | Op::SUBI | Op::MULI | Op::DIVI | Op::MODI | Op::POWI => {
                        let (l, r) = self.int_pair(base, ins, &proto, here)?;
                        let span = || proto.span_at(here);
                        let out = match op {
                            Op::ADDI => l.wrapping_add(r),
                            Op::SUBI => l.wrapping_sub(r),
                            Op::MULI => l.wrapping_mul(r),
                            Op::DIVI => {
                                if r == 0 {
                                    return Err(RuntimeError::DivisionByZero { span: span() });
                                }
                                l.wrapping_div(r)
                            }
                            Op::MODI => {
                                if r == 0 {
                                    return Err(RuntimeError::DivisionByZero { span: span() });
                                }
                                l.wrapping_rem(r)
                            }
                            _ => {
                                // `integer ^ integer` stays an integer, so a
                                // negative exponent has no answer — an error
                                // rather than a silent 0, matching `int_op`.
                                let Ok(exp) = u32::try_from(r) else {
                                    return Err(RuntimeError::TypeError {
                                        message: format!(
                                            "`^` on integers requires a non-negative exponent, \
                                             got {r} — use floats (`float(base) ^ {r}.0`) for a \
                                             fractional result"
                                        ),
                                        span: span(),
                                    });
                                };
                                l.wrapping_pow(exp)
                            }
                        };
                        self.stack[base + a] = Value::Int(out);
                    }
                    Op::NEGI => {
                        let v = self.int_at(base + ins.b() as usize, &proto, here)?;
                        self.stack[base + a] = Value::Int(v.wrapping_neg());
                    }
                    Op::ADDII | Op::SUBII | Op::MULII => {
                        let l = self.int_at(base + ins.b() as usize, &proto, here)?;
                        let imm = ins.sc();
                        let out = match op {
                            Op::ADDII => l.wrapping_add(imm),
                            Op::SUBII => l.wrapping_sub(imm),
                            _ => l.wrapping_mul(imm),
                        };
                        self.stack[base + a] = Value::Int(out);
                    }

                    // ---- §15.4 float arithmetic --------------------------
                    Op::ADDF | Op::SUBF | Op::MULF | Op::DIVF | Op::MODF | Op::POWF => {
                        let (l, r) = self.float_pair(base, ins, &proto, here)?;
                        let out = match op {
                            Op::ADDF => l + r,
                            Op::SUBF => l - r,
                            Op::MULF => l * r,
                            // Float division by zero yields infinity, matching
                            // `float_op` — only integer division errors.
                            Op::DIVF => l / r,
                            Op::MODF => l % r,
                            _ => l.powf(r),
                        };
                        self.stack[base + a] = Value::Float(out);
                    }
                    Op::NEGF => {
                        let v = self.float_at(base + ins.b() as usize, &proto, here)?;
                        self.stack[base + a] = Value::Float(-v);
                    }

                    // ---- §15.5 bitwise -----------------------------------
                    Op::BAND | Op::BOR | Op::BXOR | Op::SHL | Op::SHR => {
                        let (l, r) = self.int_pair(base, ins, &proto, here)?;
                        let out = match op {
                            Op::BAND => l & r,
                            Op::BOR => l | r,
                            Op::BXOR => l ^ r,
                            Op::SHL => shift(l, r),
                            // A right shift is a left shift by the negated
                            // count; `wrapping_neg` leaves `i64::MIN` alone,
                            // which `shift` already reads as "all bits out".
                            _ => shift(l, r.wrapping_neg()),
                        };
                        self.stack[base + a] = Value::Int(out);
                    }
                    Op::BNOT => {
                        let v = self.int_at(base + ins.b() as usize, &proto, here)?;
                        self.stack[base + a] = Value::Int(!v);
                    }

                    // ---- §15.6 dynamic arithmetic fallback ---------------
                    //
                    // The escape hatch that makes an incomplete type table
                    // safe. Where the front end proved nothing, this runs
                    // `ops::binary` — the tree-walker's own operator logic,
                    // reused rather than reimplemented, so `Op*` overloads,
                    // string coercion and every error message are identical
                    // by construction instead of by care.
                    Op::ARITHX => {
                        let code_v = self.extra_arg(code, &mut pc, &proto, here)?;
                        let Some(op) = crate::op::dynop::decode_binary(code_v) else {
                            return Err(RuntimeError::TypeError {
                                message: format!("internal: ARITHX with unknown operator {code_v}"),
                                span: proto.span_at(here),
                            });
                        };
                        let l = self.stack[base + ins.b() as usize].clone();
                        let r = self.stack[base + ins.c() as usize].clone();
                        let v = saule_interpreter::eval::ops::binary(
                            op,
                            l,
                            r,
                            proto.span_at(here),
                        )?;
                        self.stack[base + a] = v;
                    }
                    Op::UNARYX => {
                        let code_v = self.extra_arg(code, &mut pc, &proto, here)?;
                        let Some(op) = crate::op::dynop::decode_unary(code_v) else {
                            return Err(RuntimeError::TypeError {
                                message: format!("internal: UNARYX with unknown operator {code_v}"),
                                span: proto.span_at(here),
                            });
                        };
                        let v = self.stack[base + ins.b() as usize].clone();
                        self.stack[base + a] =
                            saule_interpreter::eval::ops::unary(op, v, proto.span_at(here))?;
                    }

                    // ---- §15.7 comparison and branching ------------------
                    Op::JMP => {
                        if a > 0 {
                            self.close_upvalues((base + a - 1) as u32);
                        }
                        pc = jump(pc, ins.sbx());
                    }
                    Op::JLTI | Op::JLEI | Op::JGTI | Op::JGEI | Op::JEQI | Op::JNEI => {
                        let l = self.int_at(base + a, &proto, here)?;
                        let r = self.int_at(base + ins.b() as usize, &proto, here)?;
                        let take = match op {
                            Op::JLTI => l < r,
                            Op::JLEI => l <= r,
                            Op::JGTI => l > r,
                            Op::JGEI => l >= r,
                            Op::JEQI => l == r,
                            _ => l != r,
                        };
                        // "Skip the next instruction" — by convention that
                        // next instruction is the JMP to the false branch.
                        if take {
                            pc += 1;
                        }
                    }
                    Op::JLTF | Op::JLEF | Op::JGTF | Op::JGEF => {
                        let l = self.float_at(base + a, &proto, here)?;
                        let r = self.float_at(base + ins.b() as usize, &proto, here)?;
                        let take = match op {
                            Op::JLTF => l < r,
                            Op::JLEF => l <= r,
                            Op::JGTF => l > r,
                            _ => l >= r,
                        };
                        if take {
                            pc += 1;
                        }
                    }
                    Op::JEQ | Op::JNE => {
                        let eq = self.stack[base + a] == self.stack[base + ins.b() as usize];
                        if eq == (op == Op::JEQ) {
                            pc += 1;
                        }
                    }
                    Op::JEQK => {
                        let eq = self.stack[base + a] == chunk.constants[ins.c() as usize];
                        if eq {
                            pc += 1;
                        }
                    }
                    Op::TEST => {
                        if self.stack[base + a].is_truthy() != (ins.c() != 0) {
                            pc += 1;
                        }
                    }
                    Op::TESTSET => {
                        let src = self.stack[base + ins.b() as usize].clone();
                        if src.is_truthy() == (ins.c() != 0) {
                            self.stack[base + a] = src;
                            pc += 1;
                        }
                    }
                    Op::JNIL | Op::JNOTNIL => {
                        let is_nil = matches!(self.stack[base + a], Value::Nil);
                        if is_nil == (op == Op::JNIL) {
                            pc += 1;
                        }
                    }
                    Op::LTI | Op::LEI | Op::EQI => {
                        let (l, r) = self.int_pair(base, ins, &proto, here)?;
                        self.stack[base + a] = Value::Bool(match op {
                            Op::LTI => l < r,
                            Op::LEI => l <= r,
                            _ => l == r,
                        });
                    }
                    Op::LTF | Op::LEF | Op::EQF => {
                        let (l, r) = self.float_pair(base, ins, &proto, here)?;
                        self.stack[base + a] = Value::Bool(match op {
                            Op::LTF => l < r,
                            Op::LEF => l <= r,
                            _ => l == r,
                        });
                    }
                    Op::EQV => {
                        let eq = self.stack[base + ins.b() as usize]
                            == self.stack[base + ins.c() as usize];
                        self.stack[base + a] = Value::Bool(eq);
                    }
                    Op::NOT => {
                        let t = self.stack[base + ins.b() as usize].is_truthy();
                        self.stack[base + a] = Value::Bool(!t);
                    }

                    // ---- §15.8 numeric loops ------------------------------
                    Op::FORPREP_I => {
                        let from = self.int_at(base + a, &proto, here)?;
                        let limit = self.int_at(base + a + 1, &proto, here)?;
                        let step = self.int_at(base + a + 2, &proto, here)?;
                        if step == 0 {
                            return Err(RuntimeError::ZeroStep { span: proto.span_at(here) });
                        }
                        if int_in_range(from, limit, step) {
                            self.stack[base + a + 3] = Value::Int(from);
                        } else {
                            pc = jump(pc, ins.sbx());
                        }
                    }
                    Op::FORLOOP_I => {
                        let i = self.int_at(base + a, &proto, here)?;
                        let limit = self.int_at(base + a + 1, &proto, here)?;
                        let step = self.int_at(base + a + 2, &proto, here)?;
                        // Detect overflow so a too-large step cannot loop
                        // forever — the guard `run_numeric_loop_int` has.
                        let (next, overflow) = i.overflowing_add(step);
                        if !overflow && int_in_range(next, limit, step) {
                            self.stack[base + a] = Value::Int(next);
                            self.stack[base + a + 3] = Value::Int(next);
                            pc = jump(pc, ins.sbx());
                        }
                    }
                    Op::FORPREP_F => {
                        let from = self.float_at(base + a, &proto, here)?;
                        let limit = self.float_at(base + a + 1, &proto, here)?;
                        let step = self.float_at(base + a + 2, &proto, here)?;
                        if step == 0.0 {
                            return Err(RuntimeError::ZeroStep { span: proto.span_at(here) });
                        }
                        if float_in_range(from, limit, step) {
                            self.stack[base + a + 3] = Value::Float(from);
                        } else {
                            pc = jump(pc, ins.sbx());
                        }
                    }
                    Op::FORLOOP_F => {
                        let i = self.float_at(base + a, &proto, here)?;
                        let limit = self.float_at(base + a + 1, &proto, here)?;
                        let step = self.float_at(base + a + 2, &proto, here)?;
                        let next = i + step;
                        if float_in_range(next, limit, step) {
                            self.stack[base + a] = Value::Float(next);
                            self.stack[base + a + 3] = Value::Float(next);
                            pc = jump(pc, ins.sbx());
                        }
                    }

                    // ---- §15.8 generic iteration -------------------------
                    Op::ITERPREP => {
                        // The snapshot is the observable part (§11.2): the
                        // tree-walker copies the array and a *sorted* list of
                        // map entries before iterating, so a table mutated
                        // inside the loop does not change what the loop sees.
                        // Reproducing that exactly is why this materialises
                        // the pairs up front rather than holding a cursor.
                        let pairs = match &self.stack[base + a] {
                            Value::Table(t) => snapshot_pairs(&t.borrow()),
                            other => {
                                return Err(RuntimeError::TypeError {
                                    message: format!(
                                        "cannot iterate a `{}` — `for … in` needs a table",
                                        other.type_name()
                                    ),
                                    span: proto.span_at(here),
                                });
                            }
                        };
                        let empty = pairs.is_empty();
                        self.ensure_stack(base + a + 5);
                        self.stack[base + a] = Value::Table(Rc::new(RefCell::new(
                            TableObject::from_array(pairs),
                        )));
                        self.stack[base + a + 1] = Value::Int(0);
                        if empty {
                            pc = jump(pc, ins.bx() as i32);
                        }
                    }
                    Op::ITERNEXT => {
                        let i = match &self.stack[base + a + 1] {
                            Value::Int(n) => *n as usize,
                            _ => 0,
                        };
                        let (k, v, more) = match &self.stack[base + a] {
                            Value::Table(t) => {
                                let t = t.borrow();
                                if i * 2 + 1 < t.array.len() {
                                    (
                                        t.array[i * 2].clone(),
                                        t.array[i * 2 + 1].clone(),
                                        true,
                                    )
                                } else {
                                    (Value::Nil, Value::Nil, false)
                                }
                            }
                            _ => (Value::Nil, Value::Nil, false),
                        };
                        if more {
                            self.stack[base + a + 1] = Value::Int(i as i64 + 1);
                            self.stack[base + a + 3] = k;
                            self.stack[base + a + 4] = v;
                            pc = jump(pc, ins.sbx());
                        }
                    }
                    Op::ITERPREPX => {
                        // The dynamic form of `ITERPREP`, for a source the
                        // front end could not prove. It **dispatches** on
                        // the runtime value exactly as `exec_for_in`'s
                        // `match` does, and writes a mode flag to `R[A+2]`
                        // that the compiler's per-step `TEST` reads.
                        //
                        // Normalising both sources into one driver protocol
                        // was the tempting design and it is wrong: a table
                        // has no nil terminator, and Saule stores a nil
                        // rather than deleting the key, so a table holding
                        // one would end a single-variable loop early here
                        // and run to completion under the tree-walker.
                        let src = self.stack[base + a].clone();
                        self.ensure_stack(base + a + 5);
                        match src {
                            Value::Table(t) => {
                                let pairs = snapshot_pairs(&t.borrow());
                                let empty = pairs.is_empty();
                                self.stack[base + a] = Value::Table(Rc::new(RefCell::new(
                                    TableObject::from_array(pairs),
                                )));
                                self.stack[base + a + 1] = Value::Int(0);
                                self.stack[base + a + 2] = Value::Bool(false);
                                if empty {
                                    pc = jump(pc, ins.bx() as i32);
                                }
                            }
                            // `VmFunction` joins the tree-walker's three
                            // callable variants here: a compiled closure is
                            // the *usual* driver under this engine, and
                            // `exec_for_in` omits it only because the
                            // tree-walker never constructs one.
                            Value::Function(_)
                            | Value::Native(_)
                            | Value::NativeClosure(_)
                            | Value::VmFunction(_) => {
                                self.stack[base + a + 1] = Value::Nil;
                                self.stack[base + a + 2] = Value::Bool(true);
                            }
                            Value::Instance(_) => {
                                // `iter()` runs once per *loop*, not once
                                // per step, so the re-entrant call costs
                                // nothing measurable. Routed through the
                                // tree-walker's own dynamic dispatcher so
                                // an `Iterable` cannot behave differently
                                // under the two engines.
                                let vs = saule_interpreter::call_member_dynamic(
                                    &src,
                                    "iter",
                                    &[],
                                    proto.span_at(here),
                                )?;
                                let Some(driver) = vs.into_iter().next() else {
                                    return Err(RuntimeError::TypeError {
                                        message: format!(
                                            "`{}.iter()` returned no value — it must return a function",
                                            src.type_name()
                                        ),
                                        span: proto.span_at(here),
                                    });
                                };
                                if !matches!(
                                    driver,
                                    Value::Function(_)
                                        | Value::Native(_)
                                        | Value::NativeClosure(_)
                                        | Value::VmFunction(_)
                                ) {
                                    return Err(RuntimeError::TypeError {
                                        message: format!(
                                            "`iter()` must return a function, got `{}`",
                                            driver.type_name()
                                        ),
                                        span: proto.span_at(here),
                                    });
                                }
                                self.stack[base + a] = driver;
                                self.stack[base + a + 1] = Value::Nil;
                                self.stack[base + a + 2] = Value::Bool(true);
                            }
                            other => {
                                return Err(RuntimeError::TypeError {
                                    message: format!(
                                        "cannot iterate over a `{}` with `for ... in` — use a table, a function, or a class that implements `Iterable`",
                                        other.type_name()
                                    ),
                                    span: proto.span_at(here),
                                });
                            }
                        }
                    }

                    // ---- §15.9 tables -------------------------------------
                    Op::NEWT => {
                        let mut t = TableObject::new();
                        t.array.reserve(ins.b() as usize);
                        t.map.reserve(ins.c() as usize);
                        self.stack[base + a] = Value::Table(Rc::new(RefCell::new(t)));
                    }
                    Op::SETLIST => {
                        let t = self.table_at(base + a, &proto, here)?;
                        let n = ins.b() as usize;
                        let mut t = t.borrow_mut();
                        t.array.reserve(n);
                        for i in 1..=n {
                            t.array.push(self.stack[base + a + i].clone());
                        }
                    }
                    Op::GETARR => {
                        let t = self.table_at(base + ins.b() as usize, &proto, here)?;
                        let idx = self.int_at(base + ins.c() as usize, &proto, here)?;
                        let v = {
                            let t = t.borrow();
                            index_array(&t, idx)
                        };
                        self.stack[base + a] = v;
                    }
                    Op::SETARR => {
                        let t = self.table_at(base + a, &proto, here)?;
                        let idx = self.int_at(base + ins.b() as usize, &proto, here)?;
                        let v = self.stack[base + ins.c() as usize].clone();
                        let mut t = t.borrow_mut();
                        let n = t.array.len() as i64;
                        if idx >= 1 && idx <= n {
                            t.array[(idx - 1) as usize] = v;
                        } else if idx == n + 1 {
                            t.array.push(v);
                        } else {
                            t.set(&Value::Int(idx), v).map_err(|m| RuntimeError::TypeError {
                                message: m,
                                span: proto.span_at(here),
                            })?;
                        }
                    }
                    // Both index forms borrow the table and the key in place.
                    // Taking `Rc::clone` of the table and a `Value::clone` of
                    // the key cost a refcount pair and a discriminant branch
                    // per index operation — three per iteration of a matrix
                    // inner loop — to produce two operands that are only read.
                    Op::GETMAP | Op::GETMAPK | Op::GETIDX => {
                        let tr = base + ins.b() as usize;
                        let v = {
                            let Value::Table(t) = &self.stack[tr] else {
                                return Err(operand_err(&self.stack[tr], "table", &proto, here));
                            };
                            let t = t.borrow();
                            if op == Op::GETMAPK {
                                t.get(&chunk.constants[ins.c() as usize])
                            } else {
                                t.get(&self.stack[base + ins.c() as usize])
                            }
                        };
                        self.stack[base + a] = v;
                    }
                    // `t[i]!` — the index and the force-unwrap in one word.
                    // See the opcode's doc for the profile that justifies it.
                    Op::GETIDXU => {
                        let tr = base + ins.b() as usize;
                        let v = {
                            let Value::Table(t) = &self.stack[tr] else {
                                return Err(operand_err(&self.stack[tr], "table", &proto, here));
                            };
                            let v = t.borrow().get(&self.stack[base + ins.c() as usize]);
                            v
                        };
                        if matches!(v, Value::Nil) {
                            return Err(RuntimeError::ForceUnwrapNil { span: proto.span_at(here) });
                        }
                        self.stack[base + a] = v;
                    }
                    Op::SETMAP | Op::SETMAPK | Op::SETIDX => {
                        let tr = base + a;
                        // The stored value is the one thing that genuinely
                        // moves into the table, so it is the one clone left.
                        let v = self.stack[base + ins.c() as usize].clone();
                        let Value::Table(t) = &self.stack[tr] else {
                            return Err(operand_err(&self.stack[tr], "table", &proto, here));
                        };
                        let r = if op == Op::SETMAPK {
                            t.borrow_mut().set(&chunk.constants[ins.b() as usize], v)
                        } else {
                            t.borrow_mut().set(&self.stack[base + ins.b() as usize], v)
                        };
                        r.map_err(|m| RuntimeError::TypeError {
                            message: m,
                            span: proto.span_at(here),
                        })?;
                    }
                    Op::APPEND => {
                        let t = self.table_at(base + a, &proto, here)?;
                        let v = self.stack[base + ins.b() as usize].clone();
                        t.borrow_mut().array.push(v);
                    }
                    Op::LEN => {
                        let v = match &self.stack[base + ins.b() as usize] {
                            Value::Table(t) => Value::Int(t.borrow().array_len() as i64),
                            Value::Str(s) => Value::Int(s.chars().count() as i64),
                            // Anything else is `ops::unary`'s to answer, not
                            // ours. Reimplementing the message here made the
                            // engines report `#` on an integer differently —
                            // caught by `SAULE_DIFF=1 ./run_tests.sh`, which
                            // compares diagnostics as well as values.
                            other => saule_interpreter::eval::ops::unary(
                                saule_ast::UnaryOp::Len,
                                other.clone(),
                                proto.span_at(here),
                            )?,
                        };
                        self.stack[base + a] = v;
                    }

                    // ---- §15.14 strings -----------------------------------
                    Op::CONCAT => {
                        // n-ary: the result costs one `String`, where the
                        // tree-walker's left-folded `..` costs n-1.
                        //
                        // Rendering goes through `display_value`, not
                        // `to_display_string`, because a class may overload
                        // `OpToString` and `..` has to honour it (§8.7) —
                        // the same reuse-rather-than-reimplement rule
                        // `ARITHX` follows. Reading the value directly is
                        // what made `"cost: " .. money` print
                        // `<instance of Money>` under this engine and `300c`
                        // under the other, which `SAULE_DIFF=1` caught the
                        // moment the compile-time refusal was lifted.
                        //
                        // Each operand is rendered **exactly once**: an
                        // overload is user code, so the second pass this
                        // used to make to measure the length would run its
                        // side effects twice.
                        let (from, to) = (base + ins.b() as usize, base + ins.c() as usize);
                        let span = proto.span_at(here);
                        let mut parts: Vec<String> = Vec::with_capacity(to + 1 - from);
                        let mut len = 0usize;
                        for i in from..=to {
                            let part = saule_interpreter::eval::ops::display_value(
                                &self.stack[i],
                                span.clone(),
                            )?;
                            len += part.len();
                            parts.push(part);
                        }
                        let mut s = String::with_capacity(len);
                        for p in &parts {
                            s.push_str(p);
                        }
                        self.stack[base + a] = Value::Str(Rc::new(s));
                    }
                    Op::TOSTR => {
                        let s = saule_interpreter::eval::ops::display_value(
                            &self.stack[base + ins.b() as usize],
                            proto.span_at(here),
                        )?;
                        self.stack[base + a] = Value::Str(Rc::new(s));
                    }

                    // ---- §15.12 nullability -------------------------------
                    Op::COALESCE => {
                        let v = match &self.stack[base + ins.b() as usize] {
                            Value::Nil => self.stack[base + ins.c() as usize].clone(),
                            v => v.clone(),
                        };
                        self.stack[base + a] = v;
                    }
                    Op::UNWRAPNIL => {
                        let v = self.stack[base + ins.b() as usize].clone();
                        if matches!(v, Value::Nil) {
                            return Err(RuntimeError::ForceUnwrapNil { span: proto.span_at(here) });
                        }
                        self.stack[base + a] = v;
                    }
                    // `x as T`. The test is the tree-walker's own — deep for
                    // `table<T>`, subclass-aware for classes — because it
                    // *is* the tree-walker's function, not a copy of it.
                    // Never throws: a failed cast is `nil`, and the static
                    // type is `T?`, so the caller already has to handle it.
                    Op::CASTCHK => {
                        let v = self.stack[base + ins.b() as usize].clone();
                        let ok = chunk
                            .cast_types
                            .get(ins.c() as usize)
                            .is_some_and(|t| {
                                saule_interpreter::eval::expr::cast::cast(&v, t)
                            });
                        self.stack[base + a] = if ok { v } else { Value::Nil };
                    }
                    // `(x as T)!` — the two above, fused. See the opcode's
                    // doc for the profile that justifies it.
                    //
                    // The failure path is `UNWRAPNIL`'s, not a new one: a
                    // cast that does not hold yields `nil`, and unwrapping a
                    // `nil` is `ForceUnwrapNil` at this instruction's span.
                    // A missing `cast_types` entry is a malformed chunk, and
                    // `is_some_and` makes it fail the cast rather than panic
                    // — the same choice `CASTCHK` makes.
                    Op::CASTUNWRAP => {
                        let v = self.stack[base + ins.b() as usize].clone();
                        let ok = chunk
                            .cast_types
                            .get(ins.c() as usize)
                            .is_some_and(|t| {
                                saule_interpreter::eval::expr::cast::cast(&v, t)
                            });
                        if !ok || matches!(v, Value::Nil) {
                            return Err(RuntimeError::ForceUnwrapNil { span: proto.span_at(here) });
                        }
                        self.stack[base + a] = v;
                    }

                    // ---- §15.13 calls and returns -------------------------
                    Op::CALL => {
                        let callee_abs = base + a;
                        let n_args = self.arg_count(ins.b(), callee_abs + 1);
                        let n_ret = if ins.c() == 0 { ALL_RESULTS } else { ins.c() - 1 };
                        let span = proto.span_at(here);
                        if self.dispatch_call(callee_abs, n_args, n_ret, span, pc)? {
                            continue 'reentry;
                        }
                    }
                    Op::CALLK => {
                        // Packed 8/16: the module, then the proto. A proto
                        // index means nothing outside its own chunk, and
                        // `self.super()` on a parent from another module is
                        // exactly this call crossing that boundary.
                        let packed = self.extra_arg(code, &mut pc, &proto, here)?;
                        let (tm, target) = ((packed >> 16) as usize, packed & 0xFFFF);
                        let n_args = self.arg_count(ins.b(), base + a);
                        let n_ret = if ins.c() == 0 { ALL_RESULTS } else { ins.c() - 1 };
                        self.frames.last_mut().expect("frame").pc = pc as u32;
                        let dst = (base + a) as u32;
                        self.enter_static(tm, target, dst, n_args, dst, n_ret, proto.span_at(here))?;
                        continue 'reentry;
                    }
                    Op::CALLNAT => {
                        let k = self.extra_arg(code, &mut pc, &proto, here)?;
                        let callee = chunk.constants[k as usize].clone();
                        let n_args = self.arg_count(ins.b(), base + a + 1);
                        let n_ret = if ins.c() == 0 { ALL_RESULTS } else { ins.c() - 1 };
                        let span = proto.span_at(here);
                        self.call_native(&callee, base + a, n_args, n_ret, span)?;
                    }
                    // ---- §6.4 tail calls ----------------------------
                    Op::TAILCALL => {
                        let callee_abs = base + a;
                        let n_args = self.arg_count(ins.b(), callee_abs + 1);
                        let span = proto.span_at(here);
                        match self.stack[callee_abs].clone() {
                            // Only a bytecode function has a frame to
                            // replace, and it is exactly what the
                            // tree-walker trampolines: `Flow::TailCall` is
                            // built for `Value::Function` and nothing else.
                            Value::VmFunction(handle) => {
                                self.enter_tail_frame(handle, callee_abs + 1, n_args, span)?;
                                continue 'reentry;
                            }
                            // A native, a constructor, anything else
                            // callable: no Saule frame to replace, so it is
                            // an ordinary call made right here and returned
                            // — word for word what `Stmt::Return` does.
                            other => {
                                self.call_native(&other, callee_abs, n_args, ALL_RESULTS, span)?;
                                let n = self.arg_count(0, callee_abs);
                                if let Some(vs) = self.pop_frame(callee_abs, n) {
                                    return Ok(vs);
                                }
                                if self.frames.len() < entry_depth {
                                    return Ok(Vec::new());
                                }
                                continue 'reentry;
                            }
                        }
                    }
                    Op::TAILCALLK => {
                        let packed = self.extra_arg(code, &mut pc, &proto, here)?;
                        let (tm, target) = ((packed >> 16) as usize, packed & 0xFFFF);
                        let n_args = self.arg_count(ins.b(), base + a);
                        let tc = Rc::clone(&self.shared.chunks[tm]);
                        let handle = self.closure_for(&tc, target);
                        self.enter_tail_frame(handle, base + a, n_args, proto.span_at(here))?;
                        continue 'reentry;
                    }
                    Op::TAILCALLS => {
                        let packed = self.extra_arg(code, &mut pc, &proto, here)?;
                        let (cls, slot) = ((packed >> 16) as usize, (packed & 0xffff) as usize);
                        let (tm, target) = match self.shared.chunks[0]
                            .classes
                            .get(cls)
                            .and_then(|c| c.static_methods.get(slot).map(|t| (c.module, *t)))
                        {
                            Some(t) => t,
                            None => {
                                return Err(RuntimeError::TypeError {
                                    message: format!(
                                        "internal: no static method {slot} on class {cls}"
                                    ),
                                    span: proto.span_at(here),
                                });
                            }
                        };
                        let n_args = self.arg_count(ins.b(), base + a);
                        let tc = Rc::clone(&self.shared.chunks[tm]);
                        let handle = self.closure_for(&tc, target);
                        self.enter_tail_frame(handle, base + a, n_args, proto.span_at(here))?;
                        continue 'reentry;
                    }
                    Op::RET0 => {
                        if let Some(vs) = self.pop_frame(base, 0) {
                            return Ok(vs);
                        }
                        if self.frames.len() < entry_depth {
                            return Ok(Vec::new());
                        }
                        continue 'reentry;
                    }
                    Op::RET1 => {
                        if let Some(vs) = self.pop_frame(base + a, 1) {
                            return Ok(vs);
                        }
                        if self.frames.len() < entry_depth {
                            return Ok(Vec::new());
                        }
                        continue 'reentry;
                    }
                    Op::RET => {
                        let n = self.arg_count(ins.b(), base + a);
                        if let Some(vs) = self.pop_frame(base + a, n) {
                            return Ok(vs);
                        }
                        if self.frames.len() < entry_depth {
                            return Ok(Vec::new());
                        }
                        continue 'reentry;
                    }

                    // ---- §15.10 classes and instances --------------------
                    Op::NEW => {
                        let idx = ins.bx() as usize;
                        let Some(class) = self.shared.classes.get(idx) else {
                            return Err(RuntimeError::TypeError {
                                message: format!("internal: no class {idx} in this chunk"),
                                span: proto.span_at(here),
                            });
                        };
                        // One allocation of a `Vec<Value>` sized from the
                        // layout — replacing the per-field `String` clone
                        // plus hash insert the tree-walker pays (§8.6).
                        let inst = saule_interpreter::value::InstanceObject::new(Rc::clone(class));
                        self.stack[base + a] = Value::Instance(Rc::new(RefCell::new(inst)));
                    }
                    Op::GETF => {
                        let slot = ins.c() as usize;
                        let v = match &self.stack[base + ins.b() as usize] {
                            Value::Instance(i) => {
                                let i = i.borrow();
                                match i.fields.get(slot) {
                                    Some(v) => v.clone(),
                                    None => {
                                        return Err(field_slot_err(
                                            slot, i.fields.len(), &proto, here,
                                        ));
                                    }
                                }
                            }
                            other => return Err(operand_err(other, "instance", &proto, here)),
                        };
                        self.stack[base + a] = v;
                    }
                    Op::SETF => {
                        let slot = ins.b() as usize;
                        let v = self.stack[base + ins.c() as usize].clone();
                        match &self.stack[base + a] {
                            Value::Instance(i) => {
                                let mut i = i.borrow_mut();
                                let n = i.fields.len();
                                match i.fields.get_mut(slot) {
                                    Some(dst) => *dst = v,
                                    None => return Err(field_slot_err(slot, n, &proto, here)),
                                }
                            }
                            other => return Err(operand_err(other, "instance", &proto, here)),
                        }
                    }
                    Op::ISA => {
                        let want = ins.c() as u32;
                        let yes = match &self.stack[base + ins.b() as usize] {
                            Value::Instance(i) => {
                                self.is_a(&i.borrow().class, want)
                            }
                            _ => false,
                        };
                        self.stack[base + a] = Value::Bool(yes);
                    }
                    Op::GETSTAT => {
                        let (cls, slot) = (ins.b() as usize, ins.c() as usize);
                        let v = self
                            .shared.statics
                            .get(cls)
                            .and_then(|s| s.borrow().get(slot).cloned());
                        match v {
                            Some(v) => self.stack[base + a] = v,
                            None => {
                                return Err(RuntimeError::TypeError {
                                    message: format!("internal: no static {slot} on class {cls}"),
                                    span: proto.span_at(here),
                                });
                            }
                        }
                    }
                    Op::SETSTAT => {
                        let (cls, slot) = (ins.b() as usize, ins.c() as usize);
                        let v = self.stack[base + a].clone();
                        match self.shared.statics.get(cls).map(|s| {
                            let mut s = s.borrow_mut();
                            match s.get_mut(slot) {
                                Some(d) => {
                                    *d = v;
                                    true
                                }
                                None => false,
                            }
                        }) {
                            Some(true) => {}
                            _ => {
                                return Err(RuntimeError::TypeError {
                                    message: format!("internal: no static {slot} on class {cls}"),
                                    span: proto.span_at(here),
                                });
                            }
                        }
                    }
                    Op::CALLM => {
                        // The receiver is `R[A]` and becomes the callee's
                        // `R[0]` — `self` is simply parameter 0 (§6.2), so
                        // the argument window needs no shuffling.
                        let recv = base + a;
                        let slot = ins.c() as usize;
                        let n_args = (ins.b() as usize).saturating_sub(1);
                        let (tm, target) = self.vtable_lookup(recv, slot, &proto, here)?;
                        self.frames.last_mut().expect("frame").pc = pc as u32;
                        // `self` counts as an argument: the frame starts at
                        // the receiver, not past it.
                        self.enter_static(
                            tm,
                            target,
                            recv as u32,
                            n_args + 1,
                            recv as u32,
                            1,
                            proto.span_at(here),
                        )?;
                        continue 'reentry;
                    }
                    Op::CALLM_MR => {
                        // `CALLM` with the vtable slot displaced into
                        // `EXTRAARG`, which frees `C` to say how many
                        // results the call site wants. `CALLM` cannot: `C`
                        // *is* its slot. Emitted only where more or fewer
                        // than one result is asked for — a parallel `local`,
                        // or a `return` passing the callee's results through.
                        let slot = self.extra_arg(code, &mut pc, &proto, here)? as usize;
                        let recv = base + a;
                        let n_args = (ins.b() as usize).saturating_sub(1);
                        let n_ret = if ins.c() == 0 { ALL_RESULTS } else { ins.c() - 1 };
                        let (tm, target) = self.vtable_lookup(recv, slot, &proto, here)?;
                        self.frames.last_mut().expect("frame").pc = pc as u32;
                        self.enter_static(
                            tm,
                            target,
                            recv as u32,
                            n_args + 1,
                            recv as u32,
                            n_ret,
                            proto.span_at(here),
                        )?;
                        continue 'reentry;
                    }
                    Op::CALLIF => {
                        // Interface dispatch: the receiver's class, one
                        // small-map probe into its itable, one vtable index
                        // (§8.4). `C` is already the interface's method
                        // slot, so `EXTRAARG` carries both the interface
                        // index and the result count, packed 8/16 exactly
                        // as `CALLK` packs its module — the same pressure
                        // that gives `CALLM` a second opcode in `CALLM_MR`.
                        let packed = self.extra_arg(code, &mut pc, &proto, here)?;
                        let (c, iface) = ((packed >> 16) as u8, packed & 0xFFFF);
                        let recv = base + a;
                        let slot = ins.c() as usize;
                        let n_args = (ins.b() as usize).saturating_sub(1);
                        let n_ret = if c == 0 { ALL_RESULTS } else { c - 1 };
                        let (tm, target) = self.itable_lookup(recv, iface, slot, &proto, here)?;
                        self.frames.last_mut().expect("frame").pc = pc as u32;
                        self.enter_static(
                            tm,
                            target,
                            recv as u32,
                            n_args + 1,
                            recv as u32,
                            n_ret,
                            proto.span_at(here),
                        )?;
                        continue 'reentry;
                    }
                    Op::CALLSTAT => {
                        let packed = self.extra_arg(code, &mut pc, &proto, here)?;
                        let (cls, slot) = ((packed >> 16) as usize, (packed & 0xffff) as usize);
                        // Resolved against the program-global class table, and
                        // loaded from the module that declared the class.
                        let (tm, target) = match self.shared.chunks[0]
                            .classes
                            .get(cls)
                            .and_then(|c| c.static_methods.get(slot).map(|t| (c.module, *t)))
                        {
                            Some(t) => t,
                            None => {
                                return Err(RuntimeError::TypeError {
                                    message: format!(
                                        "internal: no static method {slot} on class {cls}"
                                    ),
                                    span: proto.span_at(here),
                                });
                            }
                        };
                        let n_args = (ins.b() as usize).saturating_sub(1);
                        self.frames.last_mut().expect("frame").pc = pc as u32;
                        let dst = (base + a) as u32;
                        let n_ret = if ins.c() == 0 { ALL_RESULTS } else { ins.c() - 1 };
                        self.enter_static(tm, target, dst, n_args, dst, n_ret, proto.span_at(here))?;
                        continue 'reentry;
                    }

                    // ---- §15.11 enums and `match` ------------------------
                    Op::VARIANT => {
                        let (e_idx, tag) = chunk.variant_refs[ins.bx() as usize];
                        let v = self
                            .shared.enums
                            .get(e_idx as usize)
                            .and_then(|e| e.variant_by_tag(tag).cloned());
                        match v {
                            Some(v) => self.stack[base + a] = Value::EnumVariant(v),
                            None => {
                                return Err(RuntimeError::TypeError {
                                    message: format!(
                                        "internal: no singleton for enum {e_idx} tag {tag}"
                                    ),
                                    span: proto.span_at(here),
                                });
                            }
                        }
                    }
                    Op::NEWVAR => {
                        let packed = self.extra_arg(code, &mut pc, &proto, here)?;
                        let (e_idx, tag) = ((packed >> 16) as usize, packed & 0xffff);
                        let n = (ins.b() as usize).saturating_sub(1);
                        // The payload is an array-style table of the
                        // positional arguments, matching what the
                        // tree-walker's tuple-variant constructor builds —
                        // pattern destructuring reads it positionally.
                        let items: Vec<Value> =
                            (0..n).map(|i| self.stack[base + a + 1 + i].clone()).collect();
                        let payload = Value::Table(Rc::new(RefCell::new(
                            TableObject::from_array(items),
                        )));
                        let Some(e) = self.shared.enums.get(e_idx) else {
                            return Err(RuntimeError::TypeError {
                                message: format!("internal: no enum {e_idx}"),
                                span: proto.span_at(here),
                            });
                        };
                        let name = chunk.enums[e_idx].variants[tag as usize]
                            .name
                            .to_string();
                        let v = saule_interpreter::value::EnumVariantObject {
                            enum_name: e.name.clone(),
                            variant_name: name,
                            tag,
                            value: Some(payload),
                            enum_obj: RefCell::new(Some(Rc::clone(e))),
                        };
                        self.stack[base + a] = Value::EnumVariant(Rc::new(v));
                    }
                    Op::GETTAG => {
                        let t = match &self.stack[base + ins.b() as usize] {
                            Value::EnumVariant(v) => v.tag as i64,
                            other => return Err(operand_err(other, "enum", &proto, here)),
                        };
                        self.stack[base + a] = Value::Int(t);
                    }
                    Op::SWITCH => {
                        let table = &chunk.jump_tables[ins.bx() as usize];
                        let tag = match &self.stack[base + a] {
                            Value::Int(n) => *n,
                            other => return Err(operand_err(other, "integer", &proto, here)),
                        };
                        // One indexed jump regardless of arm count, where the
                        // tree-walker compares enum and variant *names* once
                        // per arm (§9.2).
                        pc = usize::try_from(tag)
                            .ok()
                            .and_then(|i| table.targets.get(i).copied())
                            .unwrap_or(table.default) as usize;
                    }
                    Op::JIFTAG => {
                        let t = match &self.stack[base + a] {
                            Value::EnumVariant(v) => v.tag,
                            _ => u32::MAX,
                        };
                        if t == ins.b() as u32 {
                            pc += 1;
                        }
                    }
                    Op::UNWRAP => {
                        let v = match &self.stack[base + ins.b() as usize] {
                            // A variant with no declared value answers with
                            // its own **name**, not nil — `read_member`'s
                            // rule for `.value`, and the reason
                            // `Direction.North.value` is `"North"`. This
                            // read nil until `GETFX` let `enums.sau` compile
                            // and `SAULE_DIFF=1` put the two side by side.
                            Value::EnumVariant(v) => v.value.clone().unwrap_or_else(|| {
                                Value::Str(Rc::new(v.variant_name.clone()))
                            }),
                            other => return Err(operand_err(other, "enum", &proto, here)),
                        };
                        self.stack[base + a] = v;
                    }

                    // ---- §15.15 errors -----------------------------------
                    Op::THROW => {
                        let v = self.stack[base + a].clone();
                        self.frames.last_mut().expect("frame").pc = pc as u32;
                        match self.unwind(v, &proto, here)? {
                            // A handler took it: re-enter with the frame and
                            // pc the handler restored.
                            () => continue 'reentry,
                        }
                    }
                    // ---- §8.5 dynamic member dispatch --------------------
                    //
                    // The escape hatch that makes an unproved receiver safe.
                    // Both defer to the tree-walker's own member logic —
                    // reused rather than reimplemented, the same rule
                    // `ARITHX` follows with `ops::binary` — so instance
                    // fields, methods, statics, enum variants, file handles
                    // and every error message are identical by construction
                    // instead of by care.
                    //
                    // §8.5's inline cache would collapse the common
                    // monomorphic case to a slot load; that is Phase 5, with
                    // a benchmark. Correct first.
                    Op::GETFX => {
                        let key = chunk.constants[ins.c() as usize].clone();
                        let Value::Str(name) = &key else {
                            return Err(RuntimeError::TypeError {
                                message: "internal: GETFX key is not a string".into(),
                                span: proto.span_at(here),
                            });
                        };
                        let recv = self.stack[base + ins.b() as usize].clone();
                        let v = saule_interpreter::read_member_dynamic(
                            &recv,
                            name,
                            proto.span_at(here),
                        )?;
                        self.stack[base + a] = v;
                    }
                    Op::SETFX => {
                        // The write counterpart of `GETFX`, deferring to
                        // the tree-walker's own `assign_member` for the
                        // same reason: an instance field, a class static
                        // and a table key are three different writes, and
                        // the compiler learning each one separately is how
                        // the engines diverge.
                        let key = chunk.constants[ins.b() as usize].clone();
                        let Value::Str(name) = &key else {
                            return Err(RuntimeError::TypeError {
                                message: "internal: SETFX key is not a string".into(),
                                span: proto.span_at(here),
                            });
                        };
                        let recv = self.stack[base + a].clone();
                        let v = self.stack[base + ins.c() as usize].clone();
                        saule_interpreter::write_member_dynamic(
                            &recv,
                            name,
                            v,
                            proto.span_at(here),
                        )?;
                    }
                    Op::CALLMX => {
                        let k = self.extra_arg(code, &mut pc, &proto, here)?;
                        let key = chunk.constants[k as usize].clone();
                        let Value::Str(name) = &key else {
                            return Err(RuntimeError::TypeError {
                                message: "internal: CALLMX name is not a string".into(),
                                span: proto.span_at(here),
                            });
                        };
                        // `A` holds the receiver and `A+1..` the arguments,
                        // matching `CALLM` — so a call site can switch
                        // between the two without moving anything.
                        let n_args = (ins.b() as usize).saturating_sub(1);
                        let recv = self.stack[base + a].clone();
                        let args: Vec<Value> = (0..n_args)
                            .map(|i| self.stack[base + a + 1 + i].clone())
                            .collect();
                        let vs = saule_interpreter::call_member_dynamic(
                            &recv,
                            name,
                            &args,
                            proto.span_at(here),
                        )?;
                        let n_ret = if ins.c() == 0 { ALL_RESULTS } else { ins.c() - 1 };
                        self.store_results(base + a, &vs, n_ret);
                    }

                    // ---- §19 variadic parameters -------------------------
                    Op::VARARG => {
                        // Everything the caller passed from `A` onward
                        // becomes an array-style table, matching what
                        // `bind_params` collects into `variadic_values`. A
                        // call that passed nothing extra gets an empty
                        // table, not nil — the parameter is always a table.
                        let n = self.frames.last().expect("frame").n_args as usize;
                        let items: Vec<Value> = (a..n.max(a))
                            .map(|i| self.stack[base + i].clone())
                            .collect();
                        self.stack[base + a] = Value::Table(Rc::new(RefCell::new(
                            TableObject::from_array(items),
                        )));
                    }
                    Op::SELFFUNC => {
                        // The frame's own handle — no allocation, no cell,
                        // and therefore no cycle. See the opcode's doc.
                        let f = Rc::clone(&self.frames.last().expect("frame").func);
                        self.stack[base + a] = Value::VmFunction(f);
                    }
                    Op::NVALS => {
                        // How many values the call left in the window at
                        // `B`. `store_results` set `top` to one past the
                        // last of them when the caller asked for all, so
                        // the count is the distance from the window base —
                        // saturating, because a callee that returned
                        // nothing leaves `top` below it.
                        let win = base + ins.b() as usize;
                        let top = self.frames.last().expect("frame").top as usize;
                        self.stack[base + a] = Value::Int(top.saturating_sub(win) as i64);
                    }
                    Op::CHKTY => {
                        let ok = self.type_matches(&chunk, base + ins.b() as usize, ins.c() as u32);
                        self.stack[base + a] = Value::Bool(ok);
                    }

                    // ---- not yet implemented ------------------------------
                    // Classes, enums/`match`, `try`/`catch`, `for … in`, and
                    // the dynamic arithmetic fallback are Phase 3 (§21.4).
                    other => {
                        return Err(RuntimeError::Unsupported {
                            thing: other.name(),
                            span: proto.span_at(here),
                        });
                    }
                }
            }
        }
    }

}
