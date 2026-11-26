//! Coroutines: functions that suspend part-way and resume where they stopped.
//!
//! ## What a coroutine is, here
//!
//! A register file that keeps its frames between runs. Calls between Saule
//! functions never use the native stack — a call is a frame pushed onto a
//! `Vec` — so suspending a whole call chain is nothing more than leaving
//! the dispatch loop without popping anything, and resuming it is entering
//! the loop again. Coroutines are therefore *stackful*, as Lua's are: a
//! function the body calls can yield on the body's behalf, as deep down as
//! it likes.
//!
//! * **`resume`** is an ordinary method call. It reaches
//!   [`Coroutine::resume`] through the runtime's method dispatch — which the
//!   caller makes through [`Vm::foreign`], so the caller's open upvalues are
//!   parked for the duration, exactly as for any other call that may run
//!   bytecode on another register file. It rebuilds the coroutine's `Vm` and
//!   runs its loop.
//! * **`yield`** is a method call on the yielder, intercepted by the
//!   coroutine's own dispatch loop ([`Vm::try_yield`]). It records where to
//!   continue and leaves the loop through its error path — the one exit
//!   that keeps every frame — carrying [`RuntimeError::Yield`], which
//!   `resume` turns back into the yielded values. The coroutine's own open
//!   upvalues are then parked for as long as it stays suspended: a suspended
//!   register file is not running, and the invariant `Vm::foreign` keeps is
//!   that every open upvalue points into the one that is.
//!
//! ## What may not happen
//!
//! * **Yielding across a native call.** A sort comparator, a `toString`, an
//!   operator overload all run on a register file of their own, under a Rust
//!   frame that cannot be put aside. A `yield` made there is reported, not
//!   honoured — Lua's "attempt to yield across a C-call boundary".
//! * **Resuming a coroutine that is running** — itself, or one further out
//!   that is waiting on the one now running ("normal", in Lua's terms).
//! * **Yielding a coroutine that is not the one running.**
//!
//! A coroutine whose body fails — a runtime error, or a `throw` nothing in
//! it catches — is dead afterwards, and the failure is *reported* rather
//! than raised: `resume` returns `nil`, and `error()` holds the message. That
//! is Lua's `resume`, and it is what lets a program outlive one of its
//! coroutines failing.
//!
//! ## Ownership
//!
//! A suspended coroutine holds its register file but **not** the program's
//! [`VmShared`]: a coroutine kept in a module slot would otherwise close the
//! cycle `VmShared → slot → coroutine → VmShared` and leak the whole
//! program, for the reason [`Closure::shared`](super::Closure) is `Weak`.
//!
//! The register file is a `Vm`, kept whole from one resume to the next: a
//! suspended one holds [`NOBODY`]'s shared state where the program's was,
//! and gets the program's back as it is resumed. It used to be taken apart
//! into its fields as it suspended and built again around them as it
//! resumed, which was the `Vm` copied four times and a box made and freed
//! for every round trip: a fifth of what one cost.
//!
//! The body is handed a separate [`Yielder`], not the coroutine itself, so
//! that a suspended body does not hold its own coroutine alive from its
//! first register; the yielder refers back weakly.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use saule_runtime::value::{CoroutineRef, VmCoroutine, VmFunctionRef};
use saule_runtime::{RuntimeError, Value};

use super::{Vm, VmShared};

thread_local! {
    /// The coroutine whose code is running now, by address, or 0 outside
    /// every coroutine. What tells "running" from "normal" — a coroutine
    /// that is mid-`resume` of another — and a `yield` made inside a
    /// callback from one made from somewhere else entirely.
    static CURRENT: Cell<usize> = const { Cell::new(0) };
}

thread_local! {
    /// The shared state of no program at all: what a suspended coroutine's
    /// `Vm` holds, so that it holds nothing of the program's (see
    /// "Ownership" above). Never run against.
    static NOBODY: Rc<VmShared> = Rc::new(VmShared::nobody());
}

enum State {
    /// Created and never resumed. The first `resume` calls the body.
    Fresh(Rc<VmFunctionRef>),
    /// Stopped at a `yield`: its register file, upvalues parked, with
    /// [`NOBODY`]'s shared state.
    Suspended(Box<Vm>),
    /// Its code is running, or it is waiting on a coroutine it resumed.
    /// Which of the two is [`CURRENT`]'s to say.
    Running,
    /// Finished, or failed. Nothing runs it again.
    Dead,
}

/// A coroutine — what `Coroutine.create` returns.
pub struct Coroutine {
    state: RefCell<State>,
    /// The handle the body is called with. Made once, so every resume hands
    /// the body the same one and a body can compare it.
    yielder: Rc<CoroutineRef>,
    shared: Weak<VmShared>,
    /// Why the body failed, once it has.
    error: RefCell<Option<String>>,
}

