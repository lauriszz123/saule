//! Timer module — a real monotonic clock plus per-frame delta tracking.
//!
//! The exported functions are plain, safe Rust functions annotated with
//! `#[saule_export]`; the SDK generates the C-ABI shim and the manifest entry.

use std::sync::Mutex;
use std::time::Instant;

use saule_sdk::saule_export;

struct Clock {
    /// When the clock was last reset (window creation). `Timer.getTime` is
    /// measured from here.
    start: Instant,
    /// Instant of the previous `Timer.getDelta` call, used to compute the
    /// frame delta.
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

/// `Timer.getTime()` — seconds since the clock was last reset.
#[saule_export(class = "Timer", name = "getTime")]
fn timer_get_time() -> f64 {
    with_clock(|c| c.start.elapsed().as_secs_f64())
}

/// `Timer.getDelta()` — seconds since the previous frame, and marks "now" as
/// the start of the current frame. Call once per loop iteration.
#[saule_export(class = "Timer", name = "getDelta")]
fn timer_get_delta() -> f64 {
    with_clock(|c| {
        let now = Instant::now();
        let dt = now.duration_since(c.last_frame).as_secs_f64();
        c.last_frame = now;
        dt
    })
}
