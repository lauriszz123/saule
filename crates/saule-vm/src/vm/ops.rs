//! Reading operands out of registers, and the small numeric helpers.
//!
//! Every accessor here reports against the *proto and pc* rather than
//! returning an `Option`, because a type error in a register is a program
//! error the user has to be able to locate — the span comes from the line
//! table, so the diagnostic points at source rather than at bytecode.

use std::cell::RefCell;
use std::rc::Rc;

use saule_runtime::error::a_type;
use saule_runtime::value::TableObject;
use saule_runtime::{RuntimeError, Value};

use crate::chunk::{FieldPath, LinkKind, PathLink, Proto};
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

    /// Write `R[i]`, without the call to `Value`'s drop glue that a plain
    /// `*reg_mut(i) = v` emits.
    ///
    /// That call is unconditional in the machine code — every `ADDII`,
    /// `SUBII` and `MULII` ends in one — and on the path that runs it has
    /// nothing to do: the register it overwrites held an `Int`. Testing
    /// [`is_scalar`](Value::is_scalar) here keeps the common case to a tag
    /// compare and a store, and leaves the real drop for the case that
    /// needs it.
    #[inline(always)]
    pub(crate) fn set_reg(&mut self, i: usize, v: Value) {
        let slot = self.reg_mut(i);
        if slot.is_scalar() {
            // SAFETY: the old value owns nothing, so there is no destructor
            // to skip and nothing to leak — overwriting it is the whole of
            // what dropping it would do. `slot` is a valid `&mut Value`, so
            // it is aligned and initialised, which is all `write` asks.
            unsafe { std::ptr::write(slot, v) };
        } else {
            *slot = v;
        }
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

    /// `GETPATH`: `R[dst] := ` the value at the end of `path` from
    /// `R[start]`. `false` leaves the error in [`Vm::fault`].
    ///
    /// Out of line, and answering in a register: in the arm, the walk and
    /// its `Result` were the arm's size, and what that costs the loop is
    /// paid by programs with no path in them (`loop_arith`, `fib` and
    /// `mandel` moved 8 to 10% with it).
    #[inline(never)]
    pub(crate) fn path_into(
        &mut self,
        dst: usize,
        start: usize,
        path: &FieldPath,
        proto: &Proto,
    ) -> bool {
        match self.follow_path(start, path, proto) {
            Ok(at) => {
                // SAFETY: `follow_path` answers a pointer into an
                // instance's fields or to `R[start]`, and nothing has been
                // written since: this is the read it is for.
                //
                // An object or a table at the end of a path is as common as
                // a number — it is what `row.mid != nil` reads — and those
                // two are counted here, in line, where `Value::clone` goes
                // out to a function for anything that is not a number.
                let v = match unsafe { &*at } {
                    Value::Instance(i) => Value::Instance(Rc::clone(i)),
                    Value::Table(t) => Value::Table(Rc::clone(t)),
                    other => other.clone(),
                };
                self.set_reg(dst, v);
                true
            }
            Err(e) => {
                self.fault = Some(Box::new(e));
                false
            }
        }
    }

    /// `GETPATH`'s walk: `R[start]` followed through every link of `path`,
    /// and the value at the end.
    ///
    /// Each link reads its field **in place**. The objects in the middle of
    /// the chain are never copied out — no reference count is touched until
    /// the one `clone` of the value at the end — which is what a `GETF` per
    /// link could not avoid, since each one had to land in a register.
    ///
    /// A failure blames the link it happened on, with the span that link's
    /// own instruction would have carried.
    ///
    /// **Answers where the value is, not the value.** Answering a `Value`
    /// meant an `Ok(clone)`, and a path that ends at an object or a table
    /// — every `row.mid != nil`, every `row.mid?.tags` — had its clone
    /// written to the stack, copied into the `Result`, read back out of it
    /// and written to the stack again before it reached its register:
    /// three round trips through memory, a fifth of `nullsafe`. The arm
    /// clones once, from here to the register.
    ///
    /// The pointer is into an instance's fields (or the register the walk
    /// began at) and is good until something is written: the caller reads
    /// it at once.
    #[inline]
    pub(crate) fn follow_path(
        &self,
        start: usize,
        path: &FieldPath,
        proto: &Proto,
    ) -> Result<*const Value, RuntimeError> {
        let mut cur: &Value = self.reg(start);
        for link in path.links.iter() {
            let inst = match cur {
                // SAFETY: the reference does not outlive this walk, and
                // nothing in it can borrow an instance mutably — no user code
                // runs and no register is written until the final clone has
                // been taken. A *current* mutable borrow is reported as the
                // panic `RefCell::borrow` would raise.
                Value::Instance(i) => match unsafe { i.try_borrow_unguarded() } {
                    Ok(inst) => inst,
                    Err(e) => panic!("{e}"),
                },
                // `?.` on nil: the value stays nil and this read is skipped.
                // A later `.` then meets the nil, exactly as `GETF` would.
                Value::Nil if link.kind == LinkKind::Safe => continue,
                other => return Err(path_operand_err(other, link, proto)),
            };
            cur = match inst.fields.get(link.slot as usize) {
                Some(v) => v,
                None => {
                    return Err(RuntimeError::Internal {
                        message: format!(
                            "internal: field slot {} on an instance with {} field(s) — \
                             the chunk disagrees with the layout it was compiled against",
                            link.slot,
                            inst.fields.len()
                        ),
                        span: link.span(),
                    });
                }
            };
            if link.kind == LinkKind::Unwrap && matches!(cur, Value::Nil) {
                return Err(RuntimeError::ForceUnwrapNil { span: link.span() });
            }
        }
        Ok(cur as *const Value)
    }
}

