//! Host facilities the interpreter cannot provide for itself: wall-clock
//! time, a monotonic clock, sleeping, the process id, and termination.
//!
//! These used to be called directly — `SystemTime::now()`, `Instant::now()`,
//! `thread::sleep`, `std::process::exit`. That is fine natively and fatal in a
//! browser: on `wasm32-unknown-unknown` the time APIs **panic**, which traps
//! the whole module, and `process::exit` tears it down. Both are reachable
//! from ordinary Saule code (`Os.time()`, `Os.clock()`, `Os.exit()`), so a
//! one-line program could kill the playground.
//!
//! The interpreter therefore asks a [`Platform`] instead. Natively one is
//! always installed and nothing changes. On wasm the default reports every
//! facility as unavailable — so `Os.time()` returns a clear error instead of
//! trapping — and the embedder installs a real one backed by `Date.now()` and
//! `performance.now()`.
//!
//! Deliberately no `wasm-bindgen` dependency here: the interpreter stays free
//! of any particular JS binding, exactly as [`crate::output`] stays free of
//! any particular console.

use std::cell::RefCell;

/// Host facilities, all fallible because a sandbox may provide none of them.
///
/// Every method has a default returning "unavailable", so an embedder only
/// implements what its host can actually do.
pub trait Platform {
    /// Seconds since the Unix epoch, or `None` when the host has no clock.
    fn unix_time_secs(&self) -> Option<f64> {
        None
    }

    /// Seconds since the program started. Monotonic: it never goes backwards,
    /// which is what makes it usable for timing rather than dating.
    fn monotonic_secs(&self) -> Option<f64> {
        None
    }

    /// Block for `secs`. `false` means the host cannot block at all — a
    /// browser's main thread, for instance.
    fn sleep(&self, secs: f64) -> bool {
        let _ = secs;
        false
    }

    /// The process id, when the host has such a concept.
    fn pid(&self) -> Option<u32> {
        None
    }

    /// Terminate the program with `code`.
    ///
    /// Implementations that really can terminate never return. **Returning is
    /// the signal that the host could not**, in which case the caller records
    /// the code (see [`take_exit`]) and unwinds instead, so a wasm module
    /// stops the program without tearing itself down.
    fn exit(&self, code: i32) {
        let _ = code;
    }
}

// ─── the installed platform ────────────────────────────────────────────────

thread_local! {
    /// Overrides the compiled-in default when set. Thread-local to match the
    /// interpreter, which is `Rc`-based and never leaves its thread.
    static PLATFORM: RefCell<Option<Box<dyn Platform>>> = const { RefCell::new(None) };

    /// Exit code parked by [`Platform::exit`] when the host could not
    /// actually terminate. Mirrors how `RuntimeError::Thrown` parks its
    /// payload — the error itself has to stay `Send + Sync` for miette, so
    /// the interesting value travels beside it.
    static EXIT_CODE: RefCell<Option<i32>> = const { RefCell::new(None) };
}

/// Install `platform` for this thread, returning the previous override.
///
/// Prefer [`with_platform`], which restores the previous value even if the
/// program panics.
pub fn set_platform(platform: Option<Box<dyn Platform>>) -> Option<Box<dyn Platform>> {
    PLATFORM.with(|p| std::mem::replace(&mut *p.borrow_mut(), platform))
}

/// Run `f` with `platform` installed, then restore the previous one.
pub fn with_platform<F, R>(platform: Box<dyn Platform>, f: F) -> R
where
    F: FnOnce() -> R,
{
    struct Restore(Option<Box<dyn Platform>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            set_platform(self.0.take());
        }
    }

    let _restore = Restore(set_platform(Some(platform)));
    f()
}

/// Ask the current platform for something.
///
/// Uses the installed override if there is one and the compiled-in default
/// otherwise. The borrow is released before `f` runs so a platform
/// implementation is free to re-enter.
fn query<R>(f: impl FnOnce(&dyn Platform) -> R) -> R {
    let installed = PLATFORM.with(|p| p.borrow().is_some());
    if installed {
        return PLATFORM.with(|p| {
            let borrow = p.borrow();
            let platform = borrow.as_ref().expect("platform vanished");
            f(&**platform)
        });
    }
    f(&DEFAULT)
}

/// Seconds since the Unix epoch.
pub fn unix_time_secs() -> Option<f64> {
    query(|p| p.unix_time_secs())
}

/// Seconds since the program started.
pub fn monotonic_secs() -> Option<f64> {
    query(|p| p.monotonic_secs())
}

/// Block for `secs`; `false` if the host cannot.
pub fn sleep(secs: f64) -> bool {
    query(|p| p.sleep(secs))
}

/// The process id, if the host has one.
pub fn pid() -> Option<u32> {
    query(|p| p.pid())
}