/// The handle a coroutine's body is called with: the thing that yields.
pub struct Yielder {
    co: Weak<CoroutineRef>,
    /// The coroutine's address ([`Coroutine::addr`]), once it has one: what
    /// its register file knows it by. Asked on every `yield`, where going
    /// through `co` for it meant a reference taken and given back.
    addr: Cell<usize>,
}

impl Coroutine {
    /// A coroutine that will run `body` when first resumed.
    pub(crate) fn create(body: Rc<VmFunctionRef>, shared: Weak<VmShared>) -> Rc<CoroutineRef> {
        let made = Rc::new_cyclic(|me: &Weak<CoroutineRef>| {
            CoroutineRef::wrap(Coroutine {
                state: RefCell::new(State::Fresh(body)),
                yielder: CoroutineRef::new(Yielder {
                    co: me.clone(),
                    addr: Cell::new(0),
                }),
                shared,
                error: RefCell::new(None),
            })
        });
        // It has an address now, and keeps it: tell its yielder.
        if let Some(co) = made.as_any().downcast_ref::<Coroutine>()
            && let Some(yielder) = co.yielder.as_any().downcast_ref::<Yielder>()
        {
            yielder.addr.set(co.addr());
        }
        made
    }

    /// This coroutine's identity: the address [`Vm::coroutine`] and
    /// [`CURRENT`] hold.
    fn addr(&self) -> usize {
        self as *const Coroutine as usize
    }

    fn status(&self) -> &'static str {
        match &*self.state.borrow() {
            State::Fresh(_) | State::Suspended(_) => "suspended",
            State::Running if CURRENT.with(Cell::get) == self.addr() => "running",
            State::Running => "normal",
            State::Dead => "dead",
        }
    }

    /// Run the coroutine until it yields, returns, or fails.
    ///
    /// * yields — the values passed to `yield`;
    /// * returns — the body's return values;
    /// * fails — `nil`, with the coroutine dead and [`error`](Self::error)
    ///   set;
    /// * already dead — `nil`, so a generator loop can simply run until it
    ///   gets nothing back.
    ///
    /// Resuming a coroutine that is running is the one misuse raised as an
    /// error, because it is never a state a correct program reaches.
    fn resume(
        &self,
        args: &[Value],
        span: std::ops::Range<usize>,
    ) -> Result<Vec<Value>, RuntimeError> {
        match &*self.state.borrow() {
            State::Fresh(_) | State::Suspended(_) => {}
            State::Running => {
                let message = if CURRENT.with(Cell::get) == self.addr() {
                    "cannot resume a coroutine from inside itself — it is already running"
                } else {
                    "cannot resume a coroutine that is waiting on another one it resumed"
                };
                return Err(
                    RuntimeError::fault(message, "resumed here", span).with_help(
                        "a coroutine runs one resume at a time; resume it again after it yields",
                    ),
                );
            }
            State::Dead => return Ok(vec![Value::Nil]),
        }
        let Some(shared) = self.shared.upgrade() else {
            return Err(RuntimeError::fault(
                "cannot resume a coroutine whose program has finished running",
                "resumed here",
                span,
            ));
        };
        // Each resume runs a dispatch loop under a Rust frame of its own, so
        // coroutines resuming coroutines nest on the native stack — the
        // resource this guard exists to bound.
        let _depth = saule_runtime::enter_call_depth(&span)?;

        let prior = std::mem::replace(&mut *self.state.borrow_mut(), State::Running);
        let mut vm = match prior {
            State::Fresh(body) => {
                let mut vm = Box::new(Vm::from_shared(shared));
                vm.coroutine = self.addr();
                let mut call_args = Vec::with_capacity(args.len() + 1);
                call_args.push(Value::Coroutine(Rc::clone(&self.yielder)));
                call_args.extend_from_slice(args);
                if let Err(e) = vm.enter_invocation(&body, &call_args, span) {
                    return Ok(self.fail(&mut vm, e));
                }
                vm
            }
            State::Suspended(mut vm) => {
                vm.shared = shared;
                vm.unpark_upvalues();
                // What `resume` was passed is what the pending `yield` call
                // returns inside the coroutine.
                let (dst, n_ret) = vm.yield_slot;
                vm.store_results(dst, args, n_ret);
                vm
            }
            State::Running | State::Dead => unreachable!("checked above"),
        };

        let outer = CURRENT.with(|c| c.replace(self.addr()));
        let outcome = vm.execute_from(1);
        CURRENT.with(|c| c.set(outer));

        match outcome {
            Ok(()) => {
                *self.state.borrow_mut() = State::Dead;
                Ok(std::mem::take(&mut vm.results))
            }
            Err(RuntimeError::Yield) => {
                // Copied out, not taken: the register file keeps the room
                // it collects a yield's values in, where giving it away
                // meant growing a new one from nothing at every yield.
                let out = vm.results.clone();
                vm.results.clear();
                vm.park_upvalues();
                vm.shared = NOBODY.with(Rc::clone);
                *self.state.borrow_mut() = State::Suspended(vm);
                Ok(out)
            }
            Err(e) => Ok(self.fail(&mut vm, e)),
        }
    }

    /// One step of `for v in gen do`: resume, and hand the loop what was
    /// yielded. `nil` — the coroutine finished, or had already — ends it.
    ///
    /// A failure **raises** here, where an explicit `resume` reports it.
    /// `resume` has a return value and `error()` beside it to say what went
    /// wrong; a loop has neither, and reporting the failure as the end of
    /// the sequence would make a generator that crashed half-way look like
    /// one that finished. Lua's `coroutine.wrap` raises for the same reason.
    fn next_for_loop(&self) -> Result<Vec<Value>, String> {
        if matches!(*self.state.borrow(), State::Dead) {
            return Ok(vec![Value::Nil]);
        }
        let out = self.resume(&[], 0..0).map_err(|e| e.to_string())?;
        if matches!(*self.state.borrow(), State::Dead)
            && let Some(message) = &*self.error.borrow()
        {
            return Err(format!("coroutine failed during `for … in`: {message}"));
        }
        Ok(if out.is_empty() {
            vec![Value::Nil]
        } else {
            out
        })
    }

    /// The body failed: record why, close what it leaves behind, and answer
    /// the `resume` with `nil`.
    fn fail(&self, vm: &mut Vm, e: RuntimeError) -> Vec<Value> {
        // A `throw` reports the value thrown, which is what a program that
        // threw a string wants back; anything else reports itself.
        let message = match &e {
            RuntimeError::Thrown { value, .. } => value.clone(),
            other => other.to_string(),
        };
        // The failure is handled — by being reported here — so the call
        // stack recorded on its way out must not be shown later as if it
        // had ended the program.
        super::trace::clear();
        // A closure the body built may outlive it; see `Vm::abandon`.
        vm.abandon();
        *self.error.borrow_mut() = Some(message);
        *self.state.borrow_mut() = State::Dead;
        vec![Value::Nil]
    }
}

