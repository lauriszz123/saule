//! Register allocation (`VM_DESIGN.md` §18).
//!
//! A **stack discipline**, not graph colouring. Every Lua-family compiler
//! does it this way and it is sufficient: registers are a frame-local
//! resource with strictly nested lifetimes, so a bump pointer and a
//! high-water mark answer the whole question.
//!
//! ```text
//!   0        n_params      free                       max_regs
//!   ├───────────┼─────────────┼──────────────────────────┤
//!   │ params    │ locals      │ temporaries →            │
//!   └───────────┴─────────────┴──────────────────────────┘
//!                             ▲
//!                             └── `free`: first unused register
//! ```
//!
//! * **Locals** are allocated in declaration order and stay put for their
//!   whole lexical extent.
//! * **Temporaries** allocate at `free` and are released in LIFO order as
//!   subexpressions complete — the caller takes a [`Mark`] before evaluating
//!   a subexpression and releases back to it afterwards.
//! * **Blocks** reset `free` to their entry value on exit, so sibling blocks
//!   share registers. That mirrors exactly what `saule-semantic`'s resolver
//!   already computed in Phase 0.6, which is why a slot handed out here for a
//!   local agrees with the slot the `ResolveTable` recorded.
//! * **`max_regs`** is the high-water mark, recorded on the proto.
//!
//! ## Why 256 is a real limit and not a panic
//!
//! `A`, `B` and `C` are 8 bits (§5.2), so a frame names at most 256
//! registers. §24.4 is explicit that exceeding it must be a clean
//! `CompileError` naming the function and saying what to do — never a panic,
//! and never a silent wrap, which would produce a chunk that runs and
//! computes the wrong answer.

use std::ops::Range;

use crate::compile::CompileError;
use crate::op::MAX_REGS;
use super::Compiler;

/// A saved `free` position. Releasing back to one frees every register
/// allocated since it was taken.
///
/// `#[must_use]` because taking a mark and forgetting to release it is the
/// characteristic register-allocator leak: nothing fails, the frame just
/// grows until it hits the 256 limit in a function that should have needed
/// a dozen registers.
#[must_use = "a mark must be released with `free_to`, or the registers leak"]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark(u16);

/// Ran out of registers. Carries no name or span: the allocator does not
/// know which function it is compiling, so the caller attaches that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Overflow {
    pub needed: usize,
}

impl Overflow {
    /// Attach the context needed for a user-facing diagnostic.
    pub fn at(self, name: &str, span: Range<usize>) -> CompileError {
        CompileError::TooManyRegisters {
            name: name.to_string(),
            needed: self.needed,
            span,
        }
    }
}

/// One function's register file, during compilation.
#[derive(Debug, Default)]
pub struct RegAlloc {
    /// First unused register.
    free: u16,
    /// Largest `free` ever reached — the frame size.
    high_water: u16,
    /// Per-block entry state.
    blocks: Vec<BlockState>,
}

#[derive(Debug, Clone, Copy)]
struct BlockState {
    /// `free` on entry; leaving the block resets to it.
    entry: u16,
    /// Set when something in this block was captured by a closure, so the
    /// compiler must emit `CLOSEUP` before the registers are reused.
    captured: bool,
}

impl RegAlloc {
    pub fn new() -> RegAlloc {
        RegAlloc::default()
    }

    /// Reserve the parameter registers. Parameters occupy `0..n` by the
    /// calling convention (§6.2): the callee's frame *is* the argument
    /// window, so this cannot be anything else.
    pub fn reserve_params(&mut self, n: u16) -> Result<(), Overflow> {
        debug_assert_eq!(self.free, 0, "parameters must be reserved first");
        self.alloc_n(n).map(|_| ())
    }

    /// Allocate one register.
    pub fn alloc(&mut self) -> Result<u16, Overflow> {
        self.alloc_n(1)
    }

