//! A sampling profiler for Saule code: which functions a program spends its
//! time in.
//!
//! `saule run --profile`, or `SAULE_PROFILE=1` (or `=FILE`) for any Saule
//! program, built executables included. At the end, a table:
//!
//! ```text
//! profile: 4210 ms sampled every 1 ms
//!   self   total  function
//!  31.2%   31.2%  Graphics.present (native)
//!  12.0%   40.5%  Element.dispatch  src/core/Element.sau:212
//!   ...
//! ```
//!
//! - **self**: time with this function on top: running its own code, or a
//!   native it called (natives are listed on their own when they were the
//!   ones running);
//! - **total**: time with it anywhere on the stack: itself and everything it
//!   called.
//!
//! ## How
//!
//! A thread ticks every millisecond. The VM looks at the clock only where it
//! already does work, at every call and return (and around natives): if a
//! tick has passed, it records its stack once, weighted by the number of
//! ticks since the last record. A loop that makes no calls is charged to the
//! function it runs in when that function next calls or returns, which is
//! the right function. A native that blocks — `Graphics.present` waiting for
//! the next frame — collects every tick that passed inside it, because the
//! sampler notes which native was running when it ticked.
//!
//! ## Cost
//!
//! Off: one relaxed atomic load at each call and return, and one around
//! each native call. On: a stack walk per millisecond.
//!
//! A callback a native runs (a `Table.sort` comparator) and a coroutine run
//! on VMs of their own, so a sample taken in one shows that VM's frames
//! only.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::time::{Duration, Instant};

use crate::chunk::{Chunk, Proto};

/// How often the sampler ticks.
const INTERVAL: Duration = Duration::from_millis(1);

/// While a file is the target, it is rewritten this often, so a program that
/// never ends cleanly — killed, or `Os.exit` — still leaves a report.
const WRITE_EVERY: Duration = Duration::from_secs(2);

/// Rows in the report.
const ROWS: usize = 40;

static ENABLED: AtomicBool = AtomicBool::new(false);
/// A tick has passed since the stack was last recorded.
static PENDING: AtomicBool = AtomicBool::new(false);
/// Ticks since the stack was last recorded.
static TICKS: AtomicU64 = AtomicU64::new(0);
/// The native running now, as `index + 1` into this thread's interned
/// names; 0 for none.
static NATIVE_NOW: AtomicUsize = AtomicUsize::new(0);
/// What `NATIVE_NOW` was at the last tick.
static NATIVE_AT_TICK: AtomicUsize = AtomicUsize::new(0);

/// A function, as the report counts it.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    /// A Saule function, by its proto's address: protos live as long as the
    /// program.
    Saule(usize),
    /// A native, by its interned index.
    Native(usize),
}

#[derive(Default)]
struct Profile {
    started: Option<Instant>,
    /// `None`: stderr at the end. `Some`: this file, rewritten as it goes.
    target: Option<PathBuf>,
    last_write: Option<Instant>,
    ticks: u64,
    self_ticks: HashMap<Key, u64>,
    total_ticks: HashMap<Key, u64>,
    names: HashMap<Key, String>,
    natives: Vec<&'static str>,
    native_index: HashMap<&'static str, usize>,
}

thread_local! {
    static PROFILE: RefCell<Profile> = RefCell::new(Profile::default());
}

/// Start profiling this thread's Saule code. `target` is where the report
/// goes: stderr when `None`.
pub fn start(target: Option<PathBuf>) {
    PROFILE.with(|p| {
        let mut p = p.borrow_mut();
        *p = Profile::default();
        p.started = Some(Instant::now());
        p.target = target;
    });
    if ENABLED.swap(true, Relaxed) {
        return;
    }
    std::thread::Builder::new()
        .name("saule-profiler".into())
        .spawn(|| {
            while ENABLED.load(Relaxed) {
                std::thread::sleep(INTERVAL);
                NATIVE_AT_TICK.store(NATIVE_NOW.load(Relaxed), Relaxed);
                TICKS.fetch_add(1, Relaxed);
                PENDING.store(true, Relaxed);
            }
        })
        .expect("start the profiler thread");
}

/// Start profiling if the environment asks for it: `SAULE_PROFILE=1` for
/// stderr, any other value for that file.
pub fn start_from_env() {
    if let Some(v) = std::env::var_os("SAULE_PROFILE").filter(|v| !v.is_empty()) {
        start((v != "1").then(|| PathBuf::from(v)));
    }
}

/// Stop, and deliver the report to where `start` said.
pub fn finish() {
    if !ENABLED.swap(false, Relaxed) {
        return;
    }
    PROFILE.with(|p| {
        let p = p.borrow();
        let report = render(&p);
        match &p.target {
            Some(path) => {
                if let Err(e) = std::fs::write(path, &report) {
                    eprintln!("profile: could not write {}: {e}", path.display());
                } else {
                    eprintln!("profile: written to {}", path.display());
                }
            }
            None => eprint!("\n{report}"),
        }
    });
}

/// Whether a sample is due: the one check on the hot path.
#[inline(always)]
pub(crate) fn due() -> bool {
    PENDING.load(Relaxed)
}

/// Whether the profiler is on at all.
#[inline(always)]
pub(crate) fn on() -> bool {
    ENABLED.load(Relaxed)
}