impl VmCoroutine for Coroutine {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn class_name(&self) -> &'static str {
        "Coroutine"
    }

    fn type_name(&self) -> &'static str {
        "coroutine"
    }

    fn call_method(
        &self,
        handle: &Rc<CoroutineRef>,
        name: &str,
        args: &[Value],
        span: std::ops::Range<usize>,
    ) -> Result<Vec<Value>, RuntimeError> {
        let no_args = |method: &str| -> Result<(), RuntimeError> {
            if args.is_empty() {
                Ok(())
            } else {
                Err(RuntimeError::fault(
                    format!("`{method}()` takes no arguments, got {}", args.len()),
                    "called with arguments here",
                    span.clone(),
                ))
            }
        };
        match name {
            "resume" => self.resume(args, span),
            "status" => {
                no_args("status")?;
                Ok(vec![Value::Str(self.status().into())])
            }
            "isDone" => {
                no_args("isDone")?;
                Ok(vec![Value::Bool(matches!(
                    *self.state.borrow(),
                    State::Dead
                ))])
            }
            "error" => {
                no_args("error")?;
                Ok(vec![match &*self.error.borrow() {
                    Some(m) => Value::Str(m.as_str().into()),
                    None => Value::Nil,
                }])
            }
            "iter" => {
                no_args("iter")?;
                let co = Rc::clone(handle);
                Ok(vec![Value::NativeClosure(Rc::new(
                    saule_runtime::value::NativeClosure {
                        name: "Coroutine.iter",
                        func: Box::new(move |_| match co.as_any().downcast_ref::<Coroutine>() {
                            Some(c) => c.next_for_loop(),
                            None => Err("internal: iterating a coroutine that is not one".into()),
                        }),
                        param_names: Vec::new(),
                    },
                ))])
            }
            "yield" => Err(RuntimeError::fault(
                "`yield` is not a method of the coroutine",
                "this is the coroutine",
                span,
            )
            .with_help(
                "yield on the yielder the body is given: `Coroutine.create(fn(co) co.yield(v) end)`",
            )),
            other => Err(RuntimeError::fault(
                format!("`Coroutine` has no method `{other}`"),
                "this is a `Coroutine`",
                span,
            )
            .with_help("its methods are `resume`, `status`, `isDone`, `error` and `iter`")),
        }
    }
}