/// [`operand_err`] for one link of a field path, at that link's span.
#[cold]
fn path_operand_err(got: &Value, link: &PathLink, proto: &Proto) -> RuntimeError {
    RuntimeError::Internal {
        message: format!(
            "internal: typed opcode in `{}` expected `instance` but the register held `{}` — \
             the chunk disagrees with the types it was compiled against",
            proto.label(),
            got.type_name()
        ),
        span: link.span(),
    }
}

// ---- free helpers ------------------------------------------------------

pub(crate) fn field_slot_err(slot: usize, have: usize, proto: &Proto, here: u32) -> RuntimeError {
    RuntimeError::Internal {
        message: format!(
            "internal: field slot {slot} on an instance with {have} field(s) — \
             the chunk disagrees with the layout it was compiled against"
        ),
        span: proto.span_at(here),
    }
}

pub(crate) fn operand_err(got: &Value, want: &str, proto: &Proto, here: u32) -> RuntimeError {
    RuntimeError::Internal {
        message: format!(
            "internal: typed opcode in `{}` expected `{want}` but the register held `{}` — \
             the chunk disagrees with the types it was compiled against",
            proto.label(),
            got.type_name()
        ),
        span: proto.span_at(here),
    }
}

/// `for … in` over something that cannot be iterated.
#[cold]
#[inline(never)]
pub(crate) fn not_iterable(type_name: &str, span: std::ops::Range<usize>) -> RuntimeError {
    RuntimeError::fault(
        format!("cannot iterate over {}", a_type(type_name)),
        format!("this is {}, not a collection", a_type(type_name)),
        span,
    )
    .with_help(
        "`for … in` takes a table, an iterator function, or an object with an `iter()` method (`Iterable`); to count, use `for i = 1, n do`",
    )
}

/// An `Iterable`'s `iter()` that returned something other than a function.
/// `got` reads as the end of "returned …": `nothing`, "a `string`".
#[cold]
#[inline(never)]
pub(crate) fn bad_iter_result(
    source: &Value,
    got: &str,
    span: std::ops::Range<usize>,
) -> RuntimeError {
    let class = match source {
        Value::Instance(inst) => inst.borrow().class.name.clone(),
        other => other.type_name().to_string(),
    };
    RuntimeError::fault(
        format!("`{class}.iter()` returned {got}, not a function"),
        "this loop called `iter()`",
        span,
    )
    .with_help(format!(
        "`{class}.iter()` must return a function; the loop calls it for each next value, and stops when it returns `nil`"
    ))
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
        .is_some_and(|t| saule_runtime::cast::cast(v, t))
}