    /// Allocate `n` **consecutive** registers, returning the first.
    ///
    /// Consecutive matters: a call needs its callee and arguments adjacent
    /// so the callee's frame can start at `A + 1` without copying anything
    /// (§6.2), and `CONCAT`/`SETLIST` read a register range.
    pub fn alloc_n(&mut self, n: u16) -> Result<u16, Overflow> {
        let base = self.free;
        let end = base as usize + n as usize;
        if end > MAX_REGS as usize {
            return Err(Overflow { needed: end });
        }
        self.free = end as u16;
        self.high_water = self.high_water.max(self.free);
        Ok(base)
    }

    /// Take a mark to release temporaries back to.
    pub fn mark(&self) -> Mark {
        Mark(self.free)
    }

    /// Release every register allocated since `mark`.
    pub fn free_to(&mut self, mark: Mark) {
        debug_assert!(
            mark.0 <= self.free,
            "released to a mark above the current top — marks must nest"
        );
        self.free = mark.0;
    }

    /// First unused register. This is where the next allocation lands, and
    /// where a caller building an argument window starts.
    pub fn top(&self) -> u16 {
        self.free
    }

    /// Frame size: the high-water mark, which is what `Proto::max_regs`
    /// records.
    ///
    /// Fits in a `u8` by construction — [`alloc_n`](Self::alloc_n) refuses
    /// to cross 256 — except for the single boundary case of a frame that
    /// uses all 256, which saturates rather than wrapping to 0.
    pub fn max_regs(&self) -> u8 {
        self.high_water.min(u8::MAX as u16) as u8
    }

    /// Exact high-water mark, un-clamped, for diagnostics.
    pub fn high_water(&self) -> u16 {
        self.high_water
    }

    // ---- blocks --------------------------------------------------------

    pub fn enter_block(&mut self) {
        self.blocks.push(BlockState {
            entry: self.free,
            captured: false,
        });
    }

    /// Leave the current block, freeing its registers.
    ///
    /// Returns `Some(reg)` when a closure captured something in this block,
    /// in which case the compiler must emit `CLOSEUP reg` **before** the
    /// registers are reused — otherwise the next iteration of a loop would
    /// overwrite the value a closure from the previous one still points at
    /// (§7.2).
    pub fn leave_block(&mut self) -> Option<u16> {
        let b = self.blocks.pop().expect("leave_block without enter_block");
        self.free = b.entry;
        b.captured.then_some(b.entry)
    }

    /// Record that a closure captured a register owned by the current block.
    pub fn note_capture(&mut self) {
        if let Some(b) = self.blocks.last_mut() {
            b.captured = true;
        }
    }

    /// Whether the current block has anything a closure captured.
    pub fn block_captured(&self) -> bool {
        self.blocks.last().is_some_and(|b| b.captured)
    }

    pub fn block_depth(&self) -> usize {
        self.blocks.len()
    }
}

