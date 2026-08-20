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

    // ---- typed operand reads -------------------------------------------

    pub(crate) fn int_at(&self, i: usize, proto: &Proto, here: u32) -> Result<i64, RuntimeError> {
        match &self.stack[i] {
            Value::Int(n) => Ok(*n),
            other => Err(operand_err(other, "integer", proto, here)),
        }
    }

    pub(crate) fn float_at(&self, i: usize, proto: &Proto, here: u32) -> Result<f64, RuntimeError> {
        match &self.stack[i] {
            Value::Float(n) => Ok(*n),
            other => Err(operand_err(other, "float", proto, here)),
        }
    }

    pub(crate) fn table_at(
        &self,
        i: usize,
        proto: &Proto,
        here: u32,
    ) -> Result<Rc<RefCell<TableObject>>, RuntimeError> {
        match &self.stack[i] {
            Value::Table(t) => Ok(Rc::clone(t)),
            other => Err(operand_err(other, "table", proto, here)),
        }
    }

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
    entries.sort_by_key(|(k, _)| k.display());
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
