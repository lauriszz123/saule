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
//! The `Vm` is rebuilt around the register file on every resume.
//!
//! The body is handed a separate [`Yielder`], not the coroutine itself, so
//! that a suspended body does not hold its own coroutine alive from its
//! first register; the yielder refers back weakly.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use saule_runtime::value::{CoroutineRef, VmCoroutine, VmFunctionRef};
use saule_runtime::{RuntimeError, Value};

use super::{Frame, Upvalue, Vm, VmShared};

thread_local! {
    /// The coroutine whose code is running now, by address, or 0 outside
    /// every coroutine. What tells "running" from "normal" — a coroutine
    /// that is mid-`resume` of another — and a `yield` made inside a
    /// callback from one made from somewhere else entirely.
    static CURRENT: Cell<usize> = const { Cell::new(0) };
}

/// A register file put aside: everything a [`Vm`] holds except the shared
/// program state, which a suspended coroutine must not keep alive.
struct Suspended {
    stack: Vec<Value>,
    frames: Vec<Frame>,
    open_upvals: Vec<Rc<RefCell<Upvalue>>>,
    parked: Vec<u32>,
    high_water: usize,
    yield_slot: (usize, u8),
    results: Vec<Value>,
}

enum State {
    /// Created and never resumed. The first `resume` calls the body.
    Fresh(Rc<VmFunctionRef>),
    /// Stopped at a `yield`.
    Suspended(Box<Suspended>),
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
}

impl Coroutine {
    /// A coroutine that will run `body` when first resumed.
    pub(crate) fn create(body: Rc<VmFunctionRef>, shared: Weak<VmShared>) -> Rc<CoroutineRef> {
        Rc::new_cyclic(|me: &Weak<CoroutineRef>| {
            CoroutineRef::wrap(Coroutine {
                state: RefCell::new(State::Fresh(body)),
                yielder: CoroutineRef::new(Yielder { co: me.clone() }),
                shared,
                error: RefCell::new(None),
            })
        })
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
                return Err(RuntimeError::TypeError {
                    message: message.into(),
                    span,
                });
            }
            State::Dead => return Ok(vec![Value::Nil]),
        }
        let Some(shared) = self.shared.upgrade() else {
            return Err(RuntimeError::TypeError {
                message: "cannot resume a coroutine: the engine that created it is no longer \
                          running"
                    .into(),
                span,
            });
        };
        // Each resume runs a dispatch loop under a Rust frame of its own, so
        // coroutines resuming coroutines nest on the native stack — the
        // resource this guard exists to bound.
        let _depth = saule_runtime::enter_call_depth(&span)?;

        let prior = std::mem::replace(&mut *self.state.borrow_mut(), State::Running);
        let mut vm = match prior {
            State::Fresh(body) => {
                let mut vm = Vm::from_shared(shared);
                vm.coroutine = self.addr();
                let mut call_args = Vec::with_capacity(args.len() + 1);
                call_args.push(Value::Coroutine(Rc::clone(&self.yielder)));
                call_args.extend_from_slice(args);
                if let Err(e) = vm.enter_invocation(&body, &call_args, span) {
                    return Ok(self.fail(vm, e));
                }
                vm
            }
            State::Suspended(s) => {
                let mut vm = Vm::from_suspended(shared, *s, self.addr());
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
                let out = std::mem::take(&mut vm.results);
                vm.park_upvalues();
                *self.state.borrow_mut() = State::Suspended(Box::new(vm.into_suspended()));
                Ok(out)
            }
            Err(e) => Ok(self.fail(vm, e)),
        }
    }

    /// The body failed: record why, close what it leaves behind, and answer
    /// the `resume` with `nil`.
    fn fail(&self, mut vm: Vm, e: RuntimeError) -> Vec<Value> {
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
        _handle: &Rc<CoroutineRef>,
        name: &str,
        args: &[Value],
        span: std::ops::Range<usize>,
    ) -> Result<Vec<Value>, RuntimeError> {
        let no_args = |method: &str| -> Result<(), RuntimeError> {
            if args.is_empty() {
                Ok(())
            } else {
                Err(RuntimeError::TypeError {
                    message: format!("Coroutine.{method} takes no arguments, got {}", args.len()),
                    span: span.clone(),
                })
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
            "yield" => Err(RuntimeError::TypeError {
                message: "`yield` is called on the yielder a coroutine's body is given, not on \
                          the coroutine"
                    .into(),
                span,
            }),
            other => Err(RuntimeError::TypeError {
                message: format!(
                    "no method `{other}` on Coroutine — valid: resume / status / isDone / error"
                ),
                span,
            }),
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
            return Err(RuntimeError::TypeError {
                message: format!("no method `{name}` on Yielder — valid: yield"),
                span,
            });
        }
        let running = self
            .coroutine_addr()
            .is_some_and(|addr| addr == CURRENT.with(Cell::get));
        let message = match self.co.upgrade() {
            None => "cannot yield: the coroutine this yielder belongs to no longer exists",
            Some(_) if running => {
                "cannot yield from inside a callback — a sort comparator, an operator overload, \
                 a `toString` or any other function a built-in calls back — because the \
                 built-in's own call cannot be suspended; yield from the coroutine's own code"
            }
            Some(_) => "cannot yield a coroutine that is not the one running",
        };
        Err(RuntimeError::TypeError {
            message: message.into(),
            span,
        })
    }
}