impl Compiler<'_> {

    // ---- registers -----------------------------------------------------

    pub fn alloc(&mut self, span: &Range<usize>) -> Result<u16, CompileError> {
        let name = self.func_label();
        self.f.regs.alloc().map_err(|o| o.at(&name, span.clone()))
    }

    pub fn alloc_n(&mut self, n: u16, span: &Range<usize>) -> Result<u16, CompileError> {
        let name = self.func_label();
        self.f.regs.alloc_n(n).map_err(|o| o.at(&name, span.clone()))
    }

    pub fn mark(&self) -> Mark {
        self.f.regs.mark()
    }

    pub fn free_to(&mut self, m: Mark) {
        self.f.regs.free_to(m);
    }


    /// A register operand must fit in an 8-bit field.
    pub fn reg8(&self, r: u16, span: &Range<usize>) -> Result<u8, CompileError> {
        u8::try_from(r).map_err(|_| CompileError::TooManyRegisters {
            name: self.func_label(),
            needed: r as usize + 1,
            span: span.clone(),
        })
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locals_get_consecutive_slots_and_temporaries_are_reclaimed() {
        let mut r = RegAlloc::new();
        r.reserve_params(2).unwrap();
        assert_eq!(r.top(), 2);

        let local = r.alloc().unwrap();
        assert_eq!(local, 2);

        // Evaluate a subexpression into temporaries, then release them.
        let m = r.mark();
        assert_eq!(r.alloc().unwrap(), 3);
        assert_eq!(r.alloc().unwrap(), 4);
        r.free_to(m);

        // The next allocation reuses the space the temporaries had.
        assert_eq!(r.alloc().unwrap(), 3);
        // …but the frame is sized by the peak, not the current top.
        assert_eq!(r.max_regs(), 5);
    }

    #[test]
    fn sibling_blocks_share_registers() {
        // The same property `saule-semantic`'s resolver computes for slots,
        // which is what keeps the two in agreement.
        let mut r = RegAlloc::new();
        r.reserve_params(1).unwrap();

        r.enter_block();
        let a = r.alloc().unwrap();
        assert!(r.leave_block().is_none());

        r.enter_block();
        let b = r.alloc().unwrap();
        assert!(r.leave_block().is_none());

        assert_eq!(a, b, "two blocks that cannot be live at once should share");
        assert_eq!(r.max_regs(), 2);
    }

    #[test]
    fn nested_blocks_stack() {
        let mut r = RegAlloc::new();
        r.enter_block();
        assert_eq!(r.alloc().unwrap(), 0);
        r.enter_block();
        assert_eq!(r.alloc().unwrap(), 1);
        assert_eq!(r.alloc().unwrap(), 2);
        r.leave_block();
        assert_eq!(r.top(), 1, "inner block did not give its registers back");
        r.leave_block();
        assert_eq!(r.top(), 0);
        assert_eq!(r.max_regs(), 3);
    }

    #[test]
    fn a_captured_block_reports_where_to_close() {
        let mut r = RegAlloc::new();
        r.reserve_params(1).unwrap();
        r.enter_block();
        let first = r.alloc().unwrap();
        r.note_capture();
        assert_eq!(
            r.leave_block(),
            Some(first),
            "a captured block must say which register to CLOSEUP at"
        );
    }

    #[test]
    fn capture_is_per_block() {
        let mut r = RegAlloc::new();
        r.enter_block();
        r.enter_block();
        r.note_capture();
        assert!(r.leave_block().is_some());
        // The outer block captured nothing of its own.
        assert!(r.leave_block().is_none());
    }

    #[test]
    fn a_consecutive_run_is_really_consecutive() {
        // Call windows depend on this: callee at A, arguments at A+1…
        let mut r = RegAlloc::new();
        let base = r.alloc_n(4).unwrap();
        assert_eq!(base, 0);
        assert_eq!(r.top(), 4);
        assert_eq!(r.alloc().unwrap(), 4);
    }

    #[test]
    fn running_out_of_registers_is_an_error_not_a_panic() {
        let mut r = RegAlloc::new();
        assert!(r.alloc_n(MAX_REGS).is_ok(), "256 registers must fit");
        match r.alloc() {
            Err(Overflow { needed }) => assert_eq!(needed, 257),
            Ok(reg) => panic!("allocated register {reg} past the 256 limit"),
        }
    }

    #[test]
    fn a_single_huge_request_is_refused_without_wrapping() {
        // The failure this guards: `free + n` overflowing a `u16` and coming
        // back as a small, plausible-looking register index.
        let mut r = RegAlloc::new();
        assert!(r.alloc_n(u16::MAX).is_err());
        assert_eq!(r.top(), 0, "a refused allocation must not move the top");
    }

    #[test]
    fn overflow_carries_enough_to_diagnose() {
        let mut r = RegAlloc::new();
        r.alloc_n(MAX_REGS).unwrap();
        let err = r.alloc_n(10).unwrap_err().at("render", 4..9);
        match err {
            CompileError::TooManyRegisters { name, needed, span } => {
                assert_eq!(name, "render");
                assert_eq!(needed, 266);
                assert_eq!(span, 4..9);
            }
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn max_regs_saturates_rather_than_wrapping() {
        // 256 does not fit in a `u8`. Wrapping it would record a frame size
        // of 0 and hand the VM a chunk that reads uninitialised registers.
        let mut r = RegAlloc::new();
        r.alloc_n(MAX_REGS).unwrap();
        assert_eq!(r.high_water(), 256);
        assert_eq!(r.max_regs(), 255);
    }
}
