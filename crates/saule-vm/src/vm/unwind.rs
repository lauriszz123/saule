//! Throwing: finding the handler that catches, and the type tests it applies.
//!
//! Handlers are recorded as instruction ranges on the proto rather than
//! emitted inline (§12.1), so entering a `try` costs nothing and only a
//! throw pays — it walks the frames, and in each one the handler table.

use std::rc::Rc;

use saule_interpreter::{RuntimeError, Value};

use crate::chunk::{Chunk, Proto};

use super::{Closure, Vm};

impl Vm {

    /// Find a handler for `value`, unwinding frames until one matches.
    ///
    /// The happy path pays **nothing** for this: entering a `try` emits no
    /// instructions at all, and only a `throw` ever consults the table
    /// (§12.1). It also means the thrown value never enters a
    /// `RuntimeError` unless it escapes to the top, which is what makes the
    /// tree-walker's `thrown_slot` thread-local unnecessary here.
    pub(crate) fn unwind(
        &mut self,
        value: Value,
        proto: &Proto,
        here: u32,
    ) -> Result<(), RuntimeError> {
        // Taken before the first `pop`: `proto` is borrowed from the frame
        // this throw is unwinding out of (see `Frame::proto`), so it stops
        // being safe to read the moment that frame goes. Only the escaping
        // case uses it, and a throw is cold enough not to care.
        let span = proto.span_at(here);
        while let Some(frame) = self.frames.last() {
            let func = Rc::clone(&frame.func);
            let (base, pc) = (frame.base, frame.pc);
            let closure = Closure::from_handle(&func).expect("frame holds a Closure");
            let hchunk = Rc::clone(&closure.chunk);
            // The saved pc points *past* the throwing instruction, so the
            // range test uses the instruction itself.
            let at = pc.saturating_sub(1);

            let found = closure
                .proto
                .handlers
                .iter()
                .find(|h| at >= h.pc_start && at < h.pc_end && self.value_matches(&hchunk, &value, h.catch_ty))
                .map(|h| (h.target, h.err_reg));

            if let Some((target, err_reg)) = found {
                // Anything a closure captured inside the `try` must be
                // closed before its registers are reused by the handler.
                self.close_upvalues(base + err_reg as u32);
                self.ensure_stack(base as usize + err_reg as usize + 1);
                self.stack[base as usize + err_reg as usize] = value;
                let f = self.frames.last_mut().expect("checked");
                f.pc = target;
                return Ok(());
            }

            self.close_upvalues(base);
            self.frames.pop();
        }

        Err(RuntimeError::Thrown { value: value.to_display_string(), span })
    }

    pub(crate) fn type_matches(&self, chunk: &Chunk, reg: usize, ty: u32) -> bool {
        let v = &self.stack[reg];
        self.value_matches(chunk, v, ty)
    }

    /// Does `v` satisfy the type descriptor `ty`?
    ///
    /// `type_descs` are per chunk, so the descriptor is read from the module
    /// whose code posed the question — the `catch` clause's own, not
    /// whichever module happens to be running.
    fn value_matches(&self, chunk: &Chunk, v: &Value, ty: u32) -> bool {
        use crate::chunk::TypeDesc;
        let Some(desc) = chunk.type_descs.get(ty as usize) else {
            return true;
        };
        match desc {
            TypeDesc::Any => true,
            TypeDesc::Nil => matches!(v, Value::Nil),
            TypeDesc::Bool => matches!(v, Value::Bool(_)),
            TypeDesc::Int => matches!(v, Value::Int(_)),
            TypeDesc::Float => matches!(v, Value::Float(_)),
            TypeDesc::Str => matches!(v, Value::Str(_)),
            TypeDesc::Table => matches!(v, Value::Table(_)),
            TypeDesc::Function => matches!(
                v,
                Value::Native(_) | Value::NativeClosure(_) | Value::Function(_) | Value::VmFunction(_)
            ),
            TypeDesc::Class(idx) => match v {
                Value::Instance(i) => self.is_a(&i.borrow().class, *idx),
                _ => false,
            },
            TypeDesc::Enum(idx) => match v {
                Value::EnumVariant(ev) => chunk
                    .enums
                    .get(*idx as usize)
                    .is_some_and(|e| e.name.as_ref() == ev.enum_name.as_str()),
                _ => false,
            },
            TypeDesc::Nullable(inner) => {
                matches!(v, Value::Nil) || self.value_matches(chunk, v, *inner)
            }
            // A name from another module: compare by class name, which is
            // what the tree-walker's `catch` test does too.
            TypeDesc::Named(n) => match v {
                Value::Instance(i) => i.borrow().class.name == n.as_ref(),
                Value::EnumVariant(ev) => ev.enum_name == n.as_ref(),
                _ => false,
            },
        }
    }

    /// Whether `class` is `want` or descends from it.
    pub(crate) fn is_a(&self, class: &Rc<saule_interpreter::value::ClassObject>, want: u32) -> bool {
        let Some(target) = self.shared.classes.get(want as usize) else {
            return false;
        };
        let mut cur = Some(Rc::clone(class));
        while let Some(c) = cur {
            if Rc::ptr_eq(&c, target) {
                return true;
            }
            cur = c.parent.clone();
        }
        false
    }

}
