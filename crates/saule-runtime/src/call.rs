//! Calling values and methods at run time.
//!
//! The compiler resolves every call it can prove to a slot or a proto, and
//! those never come here. What does is everything decided only by the value
//! in hand: a native invoking its callable argument (`Table.sort`'s
//! comparator), an operator overload, a `toString`, and a method call on a
//! receiver the front end could not prove (`CALLMX`). They all dispatch on
//! the value, so they share one set of rules, written once.
//!
//! Arguments are always positional here. Named arguments and trailing
//! blocks are bound at the call site by the compiler, against the callee's
//! declared parameters, so by the time a call reaches the runtime it is an
//! ordinary list.

use std::rc::Rc;

use crate::error::RuntimeError;
use crate::value::foreign::NATIVE_CONSTRUCTOR;
use crate::value::{ClassObject, MethodRef, Value};

/// Maximum nesting depth of re-entrant calls.
///
/// A call the VM makes within one register file is a frame push and costs
/// no native stack, and `max_frames` bounds those. A call made from *outside*
/// it — a native invoking a bytecode comparator that sorts with itself, an
/// operator overload calling another — runs a fresh VM on the native stack,
/// one Rust frame per level, which `max_frames` cannot see. Unbounded, that
/// is a native stack overflow: not a Rust panic but `SIGSEGV`/`SIGABRT`,
/// which cannot be caught, prints no span, and gives `try … catch` no chance
/// to run. In the language server it would kill the whole session.
///
/// This limit bounds that nesting and turns it into a diagnostic. Override
/// with `SAULE_MAX_DEPTH` when a legitimately deep workload needs it *and*
/// the stack has been grown to match.
///
/// A count cannot be the whole answer, because what it is standing in for
/// is *bytes*: one level costs tens of kilobytes, and how many — a debug
/// build's frames are several times a release build's — is not something a
/// constant can know. [`set_stack_budget`] is the other half; an embedder
/// that reports its thread's stack gets a limit measured in the resource
/// actually being spent.
pub const MAX_CALL_DEPTH: u32 = 10_000;