/// Convert the value in `v` to `cast_types[idx]`.
///
/// No fast half, unlike [`cast_holds`]: a conversion has to read the
/// payload whatever the tag, so there is nothing a tag compare could skip.
///
/// A missing entry is a malformed chunk. Yielding `nil` rather than
/// panicking is the choice `cast_holds_deep` makes for the same situation.
///
/// Out of line for the same reason as [`cast_holds_deep`], and it matters
/// more here: `convert` parses strings into numbers, so inlining this drags
/// integer and float parsing into the dispatch loop — 4 KiB of cold code
/// spreading the arithmetic arms apart, worth ~7% on a tight loop that
/// never casts at all.
#[inline(never)]
pub(crate) fn convert_to(chunk: &crate::chunk::Chunk, idx: usize, v: &Value) -> Value {
    match chunk.cast_types.get(idx) {
        Some(t) => saule_runtime::cast::convert(v, t),
        None => Value::Nil,
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

/// The lease of `t`'s array, for a `for … in` to go over it where it is:
/// `None` for a table the loop has to take a copy of ([`snapshot_pairs`]) —
/// one with entries outside its array, whose order has to be worked out —
/// and for an empty one, which is the copy's business too (the loop never
/// begins).
///
/// A table that is all array is most of what is looped over, and copying
/// it for every loop was most of what a loop cost. The loop reads the
/// array itself instead, element `i` on step `i`, and what a loop has to
/// see — the table as it was when the loop began — is kept by the array:
/// written to while a loop holds its lease, it leaves the loop a copy
/// there (`saule_runtime::value::ArrayPart`).
///
/// Entries added outside the array while the loop runs are not the loop's
/// either way: it began with none.
pub(crate) fn lease_array(t: &Rc<RefCell<TableObject>>) -> Option<Rc<RefCell<TableObject>>> {
    let mut t = t.try_borrow_mut().ok()?;
    if t.array.is_empty() || !t.map.is_empty() {
        return None;
    }
    Some(t.array.lease())
}

/// Flatten a table into `[k1, v1, k2, v2, …]`, array part first and then map
/// entries **sorted by key**.
///
/// The ordering is not incidental: `exec_for_in` sorts its map snapshot, so
/// iteration order is deterministic and observable, and the two engines have
/// to agree about it.
// `clippy::unnecessary_sort_by` wants `sort_by_key(|(k, _)| k)` on the sort
// below. That is precisely the version this function used to have, and it was
// wrong twice over — see the comment at the call. The lint cannot see either
// problem, so it is off for this function rather than papered over at the line.
#[allow(clippy::unnecessary_sort_by)]
pub(crate) fn snapshot_pairs(t: &TableObject) -> Vec<Value> {
    let mut out = Vec::with_capacity((t.array.len() + t.map.len()) * 2);
    for (i, v) in t.array.iter().enumerate() {
        out.push(Value::Int(i as i64 + 1));
        out.push(v.clone());
    }
    let mut entries: Vec<(&saule_runtime::value::TableKey, &Value)> = t.map.iter().collect();
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

impl Vm {
    /// The opcodes behind the one arm the dispatch loop has for them (see
    /// the arm). `false` leaves the error in [`Vm::fault`].
    #[inline(never)]
    pub(crate) fn outlined(
        &mut self,
        op: crate::op::Op,
        base: usize,
        ins: Instruction,
        proto: &Proto,
        here: u32,
    ) -> bool {
        use crate::op::Op;
        match op {
            Op::ISNILP => {
                // SAFETY: the verifier proved `C < paths.len()` (§17).
                let path = unsafe { proto.paths.get_unchecked(ins.c() as usize) };
                match self.follow_path(base + ins.b() as usize, path, proto) {
                    Ok(at) => {
                        // SAFETY: as in `path_into` — looked at before
                        // anything is written.
                        let nil = matches!(unsafe { &*at }, Value::Nil);
                        self.set_reg(base + ins.a() as usize, Value::Bool(nil));
                        true
                    }
                    Err(e) => {
                        self.fault = Some(Box::new(e));
                        false
                    }
                }
            }
            Op::SETFIDX => self.field_index_set(base, ins, proto, here),
            _ => self.field_index_get(base, ins, proto, here, op == Op::GETFIDXU),
        }
    }

    /// `GETFIDX` and, with `unwrap`, `GETFIDXU`: `R[A] := R[0].field[B][R[C]]`.
    ///
    /// What `GETF` and `GETIDX` (or `GETIDXU`) do one after the other, with
    /// each one's own failure, and the table read where it lies in the
    /// field. `false` leaves the error in [`Vm::fault`].
    #[inline]
    fn field_index_get(
        &mut self,
        base: usize,
        ins: Instruction,
        proto: &Proto,
        here: u32,
        unwrap: bool,
    ) -> bool {
        let read = || -> Result<Value, RuntimeError> {
            let Value::Instance(inst) = self.reg(base) else {
                return Err(operand_err(self.reg(base), "instance", proto, here));
            };
            let inst = inst.borrow();
            let slot = ins.b() as usize;
            let Some(field) = inst.fields.get(slot) else {
                return Err(field_slot_err(slot, inst.fields.len(), proto, here));
            };
            let Value::Table(t) = field else {
                return Err(operand_err(field, "table", proto, here));
            };
            let t = t.borrow();
            let key = self.reg(base + ins.c() as usize);
            let v = match key {
                Value::Int(i) => match t.array_index(*i) {
                    Some(at) => t.array[at].clone(),
                    None => t.get(key),
                },
                _ => t.get(key),
            };
            if unwrap && matches!(v, Value::Nil) {
                return Err(RuntimeError::ForceUnwrapNil {
                    span: proto.span_at(here),
                });
            }
            Ok(v)
        };
        match read() {
            Ok(v) => {
                self.set_reg(base + ins.a() as usize, v);
                true
            }
            Err(e) => {
                self.fault = Some(Box::new(e));
                false
            }
        }
    }

    /// `SETFIDX`: `R[0].field[A][R[B]] := R[C]`.
    ///
    /// What `GETF` and `SETIDX` do one after the other. The table is held
    /// for the write and the instance let go of first, so that the write
    /// finds what `SETIDX` finds: the table borrowed, and nothing else.
    /// `false` leaves the error in [`Vm::fault`].
    #[inline]
    fn field_index_set(&mut self, base: usize, ins: Instruction, proto: &Proto, here: u32) -> bool {
        let write = || -> Result<(), RuntimeError> {
            let v = (*self.reg(base + ins.c() as usize)).clone();
            let t = {
                let Value::Instance(inst) = self.reg(base) else {
                    return Err(operand_err(self.reg(base), "instance", proto, here));
                };
                let inst = inst.borrow();
                let slot = ins.a() as usize;
                let Some(field) = inst.fields.get(slot) else {
                    return Err(field_slot_err(slot, inst.fields.len(), proto, here));
                };
                let Value::Table(t) = field else {
                    return Err(operand_err(field, "table", proto, here));
                };
                Rc::clone(t)
            };
            let key = self.reg(base + ins.b() as usize);
            let mut t = t.borrow_mut();
            // An integer inside the array part, written in place, with
            // `on_store` first: `SETIDX`'s own two lines.
            let at = match key {
                Value::Int(i) => t.array_index(*i),
                _ => None,
            };
            let done = match at {
                Some(at) => {
                    saule_runtime::gc::on_store(&v);
                    t.array[at] = v;
                    Ok(())
                }
                None => t.set(key, v),
            };
            done.map_err(|m| saule_runtime::members::table_write_failed(m, proto.span_at(here)))
        };
        match write() {
            Ok(()) => true,
            Err(e) => {
                self.fault = Some(Box::new(e));
                false
            }
        }
    }
}
