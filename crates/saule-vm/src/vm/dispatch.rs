//! The interpreter loop.
//!
//! **Do not turn `proto` and `chunk` into borrows.** Reading them as
//! `&*Rc::as_ptr(..)` measured **44% slower on `loop_arith`**, which does not
//! make a single call. A `Proto` holds a `RefCell` of inline caches, so a
//! shared reference to one is not `readonly`, and the loop writes registers
//! through `&mut self` the whole time — as borrows they stop being
//! loop-invariant and every constant-pool and code access reloads. Owning
//! them is what tells LLVM the pointers cannot move. Tried twice, with and
//! without hoisting the code pointer separately.
//!
//! **They are no longer *cloned* either, and that is a different thing.**
//! This paragraph used to open "they are cloned out of the frame on every
//! activation — a call *and* a return — and removing those two refcount
//! pairs looks like free money", and it does not: four refcount pairs per
//! call round trip was real money. What the 44% measured was *borrowing*.
//! `ManuallyDrop<Rc<_>>` built by `ptr::read` is neither — the same pointer
//! in a register and the same type to codegen as the clone, so it keeps
//! whatever the owned form was buying and drops only the traffic. Worth
//! **`fib` −7.8%, `mandel` −5.5%, `closure` −4.8%**. See the `SAFETY` note at
//! the read for why the pointee cannot go away: it is `shared`, not the
//! frame, that keeps every `Proto` and `Chunk` alive for the whole call.
//!
//! The lesson generalises past this function: "owned beats borrowed here"
//! and "the refcount is load-bearing" are two claims, and the measurement
//! only ever supported the first.
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

use saule_interpreter::value::{SauleStr, TableObject, VmFunctionRef};
use saule_interpreter::{RuntimeError, Value};

use crate::chunk::{Chunk, Proto};
use crate::op::{Instruction, Op};

use super::ops::{
    cast_holds, field_slot_err, float_in_range, index_array, int_in_range, jump, operand_err,
    shift, snapshot_pairs,
};
use super::call::Site;
use super::{ALL_RESULTS, Closure, Upvalue, Vm};

impl Vm {
    /// Build `CONCAT`'s result string into register `dst`.
    ///
    /// Kept out of line, and that is the point rather than a tidiness
    /// preference. `execute_loop` is one enormous function, and every local
    /// a single arm needs — here a `String`, its capacity arithmetic and
    /// the drop and unwind paths that come with it — widens the frame and
    /// the register pressure that *every other opcode* is compiled under.
    /// Measured: moving this body out is worth double-digit percentages on
    /// benchmarks that never concatenate anything at all (`fib`, `mandel`,
    /// `loop_arith`), which is the same layout sensitivity the `profile`
    /// feature's note in `Cargo.toml` records from the other direction.
    ///
    /// The call this adds is paid once per `..`, against building a string —
    /// far too small to see on the benchmarks that do concatenate.
    #[inline(never)]
    fn concat(
        &mut self,
        from: usize,
        to: usize,
        dst: usize,
        span: std::ops::Range<usize>,
    ) -> Result<(), RuntimeError> {
        // Two passes, but only the second one renders. The first asks each
        // operand how much room it wants — a question `display_hint`
        // answers without running any user code, so `CONCAT`'s "rendered
        // exactly once" rule survives having a sizing pass in front of it.
        let mut len = 0usize;
        for i in from..=to {
            len += saule_interpreter::eval::ops::display_hint(&self.stack[i]);
        }
        let mut s = String::with_capacity(len);
        for i in from..=to {
            saule_interpreter::eval::ops::display_into(&self.stack[i], span.clone(), &mut s)?;
        }
        *self.reg_mut(dst) = Value::Str(SauleStr::new(s));
        Ok(())
    }

    /// `CLOSURE`: build a closure over the upvalues it captures.
    ///
    /// Out of line for the reason [`Vm::concat`] is, and for the reason the
    /// `DIVI | MODI | POWI` arm below shares one body: an arm's code costs
    /// the whole loop, not just itself. This one carries a `Vec`, a capture
    /// loop and an allocation, and it runs once per lambda *created* — which
    /// on `closure` is once, against ten million calls of the result.
    #[inline(never)]
    fn make_closure(&mut self, proto: &Proto, chunk: &Rc<Chunk>, bx: u16, base: usize, a: usize) {
        let child_idx = proto.protos[bx as usize];
        let child = Rc::clone(chunk.proto(child_idx));
        let mut upvals = Vec::with_capacity(child.upvals.len());
        for desc in &child.upvals {
            upvals.push(if desc.from_parent_stack {
                self.capture_upvalue((base + desc.index as usize) as u32)
            } else {
                self.upvalue(desc.index as usize)
            });
        }
        // Bound to the engine state, so a closure handed to a native — a
        // sort comparator, an iterator step — can run itself when the
        // native calls it back. The one place the loop needs the chunk as
        // an owner rather than a borrow: the closure outlives this frame.
        let cl = VmFunctionRef::new(Closure::bound(child, Rc::clone(chunk), upvals, &self.shared));
        *self.reg_mut(base + a) = Value::VmFunction(cl);
    }

    /// `VARARG`: gather the surplus arguments into an array-style table.
    ///
    /// Outlined with the rest — a `Vec`, a table and an `Rc` for an opcode
    /// only variadic functions ever reach.
    #[inline(never)]
    fn vararg(&mut self, base: usize, a: usize) {
        let n = self.frames.last().expect("frame").n_args as usize;
        let items: Vec<Value> = (a..n.max(a)).map(|i| (*self.reg(base + i)).clone()).collect();
        *self.reg_mut(base + a) =
            Value::Table(Rc::new(RefCell::new(TableObject::from_array(items))));
    }

