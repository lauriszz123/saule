//! How many values a call site wants, and where the callee left them.
//!
//! Saule is multi-return, so "the value of a call" is a *run of registers*
//! rather than one register. [`Want`] is the question asked before the call
//! and [`Results`] is the answer after it; the four helpers below are how a
//! caller turns that answer back into an ordinary single-register value.

use std::ops::Range;

use super::CompileError;
use super::super::ctx::Compiler;
use crate::op::{Instruction, Op};

/// How many values a call site wants back (§6.2's `C` operand).
///
/// Saule is multi-return, but almost every call site consumes exactly one
/// value, which is why `Fixed(1)` is the default everywhere and the other
/// two shapes are reached only from a parallel `local`/assignment and from
/// `return f()`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Want {
    /// Exactly `n`. A callee returning fewer is padded with nil and a
    /// callee returning more has the surplus dropped — which is what
    /// `pop_frame` does with a non-zero `C`, and exactly the tree-walker's
    /// "extras → nil, surplus dropped".
    Fixed(u8),
    /// Every value the callee produced. The count is a **run-time** fact, so
    /// the results stay in the call window and the frame's `top` marks their
    /// end; only `RET A 0` can read that, which is why `return f()` is the
    /// sole caller.
    All,
    /// `return f(args)` in **tail position** — replace the frame instead of
    /// nesting inside it (§6.4).
    ///
    /// A request, not a promise. Only the three shapes that can actually
    /// replace a frame honour it; a constructor, a native, a method call or
    /// a pipeline treats it as [`Want::All`] and reports back through
    /// [`Results::terminated`], because the tree-walker draws the line in the same
    /// place — `Flow::TailCall` is built only for a `Value::Function`.
    Tail,
}

impl Want {
    /// The `C` operand: `nret + 1`, with 0 meaning "all results, set `top`".
    pub(crate) fn c(self) -> u8 {
        match self {
            Want::Fixed(n) => n + 1,
            // A tail call that has to fall back to an ordinary one still
            // has to hand every result through, so it wants what `All`
            // wants.
            Want::All | Want::Tail => 0,
        }
    }

    /// Registers the call window must reserve for the results it will
    /// receive. `All` reserves one; anything beyond that lands above the
    /// frame's high-water mark, which is safe because the window is the
    /// top of the register file and `RET` consumes it immediately.
    pub(crate) fn slots(self) -> u16 {
        match self {
            Want::Fixed(n) => n as u16,
            Want::All | Want::Tail => 1,
        }
    }
}

/// Where a call left its results.
pub(crate) struct Results {
    /// First register of the run.
    pub base: u16,
    /// How many registers from `base` hold results, or `None` when only the
    /// VM knows — the run then ends at the frame's `top`.
    pub count: Option<u8>,
    /// Control has already left the function: nothing is left to return and
    /// the caller must emit no `RET`.
    ///
    /// Two things set it, and both are only reachable from a `return`, which
    /// is what makes writing control flow from an expression helper
    /// legitimate here: a **tail call**, which replaced the frame, and
    /// `return x?.m()`, whose two arms return separately because their
    /// result counts differ and only one of them is known at compile time.
    pub terminated: bool,
}

impl Compiler<'_> {

    pub(crate) fn move_result(
        &mut self,
        from: u16,
        dst: u16,
        span: &Range<usize>,
    ) -> Result<(), CompileError> {
        if from == dst {
            return Ok(());
        }
        let (a, b) = (self.reg8(dst, span)?, self.reg8(from, span)?);
        self.emit(Instruction::abc(Op::MOVE, a, b, 0), span);
        Ok(())
    }

    /// Land a call's results where the caller asked, and say where they are.
    ///
    /// `m` is the mark taken before the call window was allocated. A `Fixed`
    /// count moves down to `dst` — ascending, which is safe because the
    /// window is always above `dst` — and the window is released.
    /// [`Want::All`] can move nothing, since the count is only known once the
    /// callee has returned, so the window **stays allocated** and its base is
    /// handed back for the caller to consume with `RET A 0`.
    pub(crate) fn finish_call(
        &mut self,
        base: u16,
        dst: u16,
        want: Want,
        m: crate::compile::regalloc::Mark,
        span: &Range<usize>,
    ) -> Result<Results, CompileError> {
        match want {
            // A `Tail` that reached here is one of the shapes that cannot
            // replace a frame, so it behaves as `All` and says `terminated: false`
            // — the caller then emits the `RET` it would have anyway.
            Want::All | Want::Tail => Ok(Results { base, count: None, terminated: false }),
            Want::Fixed(n) => {
                for i in 0..n as u16 {
                    self.move_result(base + i, dst + i, span)?;
                }
                self.free_to(m);
                Ok(Results {
                    base: dst,
                    count: Some(n),
                    terminated: false,
                })
            }
        }
    }

    /// The frame was replaced; there is nothing to return and nothing to
    /// move. See [`Results::terminated`].
    pub(crate) fn tail_result(base: u16) -> Results {
        Results { base, count: None, terminated: true }
    }

    /// A call shape that yields exactly one value, already written to `dst`.
    ///
    /// Constructors, variant constructors, `self.super()` and pipelines are
    /// all single-valued under the tree-walker too, so padding the surplus
    /// with nil here is `eval_expr_list`'s own rule rather than a shortcut:
    /// `local a, b = Foo()` binds the instance and a nil.
    pub(crate) fn one_result(
        &mut self,
        dst: u16,
        want: Want,
        span: &Range<usize>,
    ) -> Result<Results, CompileError> {
        let n = match want {
            Want::Fixed(n) => n,
            Want::All | Want::Tail => {
                return Ok(Results { base: dst, count: Some(1), terminated: false });
            }
        };
        for i in 1..n as u16 {
            let a = self.reg8(dst + i, span)?;
            self.emit(Instruction::abc(Op::LOADNIL, a, 0, 0), span);
        }
        Ok(Results {
            base: dst,
            count: Some(n),
            terminated: false,
        })
    }

}
