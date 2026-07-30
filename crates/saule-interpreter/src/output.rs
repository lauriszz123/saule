//! Where the interpreter's output goes.
//!
//! `print`, `println`, `printf` and `Io.write` used to call `print!` /
//! `std::io::stdout()` directly. That is fine for the CLI and wrong
//! everywhere else:
//!
//! * On `wasm32-unknown-unknown` there is no stdout. Writes are discarded, so
//!   a program run in the browser would produce no visible output at all.
//! * Tests that want to assert on what a program printed have to shell out to
//!   the binary and capture a pipe, rather than just running the interpreter.
//!
//! So every write now goes through [`write`], which routes to whatever
//! [`Sink`] the embedder installed — and to real stdout/stderr when nobody
//! installed one, which is exactly the previous behaviour.
//!
//! ```
//! use saule_interpreter::output::{self, Stream};
//!
//! let (captured, ()) = output::capture(|| {
//!     output::write(Stream::Stdout, "hello");
//! });
//! assert_eq!(captured.text(), "hello");
//! ```
//!
//! The sink is **thread-local**, matching the interpreter itself: `Value` is
//! built on `Rc`/`RefCell`, so an interpreter instance never leaves the thread
//! that created it. Two threads can run programs with two different sinks and
//! neither sees the other's output.

use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

/// Which stream a write is destined for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

/// A destination for program output.
///
/// Implementors receive text exactly as the program emitted it — no line
/// buffering, no added newlines — so a sink can faithfully reproduce
/// `print("a") print("b")` as the chunks `a` then `b`.
pub trait Sink {
    /// Handle one write.
    fn write(&mut self, stream: Stream, text: &str);

    /// Handle an explicit `Io.flush`. Sinks that buffer nothing may ignore it.
    fn flush(&mut self) {}
}

thread_local! {
    /// The sink for this thread, or `None` to write straight to the process's
    /// real stdout/stderr.
    static SINK: RefCell<Option<Box<dyn Sink>>> = const { RefCell::new(None) };
}

/// Install `sink` as this thread's output destination, returning whatever was
/// installed before so a caller can restore it.
///
/// Prefer [`capture`] or [`with_sink`] — they restore the previous sink even
/// if the program panics, which matters because an interpreter run can abort
/// partway through.
pub fn set_sink(sink: Option<Box<dyn Sink>>) -> Option<Box<dyn Sink>> {
    SINK.with(|s| std::mem::replace(&mut *s.borrow_mut(), sink))
}

/// Run `f` with `sink` installed, restoring the previous sink afterwards.
///
/// The restore happens even if `f` panics: the guard's `Drop` runs during
/// unwinding, so a panicking program cannot leave a stale sink installed for
/// whatever runs on this thread next.
pub fn with_sink<F, R>(sink: Box<dyn Sink>, f: F) -> R
where
    F: FnOnce() -> R,
{
    struct Restore(Option<Box<dyn Sink>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            set_sink(self.0.take());
        }
    }

    let _restore = Restore(set_sink(Some(sink)));
    f()
}

/// Run `f` with everything it prints collected, returning the collected output
/// alongside `f`'s own result.
///
/// The common case, and what the wasm entry point and output-asserting tests
/// both want.
pub fn capture<F, R>(f: F) -> (CaptureSink, R)
where
    F: FnOnce() -> R,
{
    // The sink handed to `set_sink` and the one returned to the caller share
    // one buffer, which avoids having to recover a concrete type back out of
    // a `Box<dyn Sink>`.
    let handle = CaptureSink::default();
    let result = with_sink(Box::new(handle.clone()), f);
    (handle, result)
}

/// Emit `text` on `stream`.
///
/// Goes to the installed [`Sink`] if there is one, and to the process's real
/// stdout/stderr otherwise. Errors writing to the real streams are ignored,
/// matching the `print!` macro this replaced — a closed stdout should not turn
/// into a Saule-level error.
pub fn write(stream: Stream, text: &str) {
    if dispatch(|sink| sink.write(stream, text)) {
        return;
    }

    match stream {
        Stream::Stdout => {
            let mut out = std::io::stdout();
            let _ = out.write_all(text.as_bytes());
            let _ = out.flush();
        }
        Stream::Stderr => {
            let mut err = std::io::stderr();
            let _ = err.write_all(text.as_bytes());
            let _ = err.flush();
        }
    }
}