impl VmCoroutine for Yielder {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn class_name(&self) -> &'static str {
        "Yielder"
    }

    fn type_name(&self) -> &'static str {
        "yielder"
    }

    /// Every `yield` that gets here was *not* made by the coroutine's own
    /// code — that one is intercepted by the coroutine's dispatch loop and
    /// never reaches the runtime. So this only ever explains the refusal.
    fn call_method(
        &self,
        _handle: &Rc<CoroutineRef>,
        name: &str,
        _args: &[Value],
        span: std::ops::Range<usize>,
    ) -> Result<Vec<Value>, RuntimeError> {
        if name != "yield" {
            return Err(RuntimeError::fault(
                format!("`Yielder` has no method `{name}`"),
                "this is a `Yielder`",
                span,
            )
            .with_help("its only method is `yield`"));
        }
        let running = self.co.strong_count() > 0 && self.addr.get() == CURRENT.with(Cell::get);
        Err(match self.co.upgrade() {
            None => RuntimeError::fault(
                "cannot yield: the coroutine this yielder belongs to no longer exists",
                "yielded here",
                span,
            )
            .with_help("a yielder can only be used while its coroutine is alive"),
            Some(_) if running => RuntimeError::fault(
                "cannot yield from inside a callback",
                "yielded here, inside a function a built-in called back",
                span,
            )
            .with_help(
                "a sort comparator, an operator overload or a `toString` runs inside a built-in's call, which cannot be suspended; yield from the coroutine's own code",
            ),
            Some(_) => RuntimeError::fault(
                "cannot yield a coroutine that is not the one running",
                "yielded here",
                span,
            )
            .with_help("only the running coroutine's own yielder can yield"),
        })
    }
}

impl Yielder {
    /// Whether this is the yielder of the coroutine at `addr`, still
    /// there. (A coroutine that is gone may have left its address to
    /// another: its yielder is nobody's.)
    fn belongs_to(&self, addr: usize) -> bool {
        self.addr.get() == addr && self.co.strong_count() > 0
    }
}

impl std::fmt::Debug for Coroutine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<coroutine {}>", self.status())
    }
}

impl std::fmt::Debug for Yielder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<yielder>")
    }
}

impl Vm {
    /// Whether a `yield` called on `c` is one this register file suspends
    /// for: it is a coroutine's, and `c` is that coroutine's yielder.
    ///
    /// Not so for a register file that is no coroutine's, or another
    /// coroutine's, or a callback's running on behalf of one: the call
    /// then goes on to the runtime, which explains why it cannot yield
    /// from there.
    #[inline]
    pub(crate) fn yields_here(&self, c: &CoroutineRef) -> bool {
        self.coroutine != 0
            && c.as_any()
                .downcast_ref::<Yielder>()
                .is_some_and(|yielder| yielder.belongs_to(self.coroutine))
    }

    /// `CALLMX` with the name constant `k`, in a coroutine's register file:
    /// whether it is that coroutine's own `yield`, in which case this file
    /// is ready to suspend when this returns ([`begin_yield`]
    /// (Self::begin_yield)) and the caller leaves the loop.
    ///
    /// Asked before the call is made at all, and not found out on its way
    /// through the dynamic call and back out as an error: a yield is the
    /// commonest thing a coroutine does.
    #[inline(never)]
    pub(crate) fn yield_at(
        &mut self,
        k: u32,
        ins: crate::op::Instruction,
        recv: usize,
        chunk: &crate::chunk::Chunk,
        pc: usize,
    ) -> bool {
        let Value::Coroutine(c) = self.reg(recv) else {
            return false;
        };
        if !self.yields_here(c) {
            return false;
        }
        let Some(Value::Str(name)) = chunk.constants.get(k as usize) else {
            return false;
        };
        if name.as_str() != "yield" {
            return false;
        }
        // Operands as `call_member_by_name` reads them.
        let n_args = (ins.b() as usize).saturating_sub(1);
        let n_ret = if ins.c() == 0 {
            super::ALL_RESULTS
        } else {
            ins.c() - 1
        };
        self.begin_yield(recv, n_args, n_ret, pc);
        true
    }

    /// Make this register file ready to suspend at the `yield` whose
    /// receiver is in register `recv`: save where to continue, and collect
    /// what is yielded. The caller then leaves the loop with
    /// [`RuntimeError::Yield`].
    pub(crate) fn begin_yield(&mut self, recv: usize, n_args: usize, n_ret: u8, pc: usize) {
        // Continue after the `CALLMX` — the saved pc is what the loop reads
        // on re-entry, and the loop's own copy is about to be discarded.
        self.frames.last_mut().expect("a yield runs in a frame").pc = pc as u32;
        self.results.clear();
        for i in 0..n_args {
            let v = self.stack[recv + 1 + i].clone();
            self.results.push(v);
        }
        self.yield_slot = (recv, n_ret);
    }
}
