//! Reading operands out of registers, and the small numeric helpers.
//!
//! Every accessor here reports against the *proto and pc* rather than
//! returning an `Option`, because a type error in a register is a program
//! error the user has to be able to locate — the span comes from the line
//! table, so the diagnostic points at source rather than at bytecode.

use std::cell::RefCell;
use std::rc::Rc;

use saule_interpreter::value::TableObject;
use saule_interpreter::{RuntimeError, Value};

use crate::chunk::Proto;
use crate::op::Instruction;

use super::DEFAULT_MAX_FRAMES;

use super::Vm;

impl Vm {

    // ---- register file -------------------------------------------------

    pub(crate) fn ensure_stack(&mut self, len: usize) {
        if self.stack.len() < len {
            self.stack.resize(len, Value::Nil);
        }
    }

    /// Make registers `..top` available to a frame that is about to run.
    ///
    /// Grows the file to `top` plus [`REG_HEADROOM`](super::REG_HEADROOM),
    /// which is the invariant the dispatch loop's unchecked register access
    /// rests on, and records `top` so the re-entry pool knows how much of
    /// the file was actually written.
    #[inline]
    pub(crate) fn claim_registers(&mut self, top: usize) {
        self.ensure_stack(top + super::REG_HEADROOM);
        if top > self.high_water {
            self.high_water = top;
        }
    }

    /// Read a register of the running frame, without a bounds check.
    ///
    /// # Safety of the unchecked access
    ///
    /// Not `unsafe` to call, because it cannot be called wrongly from the
    /// only place that calls it. Every index the dispatch loop forms is
    /// `base + A (+ B)` out of 8-bit operands, and
    /// [`claim_registers`](Self::claim_registers) leaves
    /// [`REG_HEADROOM`](super::REG_HEADROOM) = 512 live slots above
    /// `base + max_regs` — more than two saturated operand bytes. So the
    /// index is inside the `Vec` for *any* instruction, including a
    /// malformed one, and the verifier's `A < max_regs` is a second, tighter
    /// proof on top.
    #[inline(always)]
    pub(crate) fn reg(&self, i: usize) -> &Value {
        debug_assert!(i < self.stack.len(), "register {i} outside the file");
        unsafe { self.stack.get_unchecked(i) }
    }

    /// [`reg`](Self::reg), mutably. Same argument.
    #[inline(always)]
    pub(crate) fn reg_mut(&mut self, i: usize) -> &mut Value {
        debug_assert!(i < self.stack.len(), "register {i} outside the file");
        unsafe { self.stack.get_unchecked_mut(i) }
    }

    // ---- typed operand reads -------------------------------------------

    /// Marked `#[inline]` so the error half never materialises on the hot
    /// path. `RuntimeError` is 64 bytes, so out of line these return ~72-byte
    /// `Result`s by value for what is, on the path that actually runs, one
    /// discriminant test and a register move.
    #[inline]
    pub(crate) fn int_at(&self, i: usize, proto: &Proto, here: u32) -> Result<i64, RuntimeError> {
        match self.reg(i) {
            Value::Int(n) => Ok(*n),
            other => Err(operand_err(other, "integer", proto, here)),
        }
    }


    #[inline]
    pub(crate) fn float_at(&self, i: usize, proto: &Proto, here: u32) -> Result<f64, RuntimeError> {
        match self.reg(i) {
            Value::Float(n) => Ok(*n),
            other => Err(operand_err(other, "float", proto, here)),
        }
    }


    #[inline]
    pub(crate) fn table_at(
        &self,
        i: usize,
        proto: &Proto,
        here: u32,
    ) -> Result<Rc<RefCell<TableObject>>, RuntimeError> {
        match self.reg(i) {
            Value::Table(t) => Ok(Rc::clone(t)),
            other => Err(operand_err(other, "table", proto, here)),
        }
    }


    #[inline]
    pub(crate) fn int_pair(
        &self,
        base: usize,
        ins: Instruction,
        proto: &Proto,
        here: u32,
    ) -> Result<(i64, i64), RuntimeError> {
        Ok((
            self.int_at(base + ins.b() as usize, proto, here)?,
            self.int_at(base + ins.c() as usize, proto, here)?,
        ))
    }


    #[inline]
    pub(crate) fn float_pair(
        &self,
        base: usize,
        ins: Instruction,
        proto: &Proto,
        here: u32,
    ) -> Result<(f64, f64), RuntimeError> {
        Ok((
            self.float_at(base + ins.b() as usize, proto, here)?,
            self.float_at(base + ins.c() as usize, proto, here)?,
        ))
    }

}

// ---- free helpers ------------------------------------------------------