/// A native is about to run: what the sampler reports while it does.
#[inline(always)]
pub(crate) fn enter_native(name: &'static str) -> usize {
    if !ENABLED.load(Relaxed) {
        return 0;
    }
    let index = PROFILE.with(|p| {
        let mut p = p.borrow_mut();
        if let Some(&i) = p.native_index.get(name) {
            return i;
        }
        let i = p.natives.len();
        p.natives.push(name);
        p.native_index.insert(name, i);
        i
    });
    NATIVE_NOW.swap(index + 1, Relaxed)
}

/// The native has returned; `previous` is what `enter_native` handed back.
#[inline(always)]
pub(crate) fn leave_native(previous: usize) {
    if ENABLED.load(Relaxed) {
        NATIVE_NOW.store(previous, Relaxed);
    }
}

/// Record the stack — frames innermost last — weighted by the ticks since
/// the last record.
#[cold]
#[inline(never)]
pub(crate) fn record<'a>(frames: impl DoubleEndedIterator<Item = (&'a Proto, &'a Chunk)>) {
    if !PENDING.swap(false, Relaxed) || !ENABLED.load(Relaxed) {
        return;
    }
    let weight = TICKS.swap(0, Relaxed);
    let native = NATIVE_AT_TICK.swap(0, Relaxed);
    if weight == 0 {
        return;
    }
    PROFILE.with(|p| {
        let mut p = p.borrow_mut();
        p.ticks += weight;

        let mut seen: Vec<Key> = Vec::new();
        let mut top: Option<Key> = None;
        if native > 0 {
            let key = Key::Native(native - 1);
            let name = p.natives.get(native - 1).copied().unwrap_or("?");
            p.names
                .entry(key)
                .or_insert_with(|| format!("{name} (native)"));
            top = Some(key);
            seen.push(key);
        }
        for (proto, chunk) in frames.rev() {
            let key = Key::Saule(proto as *const Proto as usize);
            p.names.entry(key).or_insert_with(|| describe(proto, chunk));
            if top.is_none() {
                top = Some(key);
            }
            if !seen.contains(&key) {
                seen.push(key);
            }
        }
        if let Some(top) = top {
            *p.self_ticks.entry(top).or_default() += weight;
        }
        for key in seen {
            *p.total_ticks.entry(key).or_default() += weight;
        }

        if p.target.is_some() && p.last_write.is_none_or(|t| t.elapsed() >= WRITE_EVERY) {
            p.last_write = Some(Instant::now());
            if let Some(path) = &p.target {
                let _ = std::fs::write(path, render(&p));
            }
        }
    });
}

/// `Class.method  file:line`, as the report names a function.
fn describe(proto: &Proto, chunk: &Chunk) -> String {
    let top_level = std::ptr::eq(proto, &**chunk.proto(chunk.main));
    let name = match &proto.name {
        _ if top_level => "<top level>".to_string(),
        Some(name) => match proto
            .owner_class
            .and_then(|c| chunk.classes.get(c as usize))
        {
            Some(class) => format!("{}.{name}", class.name),
            None => name.to_string(),
        },
        None => "<lambda>".to_string(),
    };
    let source = chunk.source.inner();
    let at = proto.span_at(0).start.min(source.len());
    let line = source.as_bytes()[..at]
        .iter()
        .filter(|b| **b == b'\n')
        .count()
        + 1;
    format!("{name}  {}:{line}", chunk.source.name())
}

fn render(p: &Profile) -> String {
    let mut out = String::new();
    let elapsed = p.started.map(|s| s.elapsed()).unwrap_or_default();
    out.push_str(&format!(
        "profile: {} ms sampled every {} ms, over {:.1} s\n",
        p.ticks,
        INTERVAL.as_millis(),
        elapsed.as_secs_f64()
    ));
    if p.ticks == 0 {
        out.push_str("  (no samples: the program ran no Saule code)\n");
        return out;
    }
    let pct = |n: u64| 100.0 * n as f64 / p.ticks as f64;

    let mut rows: Vec<(&Key, &u64)> = p.self_ticks.iter().collect();
    rows.sort_by(|a, b| b.1.cmp(a.1));
    out.push_str("\nby self time — where the time is actually spent\n");
    out.push_str("   self   total  function\n");
    for (key, ticks) in rows.into_iter().take(ROWS) {
        let total = p.total_ticks.get(key).copied().unwrap_or(0);
        let name = p.names.get(key).map(String::as_str).unwrap_or("?");
        out.push_str(&format!(
            "{:6.1}% {:6.1}%  {name}\n",
            pct(*ticks),
            pct(total)
        ));
    }

    let mut rows: Vec<(&Key, &u64)> = p.total_ticks.iter().collect();
    rows.sort_by(|a, b| b.1.cmp(a.1));
    out.push_str("\nby total time — what it was called from\n");
    out.push_str("  total    self  function\n");
    for (key, ticks) in rows.into_iter().take(ROWS) {
        let own = p.self_ticks.get(key).copied().unwrap_or(0);
        let name = p.names.get(key).map(String::as_str).unwrap_or("?");
        out.push_str(&format!("{:6.1}% {:6.1}%  {name}\n", pct(*ticks), pct(own)));
    }
    out
}