    /// `NEWVAR`: an enum variant carrying a positional payload.
    #[inline(never)]
    fn new_variant(
        &mut self,
        packed: u32,
        n: usize,
        base: usize,
        a: usize,
        proto: &Proto,
        chunk: &Chunk,
        here: u32,
    ) -> Result<(), RuntimeError> {
        let (e_idx, tag) = ((packed >> 16) as usize, packed & 0xffff);
        // The payload is an array-style table of the positional arguments,
        // matching what the tree-walker's tuple-variant constructor builds —
        // pattern destructuring reads it positionally.
        let items: Vec<Value> = (0..n).map(|i| (*self.reg(base + a + 1 + i)).clone()).collect();
        let payload = Value::Table(Rc::new(RefCell::new(TableObject::from_array(items))));
        let Some(e) = self.shared.enums.get(e_idx) else {
            return Err(RuntimeError::TypeError {
                message: format!("internal: no enum {e_idx}"),
                span: proto.span_at(here),
            });
        };
        let name = chunk.enums[e_idx].variants[tag as usize].name.to_string();
        let v = saule_interpreter::value::EnumVariantObject {
            enum_name: e.name.clone().into(),
            variant_name: name.into(),
            tag,
            value: Some(payload),
            enum_obj: RefCell::new(Some(Rc::clone(e))),
        };
        *self.reg_mut(base + a) = Value::EnumVariant(Rc::new(v));
        Ok(())
    }

    /// `CALLMX`: a method call whose name is only known at run time, handed
    /// to the tree-walker's dynamic member dispatch.
    #[inline(never)]
    fn call_member_by_name(
        &mut self,
        k: u32,
        ins: Instruction,
        base: usize,
        a: usize,
        proto: &Proto,
        chunk: &Chunk,
        here: u32,
    ) -> Result<(), RuntimeError> {
        let key = chunk.constants[k as usize].clone();
        let Value::Str(name) = &key else {
            return Err(RuntimeError::TypeError {
                message: "internal: CALLMX name is not a string".into(),
                span: proto.span_at(here),
            });
        };
        // `A` holds the receiver and `A+1..` the arguments, matching
        // `CALLM` — so a call site can switch between the two without
        // moving anything.
        let n_args = (ins.b() as usize).saturating_sub(1);
        let recv = (*self.reg(base + a)).clone();
        let args: Vec<Value> =
            (0..n_args).map(|i| (*self.reg(base + a + 1 + i)).clone()).collect();
        let vs = saule_interpreter::call_member_dynamic(&recv, name, &args, proto.span_at(here))?;
        let n_ret = if ins.c() == 0 { ALL_RESULTS } else { ins.c() - 1 };
        self.store_results(base + a, &vs, n_ret);
        Ok(())
    }

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
    /// Results land in [`Vm::results`] rather than in a fresh `Vec`. A
    /// re-entrant call is an outermost return once per invocation, so
    /// allocating one per return was a malloc and a free per sort
    /// comparison; [`execute_collecting`](Self::execute_collecting) is the
    /// owning form, for the callers that want a `Vec` anyway.
    pub(crate) fn execute(&mut self) -> Result<(), RuntimeError> {
        #[cfg(feature = "profile")]
        if crate::profile::is_enabled() {
            return self.execute_loop::<true>();
        }
        self.execute_loop::<false>()
    }

    /// [`execute`](Self::execute), handing the results over as a `Vec`.
    pub(crate) fn execute_collecting(&mut self) -> Result<Vec<Value>, RuntimeError> {
        self.execute()?;
        Ok(std::mem::take(&mut self.results))
    }