/// Ask the platform to terminate.
///
/// Returns only when it could not, having recorded `code` for [`take_exit`].
pub fn exit(code: i32) {
    query(|p| p.exit(code));
    // Still here, so the host did not terminate. Record the intent so the
    // embedder can tell a deliberate `Os.exit(0)` from a crash.
    EXIT_CODE.with(|c| *c.borrow_mut() = Some(code));
}

/// Take the exit code recorded by a [`Platform::exit`] that could not
/// terminate, clearing it.
///
/// An embedder calls this after a run fails: `Some(code)` means the program
/// called `Os.exit` and the resulting error is a normal termination rather
/// than a fault.
pub fn take_exit() -> Option<i32> {
    EXIT_CODE.with(|c| c.borrow_mut().take())
}

/// Message used when a facility is missing, phrased so a playground user
/// understands it is the sandbox talking and not their program.
pub fn unavailable(what: &str) -> String {
    format!("{what} is unavailable in this build: the host provides no such facility")
}

// ─── compiled-in defaults ──────────────────────────────────────────────────

#[cfg(not(target_arch = "wasm32"))]
static DEFAULT: native::NativePlatform = native::NativePlatform;

#[cfg(target_arch = "wasm32")]
static DEFAULT: Unsupported = Unsupported;

/// The default on targets with no host facilities at all — `wasm32` with
/// nothing installed. Every method falls through to the trait's "unavailable"
/// defaults, so `Os.time()` reports a clear error rather than trapping.
#[cfg(target_arch = "wasm32")]
struct Unsupported;

#[cfg(target_arch = "wasm32")]
impl Platform for Unsupported {}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::Platform;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    thread_local! {
        /// Fixed at first use, which is what makes `Os.clock()` "seconds
        /// since the program started".
        static START: Instant = Instant::now();
    }

    pub struct NativePlatform;

    impl Platform for NativePlatform {
        fn unix_time_secs(&self) -> Option<f64> {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .map(|d| d.as_secs_f64())
        }

        fn monotonic_secs(&self) -> Option<f64> {
            Some(START.with(|s| s.elapsed().as_secs_f64()))
        }

        fn sleep(&self, secs: f64) -> bool {
            if secs > 0.0 && secs.is_finite() {
                std::thread::sleep(std::time::Duration::from_secs_f64(secs));
            }
            true
        }

        fn pid(&self) -> Option<u32> {
            Some(std::process::id())
        }

        fn exit(&self, code: i32) {
            // Diverges, so control never reaches the caller's "the host could
            // not terminate" path. `!` coerces to the trait's `()`.
            std::process::exit(code)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A platform that reports everything as unavailable, standing in for a
    /// bare wasm host so these tests run on any target.
    struct Sandbox;
    impl Platform for Sandbox {}

    struct FixedClock;
    impl Platform for FixedClock {
        fn unix_time_secs(&self) -> Option<f64> {
            Some(1_700_000_000.5)
        }
        fn monotonic_secs(&self) -> Option<f64> {
            Some(1.25)
        }
    }

    #[test]
    fn native_default_provides_a_clock() {
        // No override installed, so this exercises the compiled-in default.
        assert!(unix_time_secs().is_some());
        assert!(monotonic_secs().is_some());
        assert!(pid().is_some());
    }

    #[test]
    fn an_installed_platform_takes_over() {
        with_platform(Box::new(FixedClock), || {
            assert_eq!(unix_time_secs(), Some(1_700_000_000.5));
            assert_eq!(monotonic_secs(), Some(1.25));
            // Not overridden, so the trait default applies.
            assert_eq!(pid(), None);
        });
        // …and the default is back afterwards.
        assert!(pid().is_some());
    }

    #[test]
    fn a_sandbox_reports_everything_unavailable() {
        with_platform(Box::new(Sandbox), || {
            assert_eq!(unix_time_secs(), None);
            assert_eq!(monotonic_secs(), None);
            assert_eq!(pid(), None);
            assert!(!sleep(0.01));
        });
    }

    #[test]
    fn exit_that_cannot_terminate_records_its_code() {
        let _ = take_exit(); // clear anything a previous test left
        with_platform(Box::new(Sandbox), || {
            exit(3);
        });
        assert_eq!(take_exit(), Some(3));
        // Taking it clears it, so a later run cannot see a stale code.
        assert_eq!(take_exit(), None);
    }

    #[test]
    fn platform_is_restored_when_the_program_panics() {
        let panicked = std::panic::catch_unwind(|| {
            with_platform(Box::new(Sandbox), || panic!("boom"));
        })
        .is_err();
        assert!(panicked);
        assert!(pid().is_some(), "the sandbox should not still be installed");
    }
}