impl Yielder {
    fn coroutine_addr(&self) -> Option<usize> {
        let co = self.co.upgrade()?;
        let addr = co.as_any().downcast_ref::<Coroutine>()?.addr();
        Some(addr)
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
    /// `yield` reached the dispatch loop as a method call on `c`. Decide
    /// whether this register file is the one it suspends, and if so make it
    /// ready to: save where to continue, and collect what is yielded.
    ///
    /// `true` means the caller must leave the loop with
    /// [`RuntimeError::Yield`]. `false` means this is not that register file
    /// — not a coroutine at all, or a different coroutine's, or a callback
    /// running on behalf of one — and the call goes on to the runtime,
    /// which explains why it cannot yield from here.
    pub(crate) fn try_yield(
        &mut self,
        c: &Rc<CoroutineRef>,
        recv: usize,
        n_args: usize,
        n_ret: u8,
        pc: usize,
    ) -> bool {
        if self.coroutine == 0 {
            return false;
        }
        let Some(yielder) = c.as_any().downcast_ref::<Yielder>() else {
            return false;
        };
        if yielder.coroutine_addr() != Some(self.coroutine) {
            return false;
        }
        // Continue after the `CALLMX` — the saved pc is what the loop reads
        // on re-entry, and the loop's own copy is about to be discarded.
        self.frames.last_mut().expect("a yield runs in a frame").pc = pc as u32;
        self.results.clear();
        for i in 0..n_args {
            let v = self.stack[recv + 1 + i].clone();
            self.results.push(v);
        }
        self.yield_slot = (recv, n_ret);
        true
    }

    /// Put this register file aside, letting go of the shared state.
    fn into_suspended(self) -> Suspended {
        Suspended {
            stack: self.stack,
            frames: self.frames,
            open_upvals: self.open_upvals,
            parked: self.parked,
            high_water: self.high_water,
            yield_slot: self.yield_slot,
            results: self.results,
        }
    }

    /// Rebuild a `Vm` around a register file [`into_suspended`] put aside.
    fn from_suspended(shared: Rc<VmShared>, s: Suspended, coroutine: usize) -> Vm {
        Vm {
            shared,
            stack: s.stack,
            frames: s.frames,
            open_upvals: s.open_upvals,
            high_water: s.high_water,
            parked: s.parked,
            coroutine,
            yield_slot: s.yield_slot,
            results: s.results,
        }
    }
}
