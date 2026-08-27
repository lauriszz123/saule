//! Timer module — a real monotonic clock, per-frame delta tracking, and the
//! presented-frame rate.
//!
//! The exported functions are plain, safe Rust functions annotated with
//! `#[saule_export]`; the SDK generates the C-ABI shim and the manifest entry.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use saule_sdk::saule_export;

/// How many recent frames the reported rate averages over. Long enough that a
/// single slow frame does not make the number jump, short enough that it still
/// tracks a real change within a fraction of a second.
const FPS_WINDOW: usize = 30;

struct Clock {
    /// When the clock was last reset (window creation). `Timer.getTime` is
    /// measured from here.
    start: Instant,
    /// Instant of the previous `Timer.getDelta` call, used to compute the
    /// frame delta.
    last_frame: Instant,
    /// Instant of the previous *presented* frame, which is what the rate is
    /// measured between.
    last_present: Option<Instant>,
    /// Recent inter-present intervals, oldest first.
    intervals: Vec<Duration>,
}

static CLOCK: Mutex<Option<Clock>> = Mutex::new(None);

/// Reset the clock to "now". Called from `Window.create` so `getTime` starts
/// at zero for each window session.
pub(crate) fn reset_clock() {
    let now = Instant::now();
    *CLOCK.lock().unwrap() = Some(Clock {
        start: now,
        last_frame: now,
        last_present: None,
        intervals: Vec::with_capacity(FPS_WINDOW),
    });
}

fn with_clock<R>(f: impl FnOnce(&mut Clock) -> R) -> R {
    let mut guard = CLOCK.lock().unwrap();
    let clock = guard.get_or_insert_with(|| {
        let now = Instant::now();
        Clock {
            start: now,
            last_frame: now,
            last_present: None,
            intervals: Vec::with_capacity(FPS_WINDOW),
        }
    });
    f(clock)
}

/// Record that a frame reached the window. Called from `Graphics.present`.
///
/// The rate is measured here rather than from `getDelta` because `getDelta` is
/// the *app's* clock — a program that never calls it, or calls it twice, would
/// otherwise report a rate that has nothing to do with what the display saw.
pub(crate) fn mark_frame() {
    with_clock(|c| {
        let now = Instant::now();
        if let Some(previous) = c.last_present {
            if c.intervals.len() == FPS_WINDOW {
                c.intervals.remove(0);
            }
            c.intervals.push(now.duration_since(previous));
        }
        c.last_present = Some(now);
    });
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

/// `Timer.getFPS()` — frames actually presented per second, averaged over the
/// last 30 frames. `0.0` until two frames have been presented.
#[saule_export(class = "Timer", name = "getFPS")]
fn timer_get_fps() -> f64 {
    with_clock(|c| {
        if c.intervals.is_empty() {
            return 0.0;
        }
        let total: Duration = c.intervals.iter().sum();
        let mean = total.as_secs_f64() / c.intervals.len() as f64;
        if mean <= 0.0 { 0.0 } else { 1.0 / mean }
    })
}

/// `Timer.sleep(seconds)` — block for `seconds`.
///
/// For the deliberately idle loop: a tool that only redraws on input can wait
/// here instead of spinning through frames it has nothing to put in.
#[saule_export(class = "Timer", name = "sleep")]
fn timer_sleep(seconds: f64) -> Result<(), String> {
    if !seconds.is_finite() || seconds < 0.0 {
        return Err("Timer.sleep: the duration must be a non-negative number".into());
    }
    // A runaway sleep in a game loop looks exactly like a hang, so cap it at
    // something a person would still interrupt rather than force-quit.
    if seconds > 10.0 {
        return Err("Timer.sleep: the duration may not exceed 10 seconds".into());
    }
    std::thread::sleep(Duration::from_secs_f64(seconds));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The clock is process-global, so these tests cannot run concurrently
    /// with each other — one calling `reset_clock` mid-way through another
    /// makes the rate it reads meaningless. Cargo runs tests in parallel by
    /// default, so they take this lock rather than relying on ordering.
    static SERIAL: Mutex<()> = Mutex::new(());

    #[test]
    fn the_rate_is_zero_before_two_frames_are_presented() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        reset_clock();
        assert_eq!(timer_get_fps(), 0.0);
        mark_frame();
        // One frame gives no interval to measure.
        assert_eq!(timer_get_fps(), 0.0);
    }

    #[test]
    fn the_rate_tracks_the_interval_between_presents() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        reset_clock();
        for _ in 0..3 {
            mark_frame();
            std::thread::sleep(Duration::from_millis(10));
        }
        let fps = timer_get_fps();
        // ~100 FPS, with generous slack for a loaded test machine.
        assert!(fps > 20.0 && fps < 200.0, "implausible rate: {fps}");
    }

    #[test]
    fn the_window_never_grows_past_its_bound() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        reset_clock();
        for _ in 0..(FPS_WINDOW + 20) {
            mark_frame();
        }
        with_clock(|c| assert_eq!(c.intervals.len(), FPS_WINDOW));
    }

    #[test]
    fn sleep_rejects_a_duration_that_would_look_like_a_hang() {
        assert!(timer_sleep(-1.0).is_err());
        assert!(timer_sleep(f64::NAN).is_err());
        assert!(timer_sleep(60.0).is_err());
        assert!(timer_sleep(0.0).is_ok());
    }
}
