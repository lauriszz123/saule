//! Timer module — a real monotonic clock plus per-frame delta tracking.

use std::sync::Mutex;
use std::time::Instant;

use saule_native_abi::{return_error, CValue, NativeSymbolFn};

use crate::args::Args;

struct Clock {
    /// When the clock was last reset (window creation). `Timer.getTime` is
    /// measured from here.
    start: Instant,
    /// Instant of the previous `Timer.getDelta` / `Timer.step` call, used to
    /// compute the frame delta.
    last_frame: Instant,
}

static CLOCK: Mutex<Option<Clock>> = Mutex::new(None);

/// Reset the clock to "now". Called from `Window.create` so `getTime` starts
/// at zero for each window session.
pub(crate) fn reset_clock() {
    let now = Instant::now();
    *CLOCK.lock().unwrap() = Some(Clock {
        start: now,
        last_frame: now,
    });
}

fn with_clock<R>(f: impl FnOnce(&mut Clock) -> R) -> R {
    let mut guard = CLOCK.lock().unwrap();
    let clock = guard.get_or_insert_with(|| {
        let now = Instant::now();
        Clock {
            start: now,
            last_frame: now,
        }
    });
    f(clock)
}

/// Write the dispatch result into `out`.
fn dispatch(out: &mut CValue, body: impl FnOnce() -> Result<CValue, String>) -> i32 {
    let (value, code) = match body() {
        Ok(v) => (v, 0),
        Err(msg) => (return_error(&msg), 1),
    };
    *out = value;
    code
}

/// `Timer.getTime() -> float` — seconds since the clock was last reset.
///
/// # Safety
/// ABI entry point — see [`saule_native_abi`] for the pointer contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn saule_engine_timer_get_time(
    args: *const CValue,
    argc: usize,
    out: *mut CValue,
) -> i32 {
    let a = unsafe { Args::new(args, argc) };
    let out = unsafe { &mut *out };
    dispatch(out, || {
        a.expect_arity("Timer.getTime", 0)?;
        let secs = with_clock(|c| c.start.elapsed().as_secs_f64());
        Ok(CValue::float(secs))
    })
}

/// `Timer.getDelta() -> float` — seconds since the previous frame, and marks
/// "now" as the start of the current frame. Call once per loop iteration.
///
/// # Safety
/// ABI entry point — see [`saule_native_abi`] for the pointer contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn saule_engine_timer_get_delta(
    args: *const CValue,
    argc: usize,
    out: *mut CValue,
) -> i32 {
    let a = unsafe { Args::new(args, argc) };
    let out = unsafe { &mut *out };
    dispatch(out, || {
        a.expect_arity("Timer.getDelta", 0)?;
        let dt = with_clock(|c| {
            let now = Instant::now();
            let dt = now.duration_since(c.last_frame).as_secs_f64();
            c.last_frame = now;
            dt
        });
        Ok(CValue::float(dt))
    })
}

const _: [NativeSymbolFn; 2] = [saule_engine_timer_get_time, saule_engine_timer_get_delta];