pub(crate) fn field_slot_err(slot: usize, have: usize, proto: &Proto, here: u32) -> RuntimeError {
    RuntimeError::TypeError {
        message: format!(
            "internal: field slot {slot} on an instance with {have} field(s) — \
             the chunk disagrees with the layout it was compiled against"
        ),
        span: proto.span_at(here),
    }
}

pub(crate) fn operand_err(got: &Value, want: &str, proto: &Proto, here: u32) -> RuntimeError {
    RuntimeError::TypeError {
        message: format!(
            "internal: typed opcode in `{}` expected `{want}` but the register held `{}` — \
             the chunk disagrees with the types it was compiled against",
            proto.label(),
            got.type_name()
        ),
        span: proto.span_at(here),
    }
}

/// Does the value in `v` satisfy `cast_types[idx]`?
///
/// The fast half is a tag compare against the pre-resolved
/// [`CastFast`](crate::chunk::CastFast); the slow half is the tree-walker's
/// own `cast`, unchanged, for the tests that actually need to walk something.
/// See `CastFast` for why the split exists and what it measured.
#[inline(always)]
pub(crate) fn cast_holds(chunk: &crate::chunk::Chunk, idx: usize, v: &Value) -> bool {
    if let Some(f) = chunk.cast_fast.get(idx)
        && let Some(answer) = f.eval(v)
    {
        return answer;
    }
    cast_holds_deep(chunk, idx, v)
}

/// [`cast_holds`]'s fallback, out of line so the tag compare is what the
/// dispatch arm inlines. Not `cold` — `table<T>` and class casts are
/// ordinary code, just not reducible to a tag.
#[inline(never)]
fn cast_holds_deep(chunk: &crate::chunk::Chunk, idx: usize, v: &Value) -> bool {
    // A missing entry is a malformed chunk. Failing the cast rather than
    // panicking is the choice the `is_some_and` here always made.
    chunk
        .cast_types
        .get(idx)
        .is_some_and(|t| saule_interpreter::eval::expr::cast::cast(v, t))
}

#[inline]
pub(crate) fn jump(pc: usize, sbx: i32) -> usize {
    (pc as i64 + sbx as i64) as usize
}

#[inline]
pub(crate) fn int_in_range(i: i64, limit: i64, step: i64) -> bool {
    (step > 0 && i <= limit) || (step < 0 && i >= limit)
}

#[inline]
pub(crate) fn float_in_range(i: f64, limit: f64, step: f64) -> bool {
    (step > 0.0 && i <= limit) || (step < 0.0 && i >= limit)
}

/// Lua 5.3 shift semantics, as `ops::shift` implements them: zero-filled,
/// a negative count shifts the other way, and `|n| >= 64` is 0.
#[inline]
pub(crate) fn shift(a: i64, n: i64) -> i64 {
    if !(-63..=63).contains(&n) {
        return 0;
    }
    if n >= 0 {
        ((a as u64) << n) as i64
    } else {
        ((a as u64) >> -n) as i64
    }
}

/// Flatten a table into `[k1, v1, k2, v2, …]`, array part first and then map
/// entries **sorted by key**.
///
/// The ordering is not incidental: `exec_for_in` sorts its map snapshot, so
/// iteration order is deterministic and observable, and the two engines have
/// to agree about it.
pub(crate) fn snapshot_pairs(t: &TableObject) -> Vec<Value> {
    let mut out = Vec::with_capacity((t.array.len() + t.map.len()) * 2);
    for (i, v) in t.array.iter().enumerate() {
        out.push(Value::Int(i as i64 + 1));
        out.push(v.clone());
    }
    let mut entries: Vec<(&saule_interpreter::value::TableKey, &Value)> = t.map.iter().collect();
    // `TableKey`'s own order, which is the tree-walker's order too — see the
    // comment on its `Ord`. This used to sort on `k.display()`, which built a
    // `String` per *comparison* (`sort_by_key` re-runs its key function, it
    // does not cache) and ordered integer keys lexicographically, so it was
    // both the allocation-heaviest and the wrong answer.
    entries.sort_by(|(a, _), (b, _)| a.cmp(b));
    for (k, v) in entries {
        out.push(k.to_value());
        out.push(v.clone());
    }
    out
}

pub(crate) fn index_array(t: &TableObject, idx: i64) -> Value {
    if idx >= 1 && (idx as usize) <= t.array.len() {
        t.array[(idx - 1) as usize].clone()
    } else {
        t.get(&Value::Int(idx))
    }
}

pub(crate) fn max_frames_from_env() -> usize {
    std::env::var("SAULE_MAX_DEPTH")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_MAX_FRAMES)
}