thread_local! {
    static CALL_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    static DEPTH_LIMIT: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    /// The lowest stack address a nested call may start from, or 0 when no
    /// budget was set. One `Cell` rather than a base and a size, so the
    /// check on the call path is a load and a compare.
    static STACK_FLOOR: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Tell the runtime how much native stack this thread has for nested calls.
///
/// Called once per thread that runs Saule code, by whoever arranged the
/// stack and therefore knows its size — for `saule run`, the CLI, which
/// spawns a thread with a large one (and on macOS asks the linker for it,
/// since the program has to run on the first thread there).
///
/// Without it only [`MAX_CALL_DEPTH`] applies, which is the old behaviour:
/// right until a thread's stack is smaller than the nesting the count
/// allows, at which point the process dies with a `SIGSEGV` no `catch` can
/// see. With it, a nested call that would run past the budget reports
/// [`RuntimeError::StackExhausted`] instead.
///
/// `bytes` is the stack the *current* function still has below it. A
/// fraction is kept in reserve: unwinding the error and rendering the
/// diagnostic take stack too, and they run at the deepest point.
pub fn set_stack_budget(bytes: usize) {
    // Three quarters, which on the CLI's 512 MiB leaves 128 MiB — far more
    // than reporting needs, and the levels it costs are levels no honest
    // program uses.
    let usable = bytes / 4 * 3;
    let here = stack_probe();
    STACK_FLOOR.with(|c| c.set(here.saturating_sub(usable)));
}

/// The address of a local in the calling frame: where the stack has reached.
///
/// Stack grows downward on every target Saule builds for, so a *lower*
/// address means deeper.
#[inline(always)]
fn stack_probe() -> usize {
    let here = 0u8;
    std::ptr::addr_of!(here) as usize
}

/// Whether another nested call would run past the budget
/// [`set_stack_budget`] recorded. Always false when none was.
#[inline]
fn out_of_stack() -> bool {
    let floor = STACK_FLOOR.with(|c| c.get());
    floor != 0 && stack_probe() < floor
}

/// The active depth limit: `SAULE_MAX_DEPTH` if set and parseable, else
/// [`MAX_CALL_DEPTH`]. Read from the environment once per thread.
#[inline]
pub fn depth_limit() -> u32 {
    let cached = DEPTH_LIMIT.with(|c| c.get());
    if cached != 0 {
        return cached;
    }
    depth_limit_uncached()
}

/// The environment read, out of line: it happens once per thread and the
/// caller is on the call path of every re-entrant invocation.
#[cold]
#[inline(never)]
fn depth_limit_uncached() -> u32 {
    let limit = std::env::var("SAULE_MAX_DEPTH")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(MAX_CALL_DEPTH);
    DEPTH_LIMIT.with(|c| c.set(limit));
    limit
}

/// Enter one re-entrant call level, returning a guard that releases it on
/// drop.
///
/// `inline` for the same reason `Vm::int_at` is: `RuntimeError` is 64 bytes,
/// so out of line this returns a ~72-byte `Result` by value to carry an error
/// that, on the path that actually runs, is never built. `Table.sort` reaches
/// it once per comparison.
#[inline]
pub fn enter_call_depth(span: &std::ops::Range<usize>) -> Result<DepthGuard, RuntimeError> {
    DepthGuard::enter(span)
}

/// RAII depth counter. Decrements on every exit path — including the `?`
/// early-returns that are how errors leave a call — so the count can never
/// drift upward across a caught `throw`.
pub struct DepthGuard;

impl DepthGuard {
    /// Enter one call level, or fail if that would exceed the limit.
    ///
    /// On failure the depth is *not* incremented, so the error propagates
    /// out through frames that each decrement exactly once.
    #[inline]
    fn enter(span: &std::ops::Range<usize>) -> Result<Self, RuntimeError> {
        // Two limits, because there are two resources. The count is the
        // language's, fixed and the same everywhere; the budget is this
        // thread's actual stack, and on a small one it is reached first.
        if out_of_stack() {
            return Err(no_stack_left(span));
        }
        let limit = depth_limit();
        let entered = CALL_DEPTH.with(|d| {
            let next = d.get() + 1;
            if next > limit {
                return false;
            }
            d.set(next);
            true
        });
        if entered {
            Ok(DepthGuard)
        } else {
            Err(too_deep(limit, span))
        }
    }
}

/// The overflow error, out of line so building it is not inlined into every
/// caller of [`DepthGuard::enter`].
#[cold]
#[inline(never)]
fn too_deep(limit: u32, span: &std::ops::Range<usize>) -> RuntimeError {
    RuntimeError::StackOverflow {
        limit,
        span: span.clone(),
    }
}

#[cold]
#[inline(never)]
fn no_stack_left(span: &std::ops::Range<usize>) -> RuntimeError {
    RuntimeError::StackExhausted { span: span.clone() }
}

impl Drop for DepthGuard {
    fn drop(&mut self) {
        CALL_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

/// Reset the recursion counter. Called when starting a fresh top-level run so
/// a previous program's aborted stack can't leak into the next one — the
/// language server reuses a thread.
pub fn reset_depth() {
    CALL_DEPTH.with(|d| d.set(0));
}

/// Call `callee` with `args`, returning every value it returned.
pub fn call_value(
    callee: &Value,
    args: &[Value],
    span: std::ops::Range<usize>,
) -> Result<Vec<Value>, RuntimeError> {
    match callee {
        Value::Native(nf) => (nf.func)(args)
            .map(|v| vec![v])
            .map_err(|message| RuntimeError::TypeError { message, span }),
        Value::NativeClosure(nc) => {
            (nc.func)(args).map_err(|message| RuntimeError::TypeError { message, span })
        }
        // A bytecode function: the VM runs it on a fresh register file over
        // its existing shared state.
        Value::VmFunction(f) => f.invoke(args, span),
        // `Image(…)` on a native class: its constructor. A Saule class is
        // constructed by `NEW`, which the compiler emits for a class it can
        // prove; a native class has no layout to prove, so its construction
        // is an ordinary call of the class value, and lands here.
        Value::Class(class) => match class.lookup_static_field(NATIVE_CONSTRUCTOR) {
            Some(ctor) => call_value(&ctor, args, span),
            None => Err(RuntimeError::not_callable(callee.type_name(), span)),
        },
        other => Err(RuntimeError::not_callable(other.type_name(), span)),
    }
}

/// [`call_value`] for a caller that wants a single value: the first one
/// returned, or `nil`.
pub fn call_value_first(
    callee: &Value,
    args: &[Value],
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    match callee {
        Value::VmFunction(f) => f.invoke_first(args, span),
        _ => Ok(call_value(callee, args, span)?.into_iter().next().unwrap_or(Value::Nil)),
    }
}

/// Invoke an instance method with `receiver` as `self`. A compiled method
/// takes `self` as parameter 0, so binding it is prepending it.
pub(crate) fn call_method_ref(
    m: &MethodRef,
    receiver: Value,
    args: &[Value],
    span: std::ops::Range<usize>,
) -> Result<Vec<Value>, RuntimeError> {
    let mut all = Vec::with_capacity(args.len() + 1);
    all.push(receiver);
    all.extend_from_slice(args);
    m.0.invoke(&all, span)
}

/// Invoke a static method, which takes no receiver at all — `CALLSTAT`
/// starts its frame at the arguments.
pub fn call_static_method_ref(
    m: &MethodRef,
    args: &[Value],
    span: std::ops::Range<usize>,
) -> Result<Vec<Value>, RuntimeError> {
    m.0.invoke(args, span)
}

/// Call `receiver.name(args)`, deciding what `name` is from the receiver.
///
/// Covers every receiver kind in one place — user instances, classes,
/// enum variants, file handles, stdlib values — which is exactly why it is
/// worth having once: the alternative is every caller learning each of
/// them separately and diverging on the ones it gets wrong.
pub fn call_method(
    receiver: &Value,
    name: &str,
    args: &[Value],
    span: std::ops::Range<usize>,
) -> Result<Vec<Value>, RuntimeError> {
    match receiver {
        Value::Instance(inst) => {
            let class = inst.borrow().class.clone();
            if let Some(m) = class.lookup_method(name) {
                return call_method_ref(&m, receiver.clone(), args, span);
            }
            if let Some(m) = class.lookup_static_method(name) {
                return call_static_method_ref(&m, args, span);
            }
            if let Some(v) = inst.borrow().field(name).cloned() {
                return call_value(&v, args, span);
            }
            Err(RuntimeError::TypeError {
                message: format!(
                    "no method or field `{name}` on instance of class `{}` — instance members are case-sensitive",
                    class.name
                ),
                span,
            })
        }
        Value::Class(class) => call_static(class, name, args, span),
        Value::EnumVariant(variant) => {
            // A method the enum declares is called *with* the variant as its
            // receiver; `.value`, `.name`, and anything a payload happens to
            // hold are not. Which of the two this is has to be decided by
            // looking in the method map — deciding it from the shape of what
            // `read_member` returned would call a variant whose payload is a
            // function with a receiver it never declared.
            //
            // `read_member` answers `value` and `name` before it consults
            // the method map, so a method spelled either way is already
            // unreachable as a method. Mirrored here rather than reordered:
            // the two paths have to agree about which name wins.
            let method = (!matches!(name, "value" | "name"))
                .then(|| {
                    variant
                        .enum_obj
                        .borrow()
                        .as_ref()
                        .and_then(|e| e.methods.get(name).cloned())
                })
                .flatten();
            match method {
                Some(m) => call_method_ref(&m, receiver.clone(), args, span),
                None => {
                    let v = crate::members::read_member(receiver, name, span.clone())?;
                    call_value(&v, args, span)
                }
            }
        }
        Value::File(handle) => crate::stdlib::io::dispatch_file_method(handle, name, args)
            .map_err(|message| RuntimeError::TypeError { message, span }),
        // An object of a native class: its method takes the object as
        // argument 0, the same shape a compiled method takes `self` in.
        Value::Foreign(obj) => {
            if let Some(m) = obj.class.methods.get(name) {
                let mut all = Vec::with_capacity(args.len() + 1);
                all.push(receiver.clone());
                all.extend_from_slice(args);
                return call_value(m, &all, span);
            }
            // A property whose value happens to be callable.
            let v = crate::members::read_member(receiver, name, span.clone())?;
            call_value(&v, args, span)
        }
        _ => {
            let v = crate::members::read_member(receiver, name, span.clone())?;
            call_value(&v, args, span)
        }
    }
}

/// `Class.name(args)`: a static method, or a static field holding something
/// callable.
pub fn call_static(
    class: &Rc<ClassObject>,
    name: &str,
    args: &[Value],
    span: std::ops::Range<usize>,
) -> Result<Vec<Value>, RuntimeError> {
    if let Some(m) = class.lookup_static_method(name) {
        return call_static_method_ref(&m, args, span);
    }
    if let Some(v) = class.lookup_static_field(name) {
        return call_value(&v, args, span);
    }
    Err(RuntimeError::TypeError {
        message: format!(
            "no static member `{name}` on class `{}` — check the class definition for the correct name",
            class.name
        ),
        span,
    })
}