    fn execute_loop<const PROFILE: bool>(&mut self) -> Result<(), RuntimeError> {
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
            // Read, **not** cloned. See the note on refcounts in this
            // module's header for why these are owned values rather than
            // borrows; this keeps them owned values and stops paying for it.
            //
            // SAFETY: the pointee outlives this loop unconditionally, and not
            // because the frame keeps it alive. Every `Rc<Proto>` the VM ever
            // runs comes from `chunk.proto(idx)`, so the `Chunk` holds a
            // strong reference to it for the chunk's whole life; every
            // `Rc<Chunk>` comes from `self.shared.chunks`, which lives as long
            // as the `Rc<VmShared>` this `Vm` holds — which is the entire
            // call. So a `Proto` cannot be freed while `execute_loop` runs,
            // whatever happens to the frame that named it: `pop_frame`
            // dropping the frame, or `enter_tail_frame` overwriting it in
            // place, both leave `shared` holding the last word.
            //
            // `ManuallyDrop` rather than a reference on purpose. These are
            // re-derived on every call *and* every return, so the four
            // refcount pairs a round trip paid here were pure overhead — but
            // the header records that turning them into borrows measured 44%
            // slower on `loop_arith`. A `ManuallyDrop<Rc<T>>` is the same
            // pointer in a register and the same type to codegen, so it keeps
            // whatever made the owned form fast and drops only the traffic.
            let (proto, chunk, base, mut pc) = {
                let f = self.frames.last().expect("frame");
                (
                    unsafe { std::mem::ManuallyDrop::new(std::ptr::read(&f.proto)) },
                    unsafe { std::mem::ManuallyDrop::new(std::ptr::read(&f.chunk)) },
                    f.base as usize,
                    f.pc as usize,
                )
            };
            let code: &[Instruction] = &proto.code;
            // The previous instruction of *this* activation, for the pair
            // histogram. Reset on every `continue 'reentry` — a call or a
            // return — because a pair only means something within one
            // proto, and only the emitter's own neighbours are fusable.
            let mut prev: Option<(u32, Op)> = None;

            loop {
                // No `pc >= code.len()` test, and that is the verifier's
                // doing (§17): every proto ends in a terminator, every jump
                // lands strictly inside the code, and `EXTRAARG` consumption
                // never steps past the instruction that owns it — so `pc` is
                // in range by construction, and re-establishing it cost a
                // compare and a `len` load on *every instruction executed*.
                debug_assert!(pc < code.len(), "verify_proto keeps pc inside the code");
                // SAFETY: `pc < code.len()` was just checked, and the
                // verifier proved every opcode byte in this proto names a
                // real `Op` (§17). Decoding it through `from_u8` cost an
                // `Option` and a bounds check on every instruction executed
                // to re-establish a fact the chunk cannot violate.
                let ins = unsafe { *code.get_unchecked(pc) };
                pc += 1;
                let here = (pc - 1) as u32;

                debug_assert!(ins.op().is_some(), "verify_proto rejects unknown opcodes");
                let op = unsafe { *Op::ALL.get_unchecked(ins.raw_op() as usize) };

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

                // ---- an arm per *hot* opcode, spelled once -------------------------
                //
                // Thirteen families used to share an arm and then re-`match op`
                // inside it — `ADDI | SUBI | MULI | …` and then
                // `match op { Op::ADDI => … }`. That is two indirect branches per
                // instruction where the point of a jump table is to have one, and
                // it collapsed every arithmetic opcode onto a single back-edge, so
                // the branch predictor saw one site with six targets instead of six
                // sites with one target each. These macros give each opcode its own
                // arm without spelling the operand plumbing out thirteen times.
                //
                // **Splitting is not free and not applied uniformly.** An arm of its
                // own buys a dispatch target and costs the code it holds, in a
                // function whose sheer size is worth 2-3% (see this module's
                // header). Splitting all thirty-four opcodes measured *−7% on
                // `loop_arith` and `mandel` but +7% on `interp` and +10% on
                // `entity`* — the branchy programs paying for arms they never
                // execute. So `--profile-bytecode` picked the list: an opcode the
                // suite actually retires gets an arm, and the rest keep the shared
                // one with its inner `match`, which costs nothing it does not run.
                // The regrouped arms carry that reasoning individually.
                //
                // The operand names are macro parameters rather than identifiers
                // bound in the body because `macro_rules!` hygiene would otherwise
                // hide them from the expression passed in. `$e` expands inline in
                // the function, so an arm that has to fail can still `return Err`
                // out of the loop exactly as it did when it was a nested `match`.

                /// `R[A] = R[B] op R[C]`, integers.
                macro_rules! int_arith {
                    (|$l:ident, $r:ident| $e:expr) => {{
                        let ($l, $r) = self.int_pair(base, ins, &proto, here)?;
                        *self.reg_mut(base + a) = Value::Int($e);
                    }};
                }

                /// `R[A] = R[B] op sC`, integer against a signed 8-bit immediate.
                macro_rules! int_arith_imm {
                    (|$l:ident, $r:ident| $e:expr) => {{
                        let $l = self.int_at(base + ins.b() as usize, &proto, here)?;
                        let $r = ins.sc();
                        *self.reg_mut(base + a) = Value::Int($e);
                    }};
                }

                /// `R[A] = R[B] op R[C]`, floats.
                macro_rules! float_arith {
                    (|$l:ident, $r:ident| $e:expr) => {{
                        let ($l, $r) = self.float_pair(base, ins, &proto, here)?;
                        *self.reg_mut(base + a) = Value::Float($e);
                    }};
                }

                /// Fused compare-and-branch: skip the following `JMP` when the
                /// comparison holds. `R[A]` against `R[B]`, integers.
                macro_rules! jump_int {
                    (|$l:ident, $r:ident| $e:expr) => {{
                        let $l = self.int_at(base + a, &proto, here)?;
                        let $r = self.int_at(base + ins.b() as usize, &proto, here)?;
                        if $e {
                            pc += 1;
                        }
                    }};
                }

                /// [`jump_int`] against a signed 8-bit immediate — `fib`'s `n < 2`.
                macro_rules! jump_int_imm {
                    (|$l:ident, $r:ident| $e:expr) => {{
                        let $l = self.int_at(base + a, &proto, here)?;
                        let $r = ins.sc();
                        if $e {
                            pc += 1;
                        }
                    }};
                }

                /// [`jump_int`], floats.
                macro_rules! jump_float {
                    (|$l:ident, $r:ident| $e:expr) => {{
                        let $l = self.float_at(base + a, &proto, here)?;
                        let $r = self.float_at(base + ins.b() as usize, &proto, here)?;
                        if $e {
                            pc += 1;
                        }
                    }};
                }


                match op {
                    // ---- §15.1 moves and constants -----------------------
                    Op::MOVE => {
                        *self.reg_mut(base + a) = (*self.reg(base + ins.b() as usize)).clone();
                    }
                    Op::LOADK => {
                        *self.reg_mut(base + a) = chunk.constants[ins.bx() as usize].clone();
                    }
                    Op::LOADI => *self.reg_mut(base + a) = Value::Int(ins.sbx() as i64),
                    Op::LOADF => *self.reg_mut(base + a) = Value::Float(ins.sbx() as f64),
                    Op::LOADBOOL => *self.reg_mut(base + a) = Value::Bool(ins.b() != 0),
                    Op::LOADNIL => {
                        for i in 0..=ins.b() as usize {
                            *self.reg_mut(base + a + i) = Value::Nil;
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
                        *self.reg_mut(base + a) = v;
                    }
                    Op::SETUPVAL => {
                        let v = (*self.reg(base + a)).clone();
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
                        *self.reg_mut(base + a) = v;
                    }
                    Op::SETMOD => {
                        self.shared.modules.borrow_mut()[ins.bx() as usize] =
                            (*self.reg(base + a)).clone();
                    }
                    Op::CLOSURE => self.make_closure(&proto, &chunk, ins.bx(), base, a),

                    // ---- §15.3 integer arithmetic ------------------------
                    Op::ADDI => int_arith!(|l, r| l.wrapping_add(r)),
                    Op::SUBI => int_arith!(|l, r| l.wrapping_sub(r)),
                    Op::MULI => int_arith!(|l, r| l.wrapping_mul(r)),
                    // Regrouped deliberately, and the profile is the reason.
                    // Splitting an opcode into its own arm buys a dispatch
                    // target; it costs the code that arm holds, in a function
                    // whose own size is worth 2-3% (see this module's header).
                    // These three are the bulkiest arithmetic arms — two
                    // error paths and a `format!` — and `--profile-bytecode`
                    // finds them at 1.3% of `sort` and *zero* everywhere else
                    // in the suite. So they share an arm, and the inner
                    // `match` they pay for is one that essentially never runs.
                    Op::DIVI | Op::MODI | Op::POWI => {
                        let (l, r) = self.int_pair(base, ins, &proto, here)?;
                        let out = match op {
                            Op::DIVI => {
                                if r == 0 {
                                    return Err(RuntimeError::DivisionByZero {
                                        span: proto.span_at(here),
                                    });
                                }
                                l.wrapping_div(r)
                            }
                            Op::MODI => {
                                if r == 0 {
                                    return Err(RuntimeError::DivisionByZero {
                                        span: proto.span_at(here),
                                    });
                                }
                                l.wrapping_rem(r)
                            }
                            // `integer ^ integer` stays an integer, so a
                            // negative exponent has no answer — an error
                            // rather than a silent 0, matching `int_op`.
                            _ => {
                                let Ok(exp) = u32::try_from(r) else {
                                    return Err(RuntimeError::TypeError {
                                        message: format!(
                                            "`^` on integers requires a non-negative exponent, \
                                             got {r} — use floats (`float(base) ^ {r}.0`) for a \
                                             fractional result"
                                        ),
                                        span: proto.span_at(here),
                                    });
                                };
                                l.wrapping_pow(exp)
                            }
                        };
                        *self.reg_mut(base + a) = Value::Int(out);
                    }
                    Op::NEGI => {
                        let v = self.int_at(base + ins.b() as usize, &proto, here)?;
                        *self.reg_mut(base + a) = Value::Int(v.wrapping_neg());
                    }
                    Op::ADDII => int_arith_imm!(|l, imm| l.wrapping_add(imm)),
                    Op::SUBII => int_arith_imm!(|l, imm| l.wrapping_sub(imm)),
                    Op::MULII => int_arith_imm!(|l, imm| l.wrapping_mul(imm)),

                    // ---- §15.4 float arithmetic --------------------------
                    Op::ADDF => float_arith!(|l, r| l + r),
                    Op::SUBF => float_arith!(|l, r| l - r),
                    Op::MULF => float_arith!(|l, r| l * r),
                    // Cold, so grouped: `DIVF` is 0.3% of `mandel` and the
                    // other two never execute in the suite at all. Float
                    // division by zero yields infinity, matching `float_op` —
                    // only integer division errors.
                    Op::DIVF | Op::MODF | Op::POWF => {
                        let (l, r) = self.float_pair(base, ins, &proto, here)?;
                        let out = match op {
                            Op::DIVF => l / r,
                            Op::MODF => l % r,
                            _ => l.powf(r),
                        };
                        *self.reg_mut(base + a) = Value::Float(out);
                    }
                    Op::NEGF => {
                        let v = self.float_at(base + ins.b() as usize, &proto, here)?;
                        *self.reg_mut(base + a) = Value::Float(-v);
                    }

                    // ---- §15.5 bitwise -----------------------------------
                    // Zero executions across all sixteen benchmarks, so
                    // these keep the shared arm: five dispatch targets bought
                    // nothing and cost five arms' worth of code sitting
                    // between the ones that do run.
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
                        *self.reg_mut(base + a) = Value::Int(out);
                    }
                    Op::BNOT => {
                        let v = self.int_at(base + ins.b() as usize, &proto, here)?;
                        *self.reg_mut(base + a) = Value::Int(!v);
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
                        let code_v = self.extra_arg(code.as_ptr(), &mut pc);
                        let Some(op) = crate::op::dynop::decode_binary(code_v) else {
                            return Err(RuntimeError::TypeError {
                                message: format!("internal: ARITHX with unknown operator {code_v}"),
                                span: proto.span_at(here),
                            });
                        };
                        let l = (*self.reg(base + ins.b() as usize)).clone();
                        let r = (*self.reg(base + ins.c() as usize)).clone();
                        let v = saule_interpreter::eval::ops::binary(
                            op,
                            l,
                            r,
                            proto.span_at(here),
                        )?;
                        *self.reg_mut(base + a) = v;
                    }
                    Op::UNARYX => {
                        let code_v = self.extra_arg(code.as_ptr(), &mut pc);
                        let Some(op) = crate::op::dynop::decode_unary(code_v) else {
                            return Err(RuntimeError::TypeError {
                                message: format!("internal: UNARYX with unknown operator {code_v}"),
                                span: proto.span_at(here),
                            });
                        };
                        let v = (*self.reg(base + ins.b() as usize)).clone();
                        *self.reg_mut(base + a) =
                            saule_interpreter::eval::ops::unary(op, v, proto.span_at(here))?;
                    }

                    // ---- §15.7 comparison and branching ------------------
                    Op::JMP => {
                        if a > 0 {
                            self.close_upvalues((base + a - 1) as u32);
                        }
                        pc = jump(pc, ins.sbx());
                    }
                    Op::JLTI => jump_int!(|l, r| l < r),
                    Op::JLEI => jump_int!(|l, r| l <= r),
                    // `JLTI` and `JLEI` above are the two the compiler
                    // actually emits into hot code — the other four are
                    // absent from every profile in the suite, so they share
                    // an arm. "Skip the next instruction" is the convention:
                    // that next instruction is the `JMP` to the false branch.
                    Op::JGTI | Op::JGEI | Op::JEQI | Op::JNEI => {
                        let l = self.int_at(base + a, &proto, here)?;
                        let r = self.int_at(base + ins.b() as usize, &proto, here)?;
                        let take = match op {
                            Op::JGTI => l > r,
                            Op::JGEI => l >= r,
                            Op::JEQI => l == r,
                            _ => l != r,
                        };
                        if take {
                            pc += 1;
                        }
                    }
                    // The same six against a signed 8-bit immediate, so the
                    // `LOADI` that materialised the literal is gone. `fib`'s
                    // `n < 2` is the shape, and it runs once per call.
                    Op::JLTII => jump_int_imm!(|l, imm| l < imm),
                    Op::JLEII => jump_int_imm!(|l, imm| l <= imm),
                    Op::JGTII => jump_int_imm!(|l, imm| l > imm),
                    Op::JGEII | Op::JEQII | Op::JNEII => {
                        let l = self.int_at(base + a, &proto, here)?;
                        let imm = ins.sc();
                        let take = match op {
                            Op::JGEII => l >= imm,
                            Op::JEQII => l == imm,
                            _ => l != imm,
                        };
                        if take {
                            pc += 1;
                        }
                    }
                    Op::JLTF => jump_float!(|l, r| l < r),
                    Op::JLEF => jump_float!(|l, r| l <= r),
                    Op::JGTF => jump_float!(|l, r| l > r),
                    Op::JGEF => jump_float!(|l, r| l >= r),
                    Op::JEQ | Op::JNE => {
                        let eq = (*self.reg(base + a)) == (*self.reg(base + ins.b() as usize));
                        if eq == (op == Op::JEQ) {
                            pc += 1;
                        }
                    }
                    Op::JEQK => {
                        let eq = (*self.reg(base + a)) == chunk.constants[ins.c() as usize];
                        if eq {
                            pc += 1;
                        }
                    }
                    Op::TEST => {
                        if (*self.reg(base + a)).is_truthy() != (ins.c() != 0) {
                            pc += 1;
                        }
                    }
                    Op::TESTSET => {
                        let src = (*self.reg(base + ins.b() as usize)).clone();
                        if src.is_truthy() == (ins.c() != 0) {
                            *self.reg_mut(base + a) = src;
                            pc += 1;
                        }
                    }
                    Op::JNIL | Op::JNOTNIL => {
                        let is_nil = matches!(*self.reg(base + a), Value::Nil);
                        if is_nil == (op == Op::JNIL) {
                            pc += 1;
                        }
                    }
                    // `LTI` is 21.7% of `sort` and absent everywhere else, and
                    // it is the one hot opcode that measured *worse* with an arm
                    // of its own (+2% on `sort`, reproduced across passes). That
                    // fits what this benchmark is: the `CASTUNWRAP` fusion cut
                    // 30% of its instructions for 2.3% of its clock, so `sort`
                    // is bound by the native→VM crossing per comparison, not by
                    // dispatch — an extra target buys nothing there and the code
                    // it displaces still costs. Grouped.
                    Op::LTI | Op::LEI | Op::EQI => {
                        let (l, r) = self.int_pair(base, ins, &proto, here)?;
                        *self.reg_mut(base + a) = Value::Bool(match op {
                            Op::LTI => l < r,
                            Op::LEI => l <= r,
                            _ => l == r,
                        });
                    }
                    Op::LTF | Op::LEF | Op::EQF => {
                        let (l, r) = self.float_pair(base, ins, &proto, here)?;
                        *self.reg_mut(base + a) = Value::Bool(match op {
                            Op::LTF => l < r,
                            Op::LEF => l <= r,
                            _ => l == r,
                        });
                    }
                    Op::EQV => {
                        let eq = (*self.reg(base + ins.b() as usize))
                            == (*self.reg(base + ins.c() as usize));
                        *self.reg_mut(base + a) = Value::Bool(eq);
                    }
                    Op::NOT => {
                        let t = (*self.reg(base + ins.b() as usize)).is_truthy();
                        *self.reg_mut(base + a) = Value::Bool(!t);
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
                            *self.reg_mut(base + a + 3) = Value::Int(from);
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
                            *self.reg_mut(base + a) = Value::Int(next);
                            *self.reg_mut(base + a + 3) = Value::Int(next);
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
                            *self.reg_mut(base + a + 3) = Value::Float(from);
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
                            *self.reg_mut(base + a) = Value::Float(next);
                            *self.reg_mut(base + a + 3) = Value::Float(next);
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
                        let pairs = match self.reg(base + a) {
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
                        *self.reg_mut(base + a) = Value::Table(Rc::new(RefCell::new(
                            TableObject::from_array(pairs),
                        )));
                        *self.reg_mut(base + a + 1) = Value::Int(0);
                        if empty {
                            pc = jump(pc, ins.bx() as i32);
                        }
                    }
                    Op::ITERNEXT => {
                        let i = match self.reg(base + a + 1) {
                            Value::Int(n) => *n as usize,
                            _ => 0,
                        };
                        let (k, v, more) = match self.reg(base + a) {
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
                            *self.reg_mut(base + a + 1) = Value::Int(i as i64 + 1);
                            *self.reg_mut(base + a + 3) = k;
                            *self.reg_mut(base + a + 4) = v;
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
                        let src = (*self.reg(base + a)).clone();
                        self.ensure_stack(base + a + 5);
                        match src {
                            Value::Table(t) => {
                                let pairs = snapshot_pairs(&t.borrow());
                                let empty = pairs.is_empty();
                                *self.reg_mut(base + a) = Value::Table(Rc::new(RefCell::new(
                                    TableObject::from_array(pairs),
                                )));
                                *self.reg_mut(base + a + 1) = Value::Int(0);
                                *self.reg_mut(base + a + 2) = Value::Bool(false);
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
                                *self.reg_mut(base + a + 1) = Value::Nil;
                                *self.reg_mut(base + a + 2) = Value::Bool(true);
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
                                *self.reg_mut(base + a) = driver;
                                *self.reg_mut(base + a + 1) = Value::Nil;
                                *self.reg_mut(base + a + 2) = Value::Bool(true);
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
                        *self.reg_mut(base + a) = Value::Table(Rc::new(RefCell::new(t)));
                    }
                    Op::SETLIST => {
                        let t = self.table_at(base + a, &proto, here)?;
                        let n = ins.b() as usize;
                        let mut t = t.borrow_mut();
                        t.array.reserve(n);
                        for i in 1..=n {
                            t.array.push((*self.reg(base + a + i)).clone());
                        }
                    }
                    Op::GETARR => {
                        let t = self.table_at(base + ins.b() as usize, &proto, here)?;
                        let idx = self.int_at(base + ins.c() as usize, &proto, here)?;
                        let v = {
                            let t = t.borrow();
                            index_array(&t, idx)
                        };
                        *self.reg_mut(base + a) = v;
                    }
                    Op::SETARR => {
                        let t = self.table_at(base + a, &proto, here)?;
                        let idx = self.int_at(base + ins.b() as usize, &proto, here)?;
                        let v = (*self.reg(base + ins.c() as usize)).clone();
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
                                t.get(self.reg(base + ins.c() as usize))
                            }
                        };
                        *self.reg_mut(base + a) = v;
                    }
                    // `t[i]!` — the index and the force-unwrap in one word.
                    // See the opcode's doc for the profile that justifies it.
                    Op::GETIDXU => {
                        let tr = base + ins.b() as usize;
                        let v = {
                            let Value::Table(t) = &self.stack[tr] else {
                                return Err(operand_err(&self.stack[tr], "table", &proto, here));
                            };
                            t.borrow().get(self.reg(base + ins.c() as usize))
                        };
                        if matches!(v, Value::Nil) {
                            return Err(RuntimeError::ForceUnwrapNil { span: proto.span_at(here) });
                        }
                        *self.reg_mut(base + a) = v;
                    }
                    Op::SETMAP | Op::SETMAPK | Op::SETIDX => {
                        let tr = base + a;
                        // The stored value is the one thing that genuinely
                        // moves into the table, so it is the one clone left.
                        let v = (*self.reg(base + ins.c() as usize)).clone();
                        let Value::Table(t) = &self.stack[tr] else {
                            return Err(operand_err(&self.stack[tr], "table", &proto, here));
                        };
                        let r = if op == Op::SETMAPK {
                            t.borrow_mut().set(&chunk.constants[ins.b() as usize], v)
                        } else {
                            t.borrow_mut().set(self.reg(base + ins.b() as usize), v)
                        };
                        r.map_err(|m| RuntimeError::TypeError {
                            message: m,
                            span: proto.span_at(here),
                        })?;
                    }
                    Op::APPEND => {
                        let t = self.table_at(base + a, &proto, here)?;
                        let v = (*self.reg(base + ins.b() as usize)).clone();
                        t.borrow_mut().array.push(v);
                    }
                    Op::LEN => {
                        let v = match self.reg(base + ins.b() as usize) {
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
                        *self.reg_mut(base + a) = v;
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
                        self.concat(from, to, base + a, span)?;
                    }
                    Op::TOSTR => {
                        let s = saule_interpreter::eval::ops::display_value(
                            self.reg(base + ins.b() as usize),
                            proto.span_at(here),
                        )?;
                        *self.reg_mut(base + a) = Value::Str(SauleStr::new(s));
                    }

                    // ---- §15.12 nullability -------------------------------
                    Op::COALESCE => {
                        let v = match self.reg(base + ins.b() as usize) {
                            Value::Nil => (*self.reg(base + ins.c() as usize)).clone(),
                            v => v.clone(),
                        };
                        *self.reg_mut(base + a) = v;
                    }
                    Op::UNWRAPNIL => {
                        let v = (*self.reg(base + ins.b() as usize)).clone();
                        if matches!(v, Value::Nil) {
                            return Err(RuntimeError::ForceUnwrapNil { span: proto.span_at(here) });
                        }
                        *self.reg_mut(base + a) = v;
                    }
                    // `x as T`. The test is the tree-walker's own — deep for
                    // `table<T>`, subclass-aware for classes — because it
                    // *is* the tree-walker's function, not a copy of it.
                    // Never throws: a failed cast is `nil`, and the static
                    // type is `T?`, so the caller already has to handle it.
                    Op::CASTCHK => {
                        // Tested through the register, and cloned only if it
                        // holds — the clone used to happen first, so a cast
                        // that failed paid a refcount pair to produce a value
                        // it then threw away for a `nil`.
                        let src = base + ins.b() as usize;
                        let ok = cast_holds(&chunk, ins.c() as usize, self.reg(src));
                        let v = if ok { (*self.reg(src)).clone() } else { Value::Nil };
                        *self.reg_mut(base + a) = v;
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
                        let src = base + ins.b() as usize;
                        let ok = cast_holds(&chunk, ins.c() as usize, self.reg(src));
                        if !ok || matches!(*self.reg(src), Value::Nil) {
                            return Err(RuntimeError::ForceUnwrapNil { span: proto.span_at(here) });
                        }
                        let v = (*self.reg(src)).clone();
                        *self.reg_mut(base + a) = v;
                    }

                    // ---- §15.13 calls and returns -------------------------
                    Op::CALL => {
                        let callee_abs = base + a;
                        let n_args = self.arg_count(ins.b(), callee_abs + 1);
                        let n_ret = if ins.c() == 0 { ALL_RESULTS } else { ins.c() - 1 };
                        let site = Site::Code(&proto, here);
                        if self.dispatch_call(callee_abs, n_args, n_ret, &site, pc)? {
                            continue 'reentry;
                        }
                    }
                    Op::CALLK => {
                        // Packed 8/16: the module, then the proto. A proto
                        // index means nothing outside its own chunk, and
                        // `self.super()` on a parent from another module is
                        // exactly this call crossing that boundary.
                        let packed = self.extra_arg(code.as_ptr(), &mut pc);
                        let (tm, target) = ((packed >> 16) as usize, packed & 0xFFFF);
                        let n_args = self.arg_count(ins.b(), base + a);
                        let n_ret = if ins.c() == 0 { ALL_RESULTS } else { ins.c() - 1 };
                        self.frames.last_mut().expect("frame").pc = pc as u32;
                        let dst = (base + a) as u32;
                        self.enter_static(tm, target, dst, n_args, dst, n_ret, &Site::Code(&proto, here))?;
                        continue 'reentry;
                    }
                    Op::CALLNAT => {
                        // The callee is read **out of the constant pool in
                        // place**. It used to be cloned first, and a `Value`
                        // clone of a heap variant is a call to an
                        // `#[inline(never)]` `clone_heap` plus a matching
                        // out-of-line drop at the end of the statement — two
                        // function calls per native call, to borrow a
                        // `Rc<NativeFn>` the chunk owns for its whole life
                        // anyway. `chunk` is a loop local rather than a field
                        // of `self`, so indexing it does not borrow `self`
                        // and `call_native` can still take `&mut`.
                        //
                        // Kept to one call so the arm stays small: spelling
                        // the operand decoding out here instead cost
                        // `loop_arith` 5.6%, and `loop_arith` makes exactly
                        // one native call in its whole run — pure code
                        // layout, the same effect the opcode-splitting item
                        // measured in both directions.
                        let k = self.extra_arg(code.as_ptr(), &mut pc);
                        self.call_nat(&chunk, k, ins, base, a, &proto, here)?;
                    }
                    // ---- §6.4 tail calls ----------------------------
                    Op::TAILCALL => {
                        let callee_abs = base + a;
                        let n_args = self.arg_count(ins.b(), callee_abs + 1);
                        let site = Site::Code(&proto, here);
                        match self.stack[callee_abs].clone() {
                            // Only a bytecode function has a frame to
                            // replace, and it is exactly what the
                            // tree-walker trampolines: `Flow::TailCall` is
                            // built for `Value::Function` and nothing else.
                            Value::VmFunction(handle) => {
                                self.enter_tail_frame(handle, callee_abs + 1, n_args, &site)?;
                                continue 'reentry;
                            }
                            // A native, a constructor, anything else
                            // callable: no Saule frame to replace, so it is
                            // an ordinary call made right here and returned
                            // — word for word what `Stmt::Return` does.
                            other => {
                                self.call_native(&other, callee_abs, n_args, ALL_RESULTS, &site)?;
                                let n = self.arg_count(0, callee_abs);
                                if self.pop_frame(callee_abs, n) {
                                    return Ok(());
                                }
                                if self.frames.len() < entry_depth {
                                    self.results.clear();
                                    return Ok(());
                                }
                                continue 'reentry;
                            }
                        }
                    }
                    Op::TAILCALLK => {
                        let packed = self.extra_arg(code.as_ptr(), &mut pc);
                        let (tm, target) = ((packed >> 16) as usize, packed & 0xFFFF);
                        let n_args = self.arg_count(ins.b(), base + a);
                        let tc = Rc::clone(&self.shared.chunks[tm]);
                        let handle = self.closure_for(&tc, target);
                        self.enter_tail_frame(handle, base + a, n_args, &Site::Code(&proto, here))?;
                        continue 'reentry;
                    }
                    Op::TAILCALLS => {
                        let packed = self.extra_arg(code.as_ptr(), &mut pc);
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
                        self.enter_tail_frame(handle, base + a, n_args, &Site::Code(&proto, here))?;
                        continue 'reentry;
                    }
                    Op::RET0 => {
                        if self.pop_frame(base, 0) {
                            return Ok(());
                        }
                        if self.frames.len() < entry_depth {
                            self.results.clear();
                            return Ok(());
                        }
                        continue 'reentry;
                    }
                    Op::RET1 => {
                        if self.pop_frame(base + a, 1) {
                            return Ok(());
                        }
                        if self.frames.len() < entry_depth {
                            self.results.clear();
                            return Ok(());
                        }
                        continue 'reentry;
                    }
                    Op::RET => {
                        let n = self.arg_count(ins.b(), base + a);
                        if self.pop_frame(base + a, n) {
                            return Ok(());
                        }
                        if self.frames.len() < entry_depth {
                            self.results.clear();
                            return Ok(());
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
                        *self.reg_mut(base + a) = Value::Instance(Rc::new(RefCell::new(inst)));
                    }
                    Op::GETF => {
                        let slot = ins.c() as usize;
                        let v = match self.reg(base + ins.b() as usize) {
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
                        *self.reg_mut(base + a) = v;
                    }
                    Op::SETF => {
                        let slot = ins.b() as usize;
                        let v = (*self.reg(base + ins.c() as usize)).clone();
                        // The VM's instance write, the counterpart of
                        // `InstanceObject::set_field`. See `saule_interpreter::gc`.
                        saule_interpreter::gc::on_store(&v);
                        match self.reg(base + a) {
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
                        let yes = match self.reg(base + ins.b() as usize) {
                            Value::Instance(i) => {
                                self.is_a(&i.borrow().class, want)
                            }
                            _ => false,
                        };
                        *self.reg_mut(base + a) = Value::Bool(yes);
                    }
                    Op::GETSTAT => {
                        let (cls, slot) = (ins.b() as usize, ins.c() as usize);
                        let v = self
                            .shared.statics
                            .get(cls)
                            .and_then(|s| s.borrow().get(slot).cloned());
                        match v {
                            Some(v) => *self.reg_mut(base + a) = v,
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
                        let v = (*self.reg(base + a)).clone();
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
                        let (tm, target) = self.vtable_lookup_cached(recv, slot, &proto, here)?;
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
                            &Site::Code(&proto, here),
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
                        let slot = self.extra_arg(code.as_ptr(), &mut pc) as usize;
                        let recv = base + a;
                        let n_args = (ins.b() as usize).saturating_sub(1);
                        let n_ret = if ins.c() == 0 { ALL_RESULTS } else { ins.c() - 1 };
                        let (tm, target) = self.vtable_lookup_cached(recv, slot, &proto, here)?;
                        self.frames.last_mut().expect("frame").pc = pc as u32;
                        self.enter_static(
                            tm,
                            target,
                            recv as u32,
                            n_args + 1,
                            recv as u32,
                            n_ret,
                            &Site::Code(&proto, here),
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
                        let packed = self.extra_arg(code.as_ptr(), &mut pc);
                        let (c, iface) = ((packed >> 16) as u8, packed & 0xFFFF);
                        let recv = base + a;
                        let slot = ins.c() as usize;
                        let n_args = (ins.b() as usize).saturating_sub(1);
                        let n_ret = if c == 0 { ALL_RESULTS } else { c - 1 };
                        let (tm, target) = self.itable_lookup_cached(recv, iface, slot, &proto, here)?;
                        self.frames.last_mut().expect("frame").pc = pc as u32;
                        self.enter_static(
                            tm,
                            target,
                            recv as u32,
                            n_args + 1,
                            recv as u32,
                            n_ret,
                            &Site::Code(&proto, here),
                        )?;
                        continue 'reentry;
                    }
                    Op::CALLSTAT => {
                        let packed = self.extra_arg(code.as_ptr(), &mut pc);
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
                        self.enter_static(tm, target, dst, n_args, dst, n_ret, &Site::Code(&proto, here))?;
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
                            Some(v) => *self.reg_mut(base + a) = Value::EnumVariant(v),
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
                        let packed = self.extra_arg(code.as_ptr(), &mut pc);
                        let n = (ins.b() as usize).saturating_sub(1);
                        self.new_variant(packed, n, base, a, &proto, &chunk, here)?;
                    }
                    Op::GETTAG => {
                        let t = match self.reg(base + ins.b() as usize) {
                            Value::EnumVariant(v) => v.tag as i64,
                            other => return Err(operand_err(other, "enum", &proto, here)),
                        };
                        *self.reg_mut(base + a) = Value::Int(t);
                    }
                    Op::SWITCH => {
                        let table = &chunk.jump_tables[ins.bx() as usize];
                        let tag = match self.reg(base + a) {
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
                        let t = match self.reg(base + a) {
                            Value::EnumVariant(v) => v.tag,
                            _ => u32::MAX,
                        };
                        if t == ins.b() as u32 {
                            pc += 1;
                        }
                    }
                    Op::UNWRAP => {
                        let v = match self.reg(base + ins.b() as usize) {
                            // A variant with no declared value answers with
                            // its own **name**, not nil — `read_member`'s
                            // rule for `.value`, and the reason
                            // `Direction.North.value` is `"North"`. This
                            // read nil until `GETFX` let `enums.sau` compile
                            // and `SAULE_DIFF=1` put the two side by side.
                            Value::EnumVariant(v) => v.value.clone().unwrap_or_else(|| {
                                Value::Str(v.variant_name.clone())
                            }),
                            other => return Err(operand_err(other, "enum", &proto, here)),
                        };
                        *self.reg_mut(base + a) = v;
                    }

                    // ---- §15.15 errors -----------------------------------
                    Op::THROW => {
                        let v = (*self.reg(base + a)).clone();
                        self.frames.last_mut().expect("frame").pc = pc as u32;
                        // Returning at all means a handler took it — an
                        // unhandled throw leaves through the `?`. Re-enter with
                        // the frame and pc the handler restored.
                        self.unwind(v, &proto, here)?;
                        continue 'reentry;
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
                        let recv = (*self.reg(base + ins.b() as usize)).clone();
                        let v = saule_interpreter::read_member_dynamic(
                            &recv,
                            name,
                            proto.span_at(here),
                        )?;
                        *self.reg_mut(base + a) = v;
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
                        let recv = (*self.reg(base + a)).clone();
                        let v = (*self.reg(base + ins.c() as usize)).clone();
                        saule_interpreter::write_member_dynamic(
                            &recv,
                            name,
                            v,
                            proto.span_at(here),
                        )?;
                    }
                    Op::CALLMX => {
                        let k = self.extra_arg(code.as_ptr(), &mut pc);
                        self.call_member_by_name(k, ins, base, a, &proto, &chunk, here)?;
                    }

                    // ---- §19 variadic parameters -------------------------
                    // Everything the caller passed from `A` onward becomes an
                    // array-style table, matching what `bind_params` collects
                    // into `variadic_values`. A call that passed nothing extra
                    // gets an empty table, not nil — the parameter is always a
                    // table.
                    Op::VARARG => self.vararg(base, a),
                    Op::SELFFUNC => {
                        // The frame's own handle — no allocation, no cell,
                        // and therefore no cycle. See the opcode's doc.
                        let f = Rc::clone(&self.frames.last().expect("frame").func);
                        *self.reg_mut(base + a) = Value::VmFunction(f);
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
                        *self.reg_mut(base + a) = Value::Int(top.saturating_sub(win) as i64);
                    }
                    Op::CHKTY => {
                        let ok = self.type_matches(&chunk, base + ins.b() as usize, ins.c() as u32);
                        *self.reg_mut(base + a) = Value::Bool(ok);
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