/// Flush the installed sink, or the real stream when there is none.
pub fn flush(stream: Stream) {
    if dispatch(|sink| sink.flush()) {
        return;
    }
    match stream {
        Stream::Stdout => {
            let _ = std::io::stdout().flush();
        }
        Stream::Stderr => {
            let _ = std::io::stderr().flush();
        }
    }
}

/// Hand the installed sink to `f`, reporting whether there was one.
///
/// The borrow is released before returning, so a sink implementation is free
/// to write output of its own without re-entrancy panicking.
fn dispatch(f: impl FnOnce(&mut dyn Sink)) -> bool {
    SINK.with(|s| match s.borrow_mut().as_mut() {
        Some(sink) => {
            f(&mut **sink);
            true
        }
        None => false,
    })
}

/// One contiguous piece of program output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub stream: Stream,
    pub text: String,
}

/// A [`Sink`] that records everything written to it.
///
/// Cloning shares the underlying buffer rather than copying it, which is what
/// lets [`capture`] install one handle and return another.
///
/// Chunks are kept separate and in emission order rather than concatenated, so
/// a consumer can colour stderr differently from stdout — which is what the
/// playground's output pane does.
#[derive(Debug, Default, Clone)]
pub struct CaptureSink {
    chunks: Rc<RefCell<Vec<Chunk>>>,
}

impl CaptureSink {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every chunk, in the order the program emitted it.
    pub fn chunks(&self) -> Vec<Chunk> {
        self.chunks.borrow().clone()
    }

    /// How many chunks were recorded.
    pub fn len(&self) -> usize {
        self.chunks.borrow().len()
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.borrow().is_empty()
    }

    /// All output concatenated, both streams interleaved in emission order.
    pub fn text(&self) -> String {
        self.chunks
            .borrow()
            .iter()
            .map(|c| c.text.as_str())
            .collect()
    }

    /// Only what went to the given stream.
    pub fn stream_text(&self, stream: Stream) -> String {
        self.chunks
            .borrow()
            .iter()
            .filter(|c| c.stream == stream)
            .map(|c| c.text.as_str())
            .collect()
    }
}

impl Sink for CaptureSink {
    fn write(&mut self, stream: Stream, text: &str) {
        let mut chunks = self.chunks.borrow_mut();
        // Coalesce consecutive writes to the same stream. A `println` of five
        // arguments is several writes; storing them as one chunk keeps the
        // consumer's job simple without losing any ordering information.
        if let Some(last) = chunks.last_mut() {
            if last.stream == stream {
                last.text.push_str(text);
                return;
            }
        }
        chunks.push(Chunk {
            stream,
            text: text.to_string(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_in_emission_order() {
        let (out, ()) = capture(|| {
            write(Stream::Stdout, "a");
            write(Stream::Stderr, "boom");
            write(Stream::Stdout, "b");
        });
        assert_eq!(out.text(), "aboomb");
        assert_eq!(out.stream_text(Stream::Stdout), "ab");
        assert_eq!(out.stream_text(Stream::Stderr), "boom");
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn coalesces_same_stream_writes() {
        let (out, ()) = capture(|| {
            write(Stream::Stdout, "a");
            write(Stream::Stdout, "b");
            write(Stream::Stdout, "c");
        });
        assert_eq!(out.len(), 1);
        assert_eq!(out.text(), "abc");
    }

    #[test]
    fn passes_through_the_closure_result() {
        let (out, n) = capture(|| {
            write(Stream::Stdout, "x");
            41 + 1
        });
        assert_eq!(n, 42);
        assert_eq!(out.text(), "x");
    }

    #[test]
    fn restores_previous_sink_after_capture() {
        let (outer, ()) = capture(|| {
            write(Stream::Stdout, "outer-before");
            let (inner, ()) = capture(|| write(Stream::Stdout, "inner"));
            assert_eq!(inner.text(), "inner");
            write(Stream::Stdout, "outer-after");
        });
        // The inner capture must not have swallowed the outer's writes, and
        // the outer sink must be back in place once it returns.
        assert_eq!(outer.text(), "outer-beforeouter-after");
    }

    #[test]
    fn restores_sink_when_the_program_panics() {
        let panicked = std::panic::catch_unwind(|| {
            let _ = capture(|| panic!("program blew up"));
        })
        .is_err();
        assert!(panicked, "expected the panic to propagate");

        // If the guard failed to run, this capture would see a stale sink and
        // the write would land in the abandoned one instead.
        let (after, ()) = capture(|| write(Stream::Stdout, "still works"));
        assert_eq!(after.text(), "still works");
    }
}
