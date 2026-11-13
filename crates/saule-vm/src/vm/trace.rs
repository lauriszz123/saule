//! The call stack of an error that left the program.
//!
//! Recorded only as a failure is on its way out, so a program that does not
//! fail pays nothing for it. It lives beside the error rather than inside it:
//! `RuntimeError` is matched and compared all over, and what it says must not
//! change depending on how the program reached the fault.
//!
//! A call in tail position replaces its caller's frame, so that caller does
//! not appear. That is the same frame the program no longer has.

use std::cell::RefCell;
use std::rc::Rc;

use crate::chunk::{Chunk, Proto};

use super::{Frame, Vm};

/// Enough to read, and a bound on what a stack overflow costs to report.
const MAX_FRAMES: usize = 64;

/// One call on the stack, innermost first.
#[derive(Debug, Clone, PartialEq)]
pub struct TraceFrame {
    pub function: String,
    pub file: String,
    /// 1-based. `None` for a frame that was running rather than calling, whose
    /// position is the error's own and is already reported by it.
    pub line: Option<usize>,
}

/// What [`take_stack_trace`] hands back.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StackTrace {
    pub frames: Vec<TraceFrame>,
    /// Frames beyond [`MAX_FRAMES`], which were counted but not kept.
    pub omitted: usize,
}

thread_local! {
    static TRACE: RefCell<StackTrace> = RefCell::new(StackTrace::default());
}

/// The trace of the most recent uncaught failure on this thread, emptied.
pub fn take_stack_trace() -> StackTrace {
    TRACE.with(|t| std::mem::take(&mut *t.borrow_mut()))
}

pub(crate) fn clear() {
    TRACE.with(|t| *t.borrow_mut() = StackTrace::default());
}

fn describe(proto: &Proto, chunk: &Chunk, at: Option<u32>) -> TraceFrame {
    let source = chunk.source.inner();
    let line = at.map(|pc| line_of(source, proto.span_at(pc).start));
    let top_level = std::ptr::eq(proto, &**chunk.proto(chunk.main));
    TraceFrame {
        function: match &proto.name {
            _ if top_level => "<top level>".to_string(),
            Some(name) => name.to_string(),
            None => "<lambda>".to_string(),
        },
        file: chunk.source.name().to_string(),
        line,
    }
}

/// 1-based line of a byte offset into `source`.
fn line_of(source: &str, offset: usize) -> usize {
    let offset = offset.min(source.len());
    source.as_bytes()[..offset]
        .iter()
        .filter(|b| **b == b'\n')
        .count()
        + 1
}

fn push(frame: TraceFrame) {
    TRACE.with(|t| {
        let mut trace = t.borrow_mut();
        if trace.frames.len() < MAX_FRAMES {
            trace.frames.push(frame);
        } else {
            trace.omitted += 1;
        }
    });
}

/// Frames a `throw` popped while looking for a handler, innermost first.
pub(crate) type Popped = Vec<(Rc<Proto>, Rc<Chunk>, u32)>;

pub(crate) fn record_popped(popped: &Popped) {
    for (proto, chunk, at) in popped {
        push(describe(proto, chunk, Some(*at)));
    }
}

impl Vm {
    /// Add this VM's frames to the trace of a failure leaving it.
    ///
    /// A callback a native calls runs on a VM of its own, so a failure inside
    /// one passes through here once per VM, innermost first.
    ///
    /// `at` is where the error itself points, an offset into the running
    /// frame's file. The running frame has no saved `pc` to read its line
    /// from, but the error was raised in it, so the error's position is its
    /// line.
    pub(crate) fn record_trace(&self, at: Option<usize>) {
        for (i, frame) in self.frames.iter().rev().enumerate() {
            let mut traced = describe_frame(frame, i == 0);
            if i == 0 && traced.line.is_none() {
                traced.line = at.map(|offset| line_of(frame.chunk.source.inner(), offset));
            }
            push(traced);
        }
    }

    /// Back to no call in progress, after a failure the caller is going to
    /// carry on from.
    pub(crate) fn reset_after_failure(&mut self) {
        self.close_upvalues(0);
        self.frames.clear();
        self.results.clear();
    }
}

fn describe_frame(frame: &Frame, running: bool) -> TraceFrame {
    // A saved pc points past the call it made, and only a frame that is
    // not the running one has saved its pc.
    let at = (!running).then(|| frame.pc.saturating_sub(1));
    describe(&frame.proto, &frame.chunk, at)
}
