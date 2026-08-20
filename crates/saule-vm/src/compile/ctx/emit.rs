//! Writing instructions out: emission, jumps, labels, patching, constants.
//!
//! Every instruction in a proto goes through [`Compiler::emit`], which is
//! also where the line table is built — so a span attaches to bytecode in
//! exactly one place.

use std::ops::Range;

use saule_interpreter::Value;

use crate::chunk::LineEntry;
use crate::compile::CompileError;
use crate::op::{Instruction, Op};

use super::Compiler;

/// A patch site: an emitted jump whose target is not known yet.
#[must_use = "an unpatched jump goes to the wrong place"]
#[derive(Debug, Clone, Copy)]
pub struct Label(usize);

impl Compiler<'_> {

    // ---- emission ------------------------------------------------------

    /// Emit one instruction, recording the span it came from.
    ///
    /// The line table is built as we go and stays sorted by construction,
    /// because `pc` only ever increases. It is out of band — nothing in the
    /// instruction stream refers to it — so it costs nothing until an error
    /// needs a span (§12.3).
    pub fn emit(&mut self, ins: Instruction, span: &Range<usize>) {
        let pc = self.f.code.len() as u32;
        let entry = LineEntry {
            pc,
            span_start: span.start as u32,
            span_end: span.end as u32,
        };
        // Only record a change: consecutive instructions from one expression
        // share an entry, which is most of them.
        if self.f.lines.last().map(|l| (l.span_start, l.span_end)) != Some((entry.span_start, entry.span_end))
        {
            self.f.lines.push(entry);
        }
        self.f.code.push(ins);
    }

    /// Emit a jump whose target is patched later.
    pub fn emit_jump(&mut self, op: Op, a: u8, span: &Range<usize>) -> Label {
        let at = self.f.code.len();
        // A placeholder of 0 is harmless: `patch_to` overwrites the whole
        // word, and the verifier would catch an unpatched one.
        self.emit(Instruction::asbx(op, a, 0), span);
        Label(at)
    }

    /// Emit an `ABx` instruction whose `Bx` is a forward displacement,
    /// patched later. `ITERPREP` and `ITERPREPX` are the only ones.
    pub fn emit_jump_abx(
        &mut self,
        op: Op,
        a: u8,
        span: &Range<usize>,
    ) -> Result<Label, CompileError> {
        let at = self.f.code.len();
        self.emit(Instruction::abx(op, a, 0), span);
        Ok(Label(at))
    }

    /// Point a previously emitted jump at the current position.
    pub fn patch_here(&mut self, label: Label) -> Result<(), CompileError> {
        let target = self.f.code.len();
        self.patch_to(label, target)
    }

    pub fn patch_to(&mut self, label: Label, target: usize) -> Result<(), CompileError> {
        let from = label.0;
        self.f.max_patch_target = self.f.max_patch_target.max(target);
        // A jump's displacement is relative to the instruction *after* it,
        // because the dispatch loop has already advanced `pc` when it
        // applies the offset.
        let disp = target as i64 - (from as i64 + 1);
        let ins = self.f.code[from];
        let op = ins.op().expect("emitted opcode");
        // `ITERPREP`/`ITERPREPX` carry an unsigned forward displacement in
        // `Bx`; every other patch site is a signed `sBx`.
        if op == Op::ITERPREP || op == Op::ITERPREPX {
            let d = u16::try_from(disp).map_err(|_| CompileError::JumpTooFar {
                distance: disp,
                span: self.span_of_pc(from),
            })?;
            self.f.code[from] = Instruction::abx(op, ins.a(), d);
            return Ok(());
        }
        let patched = Instruction::try_asbx(op, ins.a(), disp as i32).ok_or_else(|| {
            CompileError::JumpTooFar {
                distance: disp,
                span: self.span_of_pc(from),
            }
        })?;
        self.f.code[from] = patched;
        Ok(())
    }

    /// Emit a backward jump to an already-known position.
    pub fn emit_jump_back(
        &mut self,
        op: Op,
        a: u8,
        target: usize,
        span: &Range<usize>,
    ) -> Result<(), CompileError> {
        let label = self.emit_jump(op, a, span);
        self.patch_to(label, target)
    }

    fn span_of_pc(&self, pc: usize) -> Range<usize> {
        match self.f.lines.binary_search_by_key(&(pc as u32), |l| l.pc) {
            Ok(i) => {
                let e = &self.f.lines[i];
                e.span_start as usize..e.span_end as usize
            }
            Err(0) => 0..0,
            Err(i) => {
                let e = &self.f.lines[i - 1];
                e.span_start as usize..e.span_end as usize
            }
        }
    }

    // ---- constants -----------------------------------------------------

    pub fn constant(&mut self, v: Value, span: &Range<usize>) -> Result<u16, CompileError> {
        let idx = self.chunk.add_constant(v);
        u16::try_from(idx).map_err(|_| CompileError::Unsupported {
            thing: "a module with more than 65536 constants",
            span: span.clone(),
        })
    }
}
