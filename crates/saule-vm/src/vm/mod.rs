//! The dispatch loop (`VM_DESIGN.md` §5.3, §6).
//!
//! ## What is implemented
//!
//! Phase 1 + the Phase 2 core: moves and constants, typed integer and float
//! arithmetic, bitwise ops, fused comparison/branch, numeric `for`, tables,
//! closures and upvalues, calls (bytecode, native, statically-resolved), and
//! returns.
//!
//! Everything else — classes, enums and `match`, `try`/`catch`, `for … in`,
//! the dynamic `ARITHX` fallback — decodes and disassembles today but returns
//! `RuntimeError::Unsupported` if executed. That is deliberate: the opcode
//! table is the chunk ABI (see `op.rs`) and freezing it now means Phase 3
//! adds bodies to a `match`, not operands to an encoding.
//!
//! ## Two departures from the design sketch, both on purpose
//!
//! **No `get_unchecked`.** §5.3 licenses it on the strength of the Pass 4
//! verifier, which does not exist yet. Until it does, this loop indexes
//! safely. Turning that back on is a Phase 5 line item with a benchmark
//! attached, not something to inherit by default.
//!
//! **Typed opcodes still tag-check their operands.** `ADDI` matches
//! `Value::Int` rather than assuming it. The typechecker proving the types
//! is what makes the check *predictable* (always taken, perfectly
//! predicted), not what makes it removable — removing it would make a
//! miscompiled chunk read a pointer as an integer.

pub mod frame;
pub mod upval;

use std::cell::RefCell;
use std::rc::Rc;

use saule_interpreter::value::{TableObject, VmFunctionRef};
use saule_interpreter::{RuntimeError, Value};

use crate::chunk::{Chunk, Proto};
use crate::op::{Instruction, Op};

pub use frame::{ALL_RESULTS, Closure, Frame};
pub use upval::Upvalue;

/// Frame-depth limit. `MAX_EVAL_DEPTH = 10_000` in the tree-walker exists
/// because a Saule call is a *native* stack frame there, and a native stack
/// overflow is a `SIGSEGV` rather than a catchable error. Here a call is a
/// `Vec` push, so the limit is pure policy and can be two orders of
/// magnitude higher (§6.4).
pub const DEFAULT_MAX_FRAMES: usize = 1_000_000;

/// The register machine.
pub struct Vm {
    chunk: Rc<Chunk>,
    /// One contiguous register file. Grown, never shrunk.
    stack: Vec<Value>,
    frames: Vec<Frame>,
    /// Open upvalues, sorted ascending by the stack index they point at, so
    /// `CLOSEUP` can pop from the back.
    open_upvals: Vec<Rc<RefCell<Upvalue>>>,
    /// Flat module slots — top-level bindings, no hashing.
    modules: Vec<Value>,
    /// Lazily-built closures for protos that capture nothing, so `CALLK`
    /// does not allocate one per call.
    closure_cache: Vec<Option<Rc<VmFunctionRef>>>,
    /// One runtime class per `ClassProto`, built once at start-up.
    classes: Vec<Rc<saule_interpreter::value::ClassObject>>,
    /// Class identity -> index, so `CALLM` can find a receiver's vtable.
    ///
    /// A hash probe per dynamic dispatch, which is what §8.5's inline cache
    /// exists to remove. Correct first; the cache is a Phase 5 line item
    /// with a benchmark attached.
    class_of: std::collections::HashMap<usize, u32>,
    /// Static fields, flat per class — the `GETSTAT`/`SETSTAT` form. Kept
    /// beside the class rather than inside it because a static is a slot,
    /// not a named entry, once the compiler has resolved it.
    statics: Vec<RefCell<Vec<Value>>>,
    /// One runtime enum per `EnumProto`, built once at start-up.
    enums: Vec<Rc<saule_interpreter::value::EnumObject>>,
    max_frames: usize,
}

impl Vm {
    pub fn new(chunk: Rc<Chunk>) -> Vm {
        let n_protos = chunk.protos.len();
        let module_slots = chunk.module_slots;
        let mut vm = Vm {
            chunk,
            stack: Vec::with_capacity(256),
            frames: Vec::with_capacity(32),
            open_upvals: Vec::new(),
            modules: vec![Value::Nil; module_slots],
            closure_cache: vec![None; n_protos],
            classes: Vec::new(),
            class_of: std::collections::HashMap::new(),
            statics: Vec::new(),
            enums: Vec::new(),
            max_frames: max_frames_from_env(),
        };
        vm.build_classes();
        vm.build_enums();
        vm
    }

    /// Materialise one runtime `ClassObject` per `ClassProto`.
    ///
    /// The layout `Rc` is **shared**, not rebuilt: the class the VM hands to
    /// an instance and the layout the compiler resolved `GETF` against are
    /// the same allocation, so they cannot disagree about which slot a field
    /// occupies (§24.2).
    ///
    /// Protos are ordered parents-first by Pass 1, so a parent is always
    /// available when its child is built.
    fn build_classes(&mut self) {
        use saule_interpreter::value::ClassObject;
        let chunk = Rc::clone(&self.chunk);
        for proto in &chunk.classes {
            let parent = proto.parent.map(|p| Rc::clone(&self.classes[p as usize]));
            let class = Rc::new(ClassObject {
                name: proto.name.to_string(),
                parent,
                field_defs: Vec::new(),
                layout: Rc::clone(&proto.layout),
                // Method dispatch goes through the chunk's vtable, not this
                // map: a bytecode method is a proto, not a `FunctionObject`.
                methods: Default::default(),
                static_fields: RefCell::new(Default::default()),
                static_methods: Default::default(),
                constructor: None,
            });
            self.class_of
                .insert(Rc::as_ptr(&class) as usize, self.classes.len() as u32);
            self.classes.push(class);
            self.statics
                .push(RefCell::new(vec![Value::Nil; proto.n_statics as usize]));
        }
    }

    /// Materialise one runtime enum per `EnumProto`.
    ///
    /// Bare and valued variants are **singletons**, so `Status.Alive` is the
    /// same `Rc` every time it is mentioned and identity comparison works
    /// (§9.1). A tuple variant has no singleton: each call constructs a
    /// fresh object carrying its own payload, stamped with the same tag.
    fn build_enums(&mut self) {
        use saule_interpreter::value::{EnumObject, EnumVariantObject};
        let chunk = Rc::clone(&self.chunk);
        for proto in &chunk.enums {
            let mut variants = saule_interpreter::fxhash::FxHashMap::default();
            let mut by_tag = Vec::with_capacity(proto.variants.len());
            let mut tags = saule_interpreter::fxhash::FxHashMap::default();
            let mut tuple_variants = saule_interpreter::fxhash::FxHashMap::default();

            for (tag, v) in proto.variants.iter().enumerate() {
                tags.insert(v.name.to_string(), tag as u32);
                if v.arity > 0 {
                    tuple_variants.insert(v.name.to_string(), v.arity as usize);
                    by_tag.push(None);
                    continue;
                }
                let obj = Rc::new(EnumVariantObject {
                    enum_name: proto.name.to_string(),
                    variant_name: v.name.to_string(),
                    tag: tag as u32,
                    value: v.value.map(|k| chunk.constants[k as usize].clone()),
                    enum_obj: RefCell::new(None),
                });
                variants.insert(v.name.to_string(), Rc::clone(&obj));
                by_tag.push(Some(obj));
            }

            let e = Rc::new(EnumObject {
                name: proto.name.to_string(),
                variants: variants.clone(),
                by_tag,
                tags,
                tuple_variants,
                methods: Default::default(),
            });
            for v in variants.values() {
                *v.enum_obj.borrow_mut() = Some(Rc::clone(&e));
            }
            self.enums.push(e);
        }
    }

    /// Execute the chunk's `main` proto and return whatever it returns.
    pub fn run(&mut self) -> Result<Vec<Value>, RuntimeError> {
        let main = Rc::clone(self.chunk.proto(self.chunk.main));
        let handle = self.closure_for(self.chunk.main, main);
        self.push_frame(handle, 0, 0, 0, ALL_RESULTS, 0..0)?;
        self.execute()
    }

    /// Call an already-built closure value with `args`. The entry point an
    /// embedder uses to invoke a Saule function it got hold of.
    pub fn call(
        &mut self,
        callee: Value,
        args: &[Value],
    ) -> Result<Vec<Value>, RuntimeError> {
        let Value::VmFunction(handle) = callee else {
            return Err(RuntimeError::TypeError {
                message: format!("attempt to call a `{}`", callee.type_name()),
                span: 0..0,
            });
        };
        let base = self.stack.len() as u32;
        self.ensure_stack(base as usize + args.len());
        for (i, a) in args.iter().enumerate() {
            self.stack[base as usize + i] = a.clone();
        }
        self.push_frame(handle, base, args.len(), base, ALL_RESULTS, 0..0)?;
        self.execute()
    }

    /// Invoke a static method by name, after `run` has executed the module
    /// body.
    ///
    /// This is the project entry point — `class Main` with a
    /// `static fn main()`. The tree-walker's equivalent is
    /// `saule_interpreter::call_class_static_method`, and the CLI needs the
    /// same thing from this engine: running the module body only *declares*
    /// the class, it does not start the program.
    ///
    /// `None` when no such class or method exists, so a caller can tell
    /// "absent" from "failed".
    pub fn call_static(
        &mut self,
        class: &str,
        method: &str,
    ) -> Option<Result<Vec<Value>, RuntimeError>> {
        let chunk = Rc::clone(&self.chunk);
        let proto_idx = chunk.classes.iter().find_map(|c| {
            (c.name.as_ref() == class)
                .then(|| c.smindex.get(method).and_then(|s| c.static_methods.get(*s as usize)))
                .flatten()
                .copied()
        })?;
        if proto_idx == u32::MAX {
            return None;
        }
        let p = Rc::clone(chunk.proto(proto_idx));
        let handle = self.closure_for(proto_idx, p);
        // Start above whatever the module body left behind, so its module
        // slots and any live registers are untouched.
        let base = self.stack.len() as u32;
        if let Err(e) = self.push_frame(handle, base, 0, base, ALL_RESULTS, 0..0) {
            return Some(Err(e));
        }
        Some(self.execute())
    }

    // ---- the loop ------------------------------------------------------

    fn execute(&mut self) -> Result<Vec<Value>, RuntimeError> {
        let entry_depth = self.frames.len();

        'reentry: loop {
            // Re-derived once per frame activation, then held in locals
            // across the whole inner loop (§5.3). `func` keeps the closure
            // alive so `closure` can borrow from it independently of `self`.
            let func = Rc::clone(&self.frames.last().expect("frame").func);
            let closure = Closure::from_handle(&func).expect("VM frame holds a Closure");
            let proto = Rc::clone(&closure.proto);
            let base = self.frames.last().expect("frame").base as usize;
            let mut pc = self.frames.last().expect("frame").pc as usize;
            let code: &[Instruction] = &proto.code;

            loop {
                if pc >= code.len() {
                    return Err(RuntimeError::TypeError {
                        message: format!(
                            "internal: ran off the end of `{}` — proto has no terminating RET",
                            proto.label()
                        ),
                        span: proto.span_at(pc.saturating_sub(1) as u32),
                    });
                }
                let ins = code[pc];
                pc += 1;
                let here = (pc - 1) as u32;

                let Some(op) = ins.op() else {
                    return Err(RuntimeError::TypeError {
                        message: format!("internal: unknown opcode {:#04x}", ins.raw_op()),
                        span: proto.span_at(here),
                    });
                };

                let a = ins.a() as usize;

                match op {
                    // ---- §15.1 moves and constants -----------------------
                    Op::MOVE => {
                        self.stack[base + a] = self.stack[base + ins.b() as usize].clone();
                    }
                    Op::LOADK => {
                        self.stack[base + a] = self.chunk.constants[ins.bx() as usize].clone();
                    }
                    Op::LOADI => self.stack[base + a] = Value::Int(ins.sbx() as i64),
                    Op::LOADF => self.stack[base + a] = Value::Float(ins.sbx() as f64),
                    Op::LOADBOOL => self.stack[base + a] = Value::Bool(ins.b() != 0),
                    Op::LOADNIL => {
                        for i in 0..=ins.b() as usize {
                            self.stack[base + a + i] = Value::Nil;
                        }
                    }
                    Op::EXTRAARG => {
                        return Err(RuntimeError::TypeError {
                            message: "internal: EXTRAARG executed as an instruction".into(),
                            span: proto.span_at(here),
                        });
                    }

                    // ---- §15.2 upvalues, module slots, closures ----------
                    Op::GETUPVAL => {
                        let cell = &closure.upvals[ins.b() as usize];
                        let v = match &*cell.borrow() {
                            Upvalue::Open(i) => self.stack[*i as usize].clone(),
                            Upvalue::Closed(v) => v.clone(),
                        };
                        self.stack[base + a] = v;
                    }
                    Op::SETUPVAL => {
                        let v = self.stack[base + a].clone();
                        let cell = Rc::clone(&closure.upvals[ins.b() as usize]);
                        let target = cell.borrow().stack_index();
                        match target {
                            Some(i) => self.stack[i as usize] = v,
                            None => *cell.borrow_mut() = Upvalue::Closed(v),
                        }
                    }
                    Op::CLOSEUP => self.close_upvalues((base + a) as u32),
                    Op::GETMOD => self.stack[base + a] = self.modules[ins.bx() as usize].clone(),
                    Op::SETMOD => {
                        self.modules[ins.bx() as usize] = self.stack[base + a].clone();
                    }
                    Op::CLOSURE => {
                        let child_idx = proto.protos[ins.bx() as usize];
                        let child = Rc::clone(self.chunk.proto(child_idx));
                        let mut upvals = Vec::with_capacity(child.upvals.len());
                        for desc in &child.upvals {
                            upvals.push(if desc.from_parent_stack {
                                self.capture_upvalue((base + desc.index as usize) as u32)
                            } else {
                                Rc::clone(&closure.upvals[desc.index as usize])
                            });
                        }
                        let cl = VmFunctionRef::new(Closure { proto: child, upvals });
                        self.stack[base + a] = Value::VmFunction(cl);
                    }

                    // ---- §15.3 integer arithmetic ------------------------
                    Op::ADDI | Op::SUBI | Op::MULI | Op::DIVI | Op::MODI | Op::POWI => {
                        let (l, r) = self.int_pair(base, ins, &proto, here)?;
                        let span = || proto.span_at(here);
                        let out = match op {
                            Op::ADDI => l.wrapping_add(r),
                            Op::SUBI => l.wrapping_sub(r),
                            Op::MULI => l.wrapping_mul(r),
                            Op::DIVI => {
                                if r == 0 {
                                    return Err(RuntimeError::DivisionByZero { span: span() });
                                }
                                l.wrapping_div(r)
                            }
                            Op::MODI => {
                                if r == 0 {
                                    return Err(RuntimeError::DivisionByZero { span: span() });
                                }
                                l.wrapping_rem(r)
                            }
                            _ => {
                                // `integer ^ integer` stays an integer, so a
                                // negative exponent has no answer — an error
                                // rather than a silent 0, matching `int_op`.
                                let Ok(exp) = u32::try_from(r) else {
                                    return Err(RuntimeError::TypeError {
                                        message: format!(
                                            "`^` on integers requires a non-negative exponent, \
                                             got {r} — use floats (`float(base) ^ {r}.0`) for a \
                                             fractional result"
                                        ),
                                        span: span(),
                                    });
                                };
                                l.wrapping_pow(exp)
                            }
                        };
                        self.stack[base + a] = Value::Int(out);
                    }
                    Op::NEGI => {
                        let v = self.int_at(base + ins.b() as usize, &proto, here)?;
                        self.stack[base + a] = Value::Int(v.wrapping_neg());
                    }
                    Op::ADDII | Op::SUBII | Op::MULII => {
                        let l = self.int_at(base + ins.b() as usize, &proto, here)?;
                        let imm = ins.sc();
                        let out = match op {
                            Op::ADDII => l.wrapping_add(imm),
                            Op::SUBII => l.wrapping_sub(imm),
                            _ => l.wrapping_mul(imm),
                        };
                        self.stack[base + a] = Value::Int(out);
                    }

                    // ---- §15.4 float arithmetic --------------------------
                    Op::ADDF | Op::SUBF | Op::MULF | Op::DIVF | Op::MODF | Op::POWF => {
                        let (l, r) = self.float_pair(base, ins, &proto, here)?;
                        let out = match op {
                            Op::ADDF => l + r,
                            Op::SUBF => l - r,
                            Op::MULF => l * r,
                            // Float division by zero yields infinity, matching
                            // `float_op` — only integer division errors.
                            Op::DIVF => l / r,
                            Op::MODF => l % r,
                            _ => l.powf(r),
                        };
                        self.stack[base + a] = Value::Float(out);
                    }
                    Op::NEGF => {
                        let v = self.float_at(base + ins.b() as usize, &proto, here)?;
                        self.stack[base + a] = Value::Float(-v);
                    }

                    // ---- §15.5 bitwise -----------------------------------
                    Op::BAND | Op::BOR | Op::BXOR | Op::SHL | Op::SHR => {
                        let (l, r) = self.int_pair(base, ins, &proto, here)?;
                        let out = match op {
                            Op::BAND => l & r,
                            Op::BOR => l | r,
                            Op::BXOR => l ^ r,
                            Op::SHL => shift(l, r),
                            // A right shift is a left shift by the negated
                            // count; `wrapping_neg` leaves `i64::MIN` alone,
                            // which `shift` already reads as "all bits out".
                            _ => shift(l, r.wrapping_neg()),
                        };
                        self.stack[base + a] = Value::Int(out);
                    }
                    Op::BNOT => {
                        let v = self.int_at(base + ins.b() as usize, &proto, here)?;
                        self.stack[base + a] = Value::Int(!v);
                    }

                    // ---- §15.6 dynamic arithmetic fallback ---------------
                    //
                    // The escape hatch that makes an incomplete type table
                    // safe. Where the front end proved nothing, this runs
                    // `ops::binary` — the tree-walker's own operator logic,
                    // reused rather than reimplemented, so `Op*` overloads,
                    // string coercion and every error message are identical
                    // by construction instead of by care.
                    Op::ARITHX => {
                        let code_v = self.extra_arg(code, &mut pc, &proto, here)?;
                        let Some(op) = crate::op::dynop::decode_binary(code_v) else {
                            return Err(RuntimeError::TypeError {
                                message: format!("internal: ARITHX with unknown operator {code_v}"),
                                span: proto.span_at(here),
                            });
                        };
                        let l = self.stack[base + ins.b() as usize].clone();
                        let r = self.stack[base + ins.c() as usize].clone();
                        let v = saule_interpreter::eval::ops::binary(
                            op,
                            l,
                            r,
                            proto.span_at(here),
                        )?;
                        self.stack[base + a] = v;
                    }
                    Op::UNARYX => {
                        let code_v = self.extra_arg(code, &mut pc, &proto, here)?;
                        let Some(op) = crate::op::dynop::decode_unary(code_v) else {
                            return Err(RuntimeError::TypeError {
                                message: format!("internal: UNARYX with unknown operator {code_v}"),
                                span: proto.span_at(here),
                            });
                        };
                        let v = self.stack[base + ins.b() as usize].clone();
                        self.stack[base + a] =
                            saule_interpreter::eval::ops::unary(op, v, proto.span_at(here))?;
                    }

                    // ---- §15.7 comparison and branching ------------------
                    Op::JMP => {
                        if a > 0 {
                            self.close_upvalues((base + a - 1) as u32);
                        }
                        pc = jump(pc, ins.sbx());
                    }
                    Op::JLTI | Op::JLEI | Op::JGTI | Op::JGEI | Op::JEQI | Op::JNEI => {
                        let l = self.int_at(base + a, &proto, here)?;
                        let r = self.int_at(base + ins.b() as usize, &proto, here)?;
                        let take = match op {
                            Op::JLTI => l < r,
                            Op::JLEI => l <= r,
                            Op::JGTI => l > r,
                            Op::JGEI => l >= r,
                            Op::JEQI => l == r,
                            _ => l != r,
                        };
                        // "Skip the next instruction" — by convention that
                        // next instruction is the JMP to the false branch.
                        if take {
                            pc += 1;
                        }
                    }
                    Op::JLTF | Op::JLEF | Op::JGTF | Op::JGEF => {
                        let l = self.float_at(base + a, &proto, here)?;
                        let r = self.float_at(base + ins.b() as usize, &proto, here)?;
                        let take = match op {
                            Op::JLTF => l < r,
                            Op::JLEF => l <= r,
                            Op::JGTF => l > r,
                            _ => l >= r,
                        };
                        if take {
                            pc += 1;
                        }
                    }
                    Op::JEQ | Op::JNE => {
                        let eq = self.stack[base + a] == self.stack[base + ins.b() as usize];
                        if eq == (op == Op::JEQ) {
                            pc += 1;
                        }
                    }
                    Op::JEQK => {
                        let eq = self.stack[base + a] == self.chunk.constants[ins.c() as usize];
                        if eq {
                            pc += 1;
                        }
                    }
                    Op::TEST => {
                        if self.stack[base + a].is_truthy() != (ins.c() != 0) {
                            pc += 1;
                        }
                    }
                    Op::TESTSET => {
                        let src = self.stack[base + ins.b() as usize].clone();
                        if src.is_truthy() == (ins.c() != 0) {
                            self.stack[base + a] = src;
                            pc += 1;
                        }
                    }
                    Op::JNIL | Op::JNOTNIL => {
                        let is_nil = matches!(self.stack[base + a], Value::Nil);
                        if is_nil == (op == Op::JNIL) {
                            pc += 1;
                        }
                    }
                    Op::LTI | Op::LEI | Op::EQI => {
                        let (l, r) = self.int_pair(base, ins, &proto, here)?;
                        self.stack[base + a] = Value::Bool(match op {
                            Op::LTI => l < r,
                            Op::LEI => l <= r,
                            _ => l == r,
                        });
                    }
                    Op::LTF | Op::LEF | Op::EQF => {
                        let (l, r) = self.float_pair(base, ins, &proto, here)?;
                        self.stack[base + a] = Value::Bool(match op {
                            Op::LTF => l < r,
                            Op::LEF => l <= r,
                            _ => l == r,
                        });
                    }
                    Op::EQV => {
                        let eq = self.stack[base + ins.b() as usize]
                            == self.stack[base + ins.c() as usize];
                        self.stack[base + a] = Value::Bool(eq);
                    }
                    Op::NOT => {
                        let t = self.stack[base + ins.b() as usize].is_truthy();
                        self.stack[base + a] = Value::Bool(!t);
                    }

                    // ---- §15.8 numeric loops ------------------------------
                    Op::FORPREP_I => {
                        let from = self.int_at(base + a, &proto, here)?;
                        let limit = self.int_at(base + a + 1, &proto, here)?;
                        let step = self.int_at(base + a + 2, &proto, here)?;
                        if step == 0 {
                            return Err(RuntimeError::ZeroStep { span: proto.span_at(here) });
                        }
                        if int_in_range(from, limit, step) {
                            self.stack[base + a + 3] = Value::Int(from);
                        } else {
                            pc = jump(pc, ins.sbx());
                        }
                    }
                    Op::FORLOOP_I => {
                        let i = self.int_at(base + a, &proto, here)?;
                        let limit = self.int_at(base + a + 1, &proto, here)?;
                        let step = self.int_at(base + a + 2, &proto, here)?;
                        // Detect overflow so a too-large step cannot loop
                        // forever — the guard `run_numeric_loop_int` has.
                        let (next, overflow) = i.overflowing_add(step);
                        if !overflow && int_in_range(next, limit, step) {
                            self.stack[base + a] = Value::Int(next);
                            self.stack[base + a + 3] = Value::Int(next);
                            pc = jump(pc, ins.sbx());
                        }
                    }
                    Op::FORPREP_F => {
                        let from = self.float_at(base + a, &proto, here)?;
                        let limit = self.float_at(base + a + 1, &proto, here)?;
                        let step = self.float_at(base + a + 2, &proto, here)?;
                        if step == 0.0 {
                            return Err(RuntimeError::ZeroStep { span: proto.span_at(here) });
                        }
                        if float_in_range(from, limit, step) {
                            self.stack[base + a + 3] = Value::Float(from);
                        } else {
                            pc = jump(pc, ins.sbx());
                        }
                    }
                    Op::FORLOOP_F => {
                        let i = self.float_at(base + a, &proto, here)?;
                        let limit = self.float_at(base + a + 1, &proto, here)?;
                        let step = self.float_at(base + a + 2, &proto, here)?;
                        let next = i + step;
                        if float_in_range(next, limit, step) {
                            self.stack[base + a] = Value::Float(next);
                            self.stack[base + a + 3] = Value::Float(next);
                            pc = jump(pc, ins.sbx());
                        }
                    }

                    // ---- §15.8 generic iteration -------------------------
                    Op::ITERPREP => {
                        // The snapshot is the observable part (§11.2): the
                        // tree-walker copies the array and a *sorted* list of
                        // map entries before iterating, so a table mutated
                        // inside the loop does not change what the loop sees.
                        // Reproducing that exactly is why this materialises
                        // the pairs up front rather than holding a cursor.
                        let pairs = match &self.stack[base + a] {
                            Value::Table(t) => snapshot_pairs(&t.borrow()),
                            other => {
                                return Err(RuntimeError::TypeError {
                                    message: format!(
                                        "cannot iterate a `{}` — `for … in` needs a table",
                                        other.type_name()
                                    ),
                                    span: proto.span_at(here),
                                });
                            }
                        };
                        let empty = pairs.is_empty();
                        self.ensure_stack(base + a + 5);
                        self.stack[base + a] = Value::Table(Rc::new(RefCell::new(
                            TableObject::from_array(pairs),
                        )));
                        self.stack[base + a + 1] = Value::Int(0);
                        if empty {
                            pc = jump(pc, ins.bx() as i32);
                        }
                    }
                    Op::ITERNEXT => {
                        let i = match &self.stack[base + a + 1] {
                            Value::Int(n) => *n as usize,
                            _ => 0,
                        };
                        let (k, v, more) = match &self.stack[base + a] {
                            Value::Table(t) => {
                                let t = t.borrow();
                                if i * 2 + 1 < t.array.len() {
                                    (
                                        t.array[i * 2].clone(),
                                        t.array[i * 2 + 1].clone(),
                                        true,
                                    )
                                } else {
                                    (Value::Nil, Value::Nil, false)
                                }
                            }
                            _ => (Value::Nil, Value::Nil, false),
                        };
                        if more {
                            self.stack[base + a + 1] = Value::Int(i as i64 + 1);
                            self.stack[base + a + 3] = k;
                            self.stack[base + a + 4] = v;
                            pc = jump(pc, ins.sbx());
                        }
                    }

                    // ---- §15.9 tables -------------------------------------
                    Op::NEWT => {
                        let mut t = TableObject::new();
                        t.array.reserve(ins.b() as usize);
                        t.map.reserve(ins.c() as usize);
                        self.stack[base + a] = Value::Table(Rc::new(RefCell::new(t)));
                    }
                    Op::SETLIST => {
                        let t = self.table_at(base + a, &proto, here)?;
                        let n = ins.b() as usize;
                        let mut t = t.borrow_mut();
                        t.array.reserve(n);
                        for i in 1..=n {
                            t.array.push(self.stack[base + a + i].clone());
                        }
                    }
                    Op::GETARR => {
                        let t = self.table_at(base + ins.b() as usize, &proto, here)?;
                        let idx = self.int_at(base + ins.c() as usize, &proto, here)?;
                        let v = {
                            let t = t.borrow();
                            index_array(&t, idx)
                        };
                        self.stack[base + a] = v;
                    }
                    Op::SETARR => {
                        let t = self.table_at(base + a, &proto, here)?;
                        let idx = self.int_at(base + ins.b() as usize, &proto, here)?;
                        let v = self.stack[base + ins.c() as usize].clone();
                        let mut t = t.borrow_mut();
                        let n = t.array.len() as i64;
                        if idx >= 1 && idx <= n {
                            t.array[(idx - 1) as usize] = v;
                        } else if idx == n + 1 {
                            t.array.push(v);
                        } else {
                            t.set(&Value::Int(idx), v).map_err(|m| RuntimeError::TypeError {
                                message: m,
                                span: proto.span_at(here),
                            })?;
                        }
                    }
                    Op::GETMAP | Op::GETMAPK | Op::GETIDX => {
                        let t = self.table_at(base + ins.b() as usize, &proto, here)?;
                        let key = if op == Op::GETMAPK {
                            self.chunk.constants[ins.c() as usize].clone()
                        } else {
                            self.stack[base + ins.c() as usize].clone()
                        };
                        let v = t.borrow().get(&key);
                        self.stack[base + a] = v;
                    }
                    Op::SETMAP | Op::SETMAPK | Op::SETIDX => {
                        let t = self.table_at(base + a, &proto, here)?;
                        let key = if op == Op::SETMAPK {
                            self.chunk.constants[ins.b() as usize].clone()
                        } else {
                            self.stack[base + ins.b() as usize].clone()
                        };
                        let v = self.stack[base + ins.c() as usize].clone();
                        t.borrow_mut().set(&key, v).map_err(|m| RuntimeError::TypeError {
                            message: m,
                            span: proto.span_at(here),
                        })?;
                    }
                    Op::APPEND => {
                        let t = self.table_at(base + a, &proto, here)?;
                        let v = self.stack[base + ins.b() as usize].clone();
                        t.borrow_mut().array.push(v);
                    }
                    Op::LEN => {
                        let v = match &self.stack[base + ins.b() as usize] {
                            Value::Table(t) => Value::Int(t.borrow().array_len() as i64),
                            Value::Str(s) => Value::Int(s.chars().count() as i64),
                            // Anything else is `ops::unary`'s to answer, not
                            // ours. Reimplementing the message here made the
                            // engines report `#` on an integer differently —
                            // caught by `SAULE_DIFF=1 ./run_tests.sh`, which
                            // compares diagnostics as well as values.
                            other => saule_interpreter::eval::ops::unary(
                                saule_ast::UnaryOp::Len,
                                other.clone(),
                                proto.span_at(here),
                            )?,
                        };
                        self.stack[base + a] = v;
                    }

                    // ---- §15.14 strings -----------------------------------
                    Op::CONCAT => {
                        // n-ary and single-allocation: `a .. b .. c` costs one
                        // `String`, where the tree-walker's left-folded `..`
                        // costs n-1.
                        let (from, to) = (base + ins.b() as usize, base + ins.c() as usize);
                        let len: usize = (from..=to)
                            .map(|i| self.stack[i].to_display_string().len())
                            .sum();
                        let mut s = String::with_capacity(len);
                        for i in from..=to {
                            s.push_str(&self.stack[i].to_display_string());
                        }
                        self.stack[base + a] = Value::Str(Rc::new(s));
                    }
                    Op::TOSTR => {
                        let s = self.stack[base + ins.b() as usize].to_display_string();
                        self.stack[base + a] = Value::Str(Rc::new(s));
                    }

                    // ---- §15.12 nullability -------------------------------
                    Op::COALESCE => {
                        let v = match &self.stack[base + ins.b() as usize] {
                            Value::Nil => self.stack[base + ins.c() as usize].clone(),
                            v => v.clone(),
                        };
                        self.stack[base + a] = v;
                    }
                    Op::UNWRAPNIL => {
                        let v = self.stack[base + ins.b() as usize].clone();
                        if matches!(v, Value::Nil) {
                            return Err(RuntimeError::ForceUnwrapNil { span: proto.span_at(here) });
                        }
                        self.stack[base + a] = v;
                    }
                    // `x as T`. The test is the tree-walker's own — deep for
                    // `table<T>`, subclass-aware for classes — because it
                    // *is* the tree-walker's function, not a copy of it.
                    // Never throws: a failed cast is `nil`, and the static
                    // type is `T?`, so the caller already has to handle it.
                    Op::CASTCHK => {
                        let v = self.stack[base + ins.b() as usize].clone();
                        let ok = self
                            .chunk
                            .cast_types
                            .get(ins.c() as usize)
                            .is_some_and(|t| {
                                saule_interpreter::eval::expr::cast::cast(&v, t)
                            });
                        self.stack[base + a] = if ok { v } else { Value::Nil };
                    }

                    // ---- §15.13 calls and returns -------------------------
                    Op::CALL => {
                        let callee_abs = base + a;
                        let n_args = self.arg_count(ins.b(), callee_abs + 1);
                        let n_ret = if ins.c() == 0 { ALL_RESULTS } else { ins.c() - 1 };
                        let span = proto.span_at(here);
                        if self.dispatch_call(callee_abs, n_args, n_ret, span, pc)? {
                            continue 'reentry;
                        }
                    }
                    Op::CALLK => {
                        let target = self.extra_arg(code, &mut pc, &proto, here)?;
                        let n_args = self.arg_count(ins.b(), base + a);
                        let n_ret = if ins.c() == 0 { ALL_RESULTS } else { ins.c() - 1 };
                        let p = Rc::clone(self.chunk.proto(target));
                        let handle = self.closure_for(target, p);
                        self.frames.last_mut().expect("frame").pc = pc as u32;
                        let dst = (base + a) as u32;
                        self.push_frame(handle, dst, n_args, dst, n_ret, proto.span_at(here))?;
                        continue 'reentry;
                    }
                    Op::CALLNAT => {
                        let k = self.extra_arg(code, &mut pc, &proto, here)?;
                        let callee = self.chunk.constants[k as usize].clone();
                        let n_args = self.arg_count(ins.b(), base + a + 1);
                        let n_ret = if ins.c() == 0 { ALL_RESULTS } else { ins.c() - 1 };
                        let span = proto.span_at(here);
                        self.call_native(&callee, base + a, n_args, n_ret, span)?;
                    }
                    Op::RET0 => {
                        if let Some(vs) = self.pop_frame(base, 0)? {
                            return Ok(vs);
                        }
                        if self.frames.len() < entry_depth {
                            return Ok(Vec::new());
                        }
                        continue 'reentry;
                    }
                    Op::RET1 => {
                        if let Some(vs) = self.pop_frame(base + a, 1)? {
                            return Ok(vs);
                        }
                        if self.frames.len() < entry_depth {
                            return Ok(Vec::new());
                        }
                        continue 'reentry;
                    }
                    Op::RET => {
                        let n = self.arg_count(ins.b(), base + a);
                        if let Some(vs) = self.pop_frame(base + a, n)? {
                            return Ok(vs);
                        }
                        if self.frames.len() < entry_depth {
                            return Ok(Vec::new());
                        }
                        continue 'reentry;
                    }

                    // ---- §15.10 classes and instances --------------------
                    Op::NEW => {
                        let idx = ins.bx() as usize;
                        let Some(class) = self.classes.get(idx) else {
                            return Err(RuntimeError::TypeError {
                                message: format!("internal: no class {idx} in this chunk"),
                                span: proto.span_at(here),
                            });
                        };
                        // One allocation of a `Vec<Value>` sized from the
                        // layout — replacing the per-field `String` clone
                        // plus hash insert the tree-walker pays (§8.6).
                        let inst = saule_interpreter::value::InstanceObject::new(Rc::clone(class));
                        self.stack[base + a] = Value::Instance(Rc::new(RefCell::new(inst)));
                    }
                    Op::GETF => {
                        let slot = ins.c() as usize;
                        let v = match &self.stack[base + ins.b() as usize] {
                            Value::Instance(i) => {
                                let i = i.borrow();
                                match i.fields.get(slot) {
                                    Some(v) => v.clone(),
                                    None => {
                                        return Err(field_slot_err(
                                            slot, i.fields.len(), &proto, here,
                                        ));
                                    }
                                }
                            }
                            other => return Err(operand_err(other, "instance", &proto, here)),
                        };
                        self.stack[base + a] = v;
                    }
                    Op::SETF => {
                        let slot = ins.b() as usize;
                        let v = self.stack[base + ins.c() as usize].clone();
                        match &self.stack[base + a] {
                            Value::Instance(i) => {
                                let mut i = i.borrow_mut();
                                let n = i.fields.len();
                                match i.fields.get_mut(slot) {
                                    Some(dst) => *dst = v,
                                    None => return Err(field_slot_err(slot, n, &proto, here)),
                                }
                            }
                            other => return Err(operand_err(other, "instance", &proto, here)),
                        }
                    }
                    Op::ISA => {
                        let want = ins.c() as u32;
                        let yes = match &self.stack[base + ins.b() as usize] {
                            Value::Instance(i) => {
                                self.is_a(&i.borrow().class, want)
                            }
                            _ => false,
                        };
                        self.stack[base + a] = Value::Bool(yes);
                    }
                    Op::GETSTAT => {
                        let (cls, slot) = (ins.b() as usize, ins.c() as usize);
                        let v = self
                            .statics
                            .get(cls)
                            .and_then(|s| s.borrow().get(slot).cloned());
                        match v {
                            Some(v) => self.stack[base + a] = v,
                            None => {
                                return Err(RuntimeError::TypeError {
                                    message: format!("internal: no static {slot} on class {cls}"),
                                    span: proto.span_at(here),
                                });
                            }
                        }
                    }
                    Op::SETSTAT => {
                        let (cls, slot) = (ins.b() as usize, ins.c() as usize);
                        let v = self.stack[base + a].clone();
                        match self.statics.get(cls).map(|s| {
                            let mut s = s.borrow_mut();
                            match s.get_mut(slot) {
                                Some(d) => {
                                    *d = v;
                                    true
                                }
                                None => false,
                            }
                        }) {
                            Some(true) => {}
                            _ => {
                                return Err(RuntimeError::TypeError {
                                    message: format!("internal: no static {slot} on class {cls}"),
                                    span: proto.span_at(here),
                                });
                            }
                        }
                    }
                    Op::CALLM => {
                        // The receiver is `R[A]` and becomes the callee's
                        // `R[0]` — `self` is simply parameter 0 (§6.2), so
                        // the argument window needs no shuffling.
                        let recv = base + a;
                        let slot = ins.c() as usize;
                        let n_args = (ins.b() as usize).saturating_sub(1);
                        let target = self.vtable_lookup(recv, slot, &proto, here)?;
                        let p = Rc::clone(self.chunk.proto(target));
                        let handle = self.closure_for(target, p);
                        self.frames.last_mut().expect("frame").pc = pc as u32;
                        // `self` counts as an argument: the frame starts at
                        // the receiver, not past it.
                        self.push_frame(
                            handle,
                            recv as u32,
                            n_args + 1,
                            recv as u32,
                            1,
                            proto.span_at(here),
                        )?;
                        continue 'reentry;
                    }
                    Op::CALLIF => {
                        // Interface dispatch: the receiver's class, one
                        // small-map probe into its itable, one vtable index
                        // (§8.4). The interface index rides in `EXTRAARG`
                        // because `C` is already the interface's method slot.
                        let iface = self.extra_arg(code, &mut pc, &proto, here)?;
                        let recv = base + a;
                        let slot = ins.c() as usize;
                        let n_args = (ins.b() as usize).saturating_sub(1);
                        let target = self.itable_lookup(recv, iface, slot, &proto, here)?;
                        let p = Rc::clone(self.chunk.proto(target));
                        let handle = self.closure_for(target, p);
                        self.frames.last_mut().expect("frame").pc = pc as u32;
                        self.push_frame(
                            handle,
                            recv as u32,
                            n_args + 1,
                            recv as u32,
                            1,
                            proto.span_at(here),
                        )?;
                        continue 'reentry;
                    }
                    Op::CALLSTAT => {
                        let packed = self.extra_arg(code, &mut pc, &proto, here)?;
                        let (cls, slot) = ((packed >> 16) as usize, (packed & 0xffff) as usize);
                        let target = match self
                            .chunk
                            .classes
                            .get(cls)
                            .and_then(|c| c.static_methods.get(slot))
                        {
                            Some(t) => *t,
                            None => {
                                return Err(RuntimeError::TypeError {
                                    message: format!(
                                        "internal: no static method {slot} on class {cls}"
                                    ),
                                    span: proto.span_at(here),
                                });
                            }
                        };
                        let n_args = (ins.b() as usize).saturating_sub(1);
                        let p = Rc::clone(self.chunk.proto(target));
                        let handle = self.closure_for(target, p);
                        self.frames.last_mut().expect("frame").pc = pc as u32;
                        let dst = (base + a) as u32;
                        let n_ret = if ins.c() == 0 { ALL_RESULTS } else { ins.c() - 1 };
                        self.push_frame(handle, dst, n_args, dst, n_ret, proto.span_at(here))?;
                        continue 'reentry;
                    }

                    // ---- §15.11 enums and `match` ------------------------
                    Op::VARIANT => {
                        let (e_idx, tag) = self.chunk.variant_refs[ins.bx() as usize];
                        let v = self
                            .enums
                            .get(e_idx as usize)
                            .and_then(|e| e.variant_by_tag(tag).cloned());
                        match v {
                            Some(v) => self.stack[base + a] = Value::EnumVariant(v),
                            None => {
                                return Err(RuntimeError::TypeError {
                                    message: format!(
                                        "internal: no singleton for enum {e_idx} tag {tag}"
                                    ),
                                    span: proto.span_at(here),
                                });
                            }
                        }
                    }
                    Op::NEWVAR => {
                        let packed = self.extra_arg(code, &mut pc, &proto, here)?;
                        let (e_idx, tag) = ((packed >> 16) as usize, packed & 0xffff);
                        let n = (ins.b() as usize).saturating_sub(1);
                        // The payload is an array-style table of the
                        // positional arguments, matching what the
                        // tree-walker's tuple-variant constructor builds —
                        // pattern destructuring reads it positionally.
                        let items: Vec<Value> =
                            (0..n).map(|i| self.stack[base + a + 1 + i].clone()).collect();
                        let payload = Value::Table(Rc::new(RefCell::new(
                            TableObject::from_array(items),
                        )));
                        let Some(e) = self.enums.get(e_idx) else {
                            return Err(RuntimeError::TypeError {
                                message: format!("internal: no enum {e_idx}"),
                                span: proto.span_at(here),
                            });
                        };
                        let name = self.chunk.enums[e_idx].variants[tag as usize]
                            .name
                            .to_string();
                        let v = saule_interpreter::value::EnumVariantObject {
                            enum_name: e.name.clone(),
                            variant_name: name,
                            tag,
                            value: Some(payload),
                            enum_obj: RefCell::new(Some(Rc::clone(e))),
                        };
                        self.stack[base + a] = Value::EnumVariant(Rc::new(v));
                    }
                    Op::GETTAG => {
                        let t = match &self.stack[base + ins.b() as usize] {
                            Value::EnumVariant(v) => v.tag as i64,
                            other => return Err(operand_err(other, "enum", &proto, here)),
                        };
                        self.stack[base + a] = Value::Int(t);
                    }
                    Op::SWITCH => {
                        let table = &self.chunk.jump_tables[ins.bx() as usize];
                        let tag = match &self.stack[base + a] {
                            Value::Int(n) => *n,
                            other => return Err(operand_err(other, "integer", &proto, here)),
                        };
                        // One indexed jump regardless of arm count, where the
                        // tree-walker compares enum and variant *names* once
                        // per arm (§9.2).
                        pc = usize::try_from(tag)
                            .ok()
                            .and_then(|i| table.targets.get(i).copied())
                            .unwrap_or(table.default) as usize;
                    }
                    Op::JIFTAG => {
                        let t = match &self.stack[base + a] {
                            Value::EnumVariant(v) => v.tag,
                            _ => u32::MAX,
                        };
                        if t == ins.b() as u32 {
                            pc += 1;
                        }
                    }
                    Op::UNWRAP => {
                        let v = match &self.stack[base + ins.b() as usize] {
                            Value::EnumVariant(v) => v.value.clone().unwrap_or(Value::Nil),
                            other => return Err(operand_err(other, "enum", &proto, here)),
                        };
                        self.stack[base + a] = v;
                    }

                    // ---- §15.15 errors -----------------------------------
                    Op::THROW => {
                        let v = self.stack[base + a].clone();
                        self.frames.last_mut().expect("frame").pc = pc as u32;
                        match self.unwind(v, &proto, here)? {
                            // A handler took it: re-enter with the frame and
                            // pc the handler restored.
                            () => continue 'reentry,
                        }
                    }
                    Op::CHKTY => {
                        let ok = self.type_matches(base + ins.b() as usize, ins.c() as u32);
                        self.stack[base + a] = Value::Bool(ok);
                    }

                    // ---- not yet implemented ------------------------------
                    // Classes, enums/`match`, `try`/`catch`, `for … in`, and
                    // the dynamic arithmetic fallback are Phase 3 (§21.4).
                    other => {
                        return Err(RuntimeError::Unsupported {
                            thing: other.name(),
                            span: proto.span_at(here),
                        });
                    }
                }
            }
        }
    }

    /// Resolve a vtable slot against the class of the instance in `recv`.
    fn vtable_lookup(
        &self,
        recv: usize,
        slot: usize,
        proto: &Proto,
        here: u32,
    ) -> Result<u32, RuntimeError> {
        let Value::Instance(inst) = &self.stack[recv] else {
            return Err(operand_err(&self.stack[recv], "instance", proto, here));
        };
        let class = Rc::clone(&inst.borrow().class);
        let idx = self
            .class_of
            .get(&(Rc::as_ptr(&class) as usize))
            .copied()
            .ok_or_else(|| RuntimeError::TypeError {
                message: format!(
                    "internal: `{}` was not built from this chunk",
                    class.name
                ),
                span: proto.span_at(here),
            })?;
        // The prefix invariant at work: a slot resolved against a parent's
        // vtable indexes the subclass's override, because a subclass's
        // vtable extends its parent's rather than reordering it (§8.3).
        self.chunk.classes[idx as usize]
            .vtable
            .get(slot)
            .copied()
            .filter(|t| *t != u32::MAX)
            .ok_or_else(|| RuntimeError::TypeError {
                message: format!(
                    "internal: `{}` has no method in vtable slot {slot}",
                    class.name
                ),
                span: proto.span_at(here),
            })
    }

    /// Resolve an interface method slot against the receiver's class.
    ///
    /// The itable was built once, when the class was laid out, so this is a
    /// map probe and two indexed loads — not a name lookup. §8.4 adds a
    /// one-entry inline cache on top; call sites on interface receivers are
    /// overwhelmingly monomorphic, so that collapses the probe to a pointer
    /// compare. That is Phase 5, with a benchmark.
    fn itable_lookup(
        &self,
        recv: usize,
        iface: u32,
        slot: usize,
        proto: &Proto,
        here: u32,
    ) -> Result<u32, RuntimeError> {
        let Value::Instance(inst) = &self.stack[recv] else {
            return Err(operand_err(&self.stack[recv], "instance", proto, here));
        };
        let class = Rc::clone(&inst.borrow().class);
        let idx = self
            .class_of
            .get(&(Rc::as_ptr(&class) as usize))
            .copied()
            .ok_or_else(|| RuntimeError::TypeError {
                message: format!("internal: `{}` was not built from this chunk", class.name),
                span: proto.span_at(here),
            })?;
        let cp = &self.chunk.classes[idx as usize];
        let vslot = cp
            .itables
            .get(&iface)
            .and_then(|t| t.get(slot))
            .copied()
            .ok_or_else(|| RuntimeError::TypeError {
                message: format!(
                    "`{}` does not implement the interface this call requires",
                    class.name
                ),
                span: proto.span_at(here),
            })?;
        cp.vtable
            .get(vslot as usize)
            .copied()
            .filter(|t| *t != u32::MAX)
            .ok_or_else(|| RuntimeError::TypeError {
                message: format!("internal: `{}` has no method in slot {vslot}", class.name),
                span: proto.span_at(here),
            })
    }

    /// Find a handler for `value`, unwinding frames until one matches.
    ///
    /// The happy path pays **nothing** for this: entering a `try` emits no
    /// instructions at all, and only a `throw` ever consults the table
    /// (§12.1). It also means the thrown value never enters a
    /// `RuntimeError` unless it escapes to the top, which is what makes the
    /// tree-walker's `thrown_slot` thread-local unnecessary here.
    fn unwind(
        &mut self,
        value: Value,
        proto: &Proto,
        here: u32,
    ) -> Result<(), RuntimeError> {
        loop {
            let Some(frame) = self.frames.last() else { break };
            let func = Rc::clone(&frame.func);
            let (base, pc) = (frame.base, frame.pc);
            let closure = Closure::from_handle(&func).expect("frame holds a Closure");
            // The saved pc points *past* the throwing instruction, so the
            // range test uses the instruction itself.
            let at = pc.saturating_sub(1);

            let found = closure
                .proto
                .handlers
                .iter()
                .find(|h| at >= h.pc_start && at < h.pc_end && self.value_matches(&value, h.catch_ty))
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

        Err(RuntimeError::Thrown {
            value: value.to_display_string(),
            span: proto.span_at(here),
        })
    }

    fn type_matches(&self, reg: usize, ty: u32) -> bool {
        let v = &self.stack[reg];
        self.value_matches(v, ty)
    }

    /// Does `v` satisfy the type descriptor `ty`?
    fn value_matches(&self, v: &Value, ty: u32) -> bool {
        use crate::chunk::TypeDesc;
        let Some(desc) = self.chunk.type_descs.get(ty as usize) else {
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
                Value::EnumVariant(ev) => self
                    .chunk
                    .enums
                    .get(*idx as usize)
                    .is_some_and(|e| e.name.as_ref() == ev.enum_name.as_str()),
                _ => false,
            },
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
    fn is_a(&self, class: &Rc<saule_interpreter::value::ClassObject>, want: u32) -> bool {
        let Some(target) = self.classes.get(want as usize) else {
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

    // ---- calls ---------------------------------------------------------

    /// Returns `true` when a new bytecode frame was pushed and the caller
    /// must re-enter the dispatch loop.
    fn dispatch_call(
        &mut self,
        callee_abs: usize,
        n_args: usize,
        n_ret: u8,
        span: std::ops::Range<usize>,
        pc_after: usize,
    ) -> Result<bool, RuntimeError> {
        let callee = self.stack[callee_abs].clone();
        match callee {
            Value::VmFunction(handle) => {
                self.frames.last_mut().expect("frame").pc = pc_after as u32;
                self.push_frame(
                    handle,
                    (callee_abs + 1) as u32,
                    n_args,
                    callee_abs as u32,
                    n_ret,
                    span,
                )?;
                Ok(true)
            }
            other => {
                self.call_native(&other, callee_abs, n_args, n_ret, span)?;
                Ok(false)
            }
        }
    }

    /// Call a native, a native closure, or fail with the same message shape
    /// the tree-walker produces. Arguments are passed as a **borrow of the
    /// register file** — no `Vec`, no per-argument clone (§13).
    fn call_native(
        &mut self,
        callee: &Value,
        dst: usize,
        n_args: usize,
        n_ret: u8,
        span: std::ops::Range<usize>,
    ) -> Result<(), RuntimeError> {
        let args_from = dst + 1;
        match callee {
            Value::Native(nf) => {
                let v = (nf.func)(&self.stack[args_from..args_from + n_args]).map_err(|m| {
                    RuntimeError::TypeError { message: m, span: span.clone() }
                })?;
                self.store_results(dst, std::slice::from_ref(&v), n_ret);
                Ok(())
            }
            Value::NativeClosure(nc) => {
                let vs = (nc.func)(&self.stack[args_from..args_from + n_args]).map_err(|m| {
                    RuntimeError::TypeError { message: m, span: span.clone() }
                })?;
                self.store_results(dst, &vs, n_ret);
                Ok(())
            }
            other => Err(RuntimeError::TypeError {
                message: format!("attempt to call a `{}`", other.type_name()),
                span,
            }),
        }
    }

    fn push_frame(
        &mut self,
        func: Rc<VmFunctionRef>,
        base: u32,
        n_args: usize,
        ret_to: u32,
        n_ret: u8,
        span: std::ops::Range<usize>,
    ) -> Result<(), RuntimeError> {
        if self.frames.len() >= self.max_frames {
            return Err(RuntimeError::StackOverflow {
                limit: self.max_frames as u32,
                span,
            });
        }
        let proto = Rc::clone(
            &Closure::from_handle(&func)
                .ok_or_else(|| RuntimeError::TypeError {
                    message: "internal: frame handle is not a bytecode closure".into(),
                    span: span.clone(),
                })?
                .proto,
        );
        let top = base as usize + proto.max_regs as usize;
        self.ensure_stack(top);
        // Missing parameters read as nil. Registers past `n_params` are the
        // compiler's temporaries and are always written before read, so they
        // are left alone rather than memset per call.
        for i in n_args..proto.n_params as usize {
            self.stack[base as usize + i] = Value::Nil;
        }
        let pc = proto.entry_for(n_args.min(u8::MAX as usize) as u8);
        self.frames.push(Frame { func, base, ret_to, n_ret, pc, top: top as u32 });
        Ok(())
    }

    /// Pop the active frame, moving `count` results from `src` into the
    /// caller. Returns `Some` when the outermost frame returned, in which
    /// case the values are the program's result.
    fn pop_frame(&mut self, src: usize, count: usize) -> Result<Option<Vec<Value>>, RuntimeError> {
        let frame = self.frames.pop().expect("frame");
        self.close_upvalues(frame.base);

        if self.frames.is_empty() {
            let mut out = Vec::with_capacity(count);
            for i in 0..count {
                out.push(std::mem::replace(&mut self.stack[src + i], Value::Nil));
            }
            return Ok(Some(out));
        }

        let dst = frame.ret_to as usize;
        // `dst < src` always — the callee's base is the caller's `A + 1` at
        // the lowest — so a forward move never clobbers an unread source.
        debug_assert!(dst <= src);
        let wanted = if frame.n_ret == ALL_RESULTS { count } else { frame.n_ret as usize };
        self.ensure_stack(dst + wanted);
        for i in 0..wanted {
            self.stack[dst + i] = if i < count {
                std::mem::replace(&mut self.stack[src + i], Value::Nil)
            } else {
                Value::Nil
            };
        }
        if frame.n_ret == ALL_RESULTS {
            self.frames.last_mut().expect("frame").top = (dst + count) as u32;
        }
        Ok(None)
    }

    fn store_results(&mut self, dst: usize, vals: &[Value], n_ret: u8) {
        let wanted = if n_ret == ALL_RESULTS { vals.len() } else { n_ret as usize };
        self.ensure_stack(dst + wanted);
        for i in 0..wanted {
            self.stack[dst + i] = vals.get(i).cloned().unwrap_or(Value::Nil);
        }
        if n_ret == ALL_RESULTS && let Some(f) = self.frames.last_mut() {
            f.top = (dst + vals.len()) as u32;
        }
    }

    /// `B = nargs + 1`; `B = 0` means "arguments run from `from` to `top`".
    fn arg_count(&self, b: u8, from: usize) -> usize {
        if b == 0 {
            (self.frames.last().map_or(from, |f| f.top as usize)).saturating_sub(from)
        } else {
            (b - 1) as usize
        }
    }

    /// Read the `EXTRAARG` word following the current instruction.
    fn extra_arg(
        &self,
        code: &[Instruction],
        pc: &mut usize,
        proto: &Proto,
        here: u32,
    ) -> Result<u32, RuntimeError> {
        let ins = code.get(*pc).copied().unwrap_or(Instruction(0));
        if ins.op() != Some(Op::EXTRAARG) {
            return Err(RuntimeError::TypeError {
                message: "internal: instruction requires a following EXTRAARG".into(),
                span: proto.span_at(here),
            });
        }
        *pc += 1;
        Ok(ins.ax())
    }

    /// A cached, upvalue-free closure for a statically-resolved callee, so
    /// `CALLK` allocates nothing per call.
    fn closure_for(&mut self, idx: u32, proto: Rc<Proto>) -> Rc<VmFunctionRef> {
        if let Some(Some(c)) = self.closure_cache.get(idx as usize) {
            return Rc::clone(c);
        }
        let c = VmFunctionRef::new(Closure::new(proto));
        if let Some(slot) = self.closure_cache.get_mut(idx as usize) {
            *slot = Some(Rc::clone(&c));
        }
        c
    }

    // ---- upvalues ------------------------------------------------------

    fn capture_upvalue(&mut self, index: u32) -> Rc<RefCell<Upvalue>> {
        match self
            .open_upvals
            .binary_search_by_key(&Some(index), |u| u.borrow().stack_index())
        {
            Ok(i) => Rc::clone(&self.open_upvals[i]),
            Err(i) => {
                let cell = Rc::new(RefCell::new(Upvalue::Open(index)));
                self.open_upvals.insert(i, Rc::clone(&cell));
                cell
            }
        }
    }

    /// Close every open upvalue pointing at a register >= `from`. The value
    /// **moves** out of the register into the cell.
    fn close_upvalues(&mut self, from: u32) {
        while let Some(last) = self.open_upvals.last() {
            let Some(idx) = last.borrow().stack_index() else {
                self.open_upvals.pop();
                continue;
            };
            if idx < from {
                break;
            }
            let cell = self.open_upvals.pop().expect("checked");
            let v = std::mem::replace(&mut self.stack[idx as usize], Value::Nil);
            *cell.borrow_mut() = Upvalue::Closed(v);
        }
    }

    // ---- register file -------------------------------------------------

    fn ensure_stack(&mut self, len: usize) {
        if self.stack.len() < len {
            self.stack.resize(len, Value::Nil);
        }
    }

    // ---- typed operand reads -------------------------------------------

    fn int_at(&self, i: usize, proto: &Proto, here: u32) -> Result<i64, RuntimeError> {
        match &self.stack[i] {
            Value::Int(n) => Ok(*n),
            other => Err(operand_err(other, "integer", proto, here)),
        }
    }

    fn float_at(&self, i: usize, proto: &Proto, here: u32) -> Result<f64, RuntimeError> {
        match &self.stack[i] {
            Value::Float(n) => Ok(*n),
            other => Err(operand_err(other, "float", proto, here)),
        }
    }

    fn table_at(
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

    fn int_pair(
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

    fn float_pair(
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

fn field_slot_err(slot: usize, have: usize, proto: &Proto, here: u32) -> RuntimeError {
    RuntimeError::TypeError {
        message: format!(
            "internal: field slot {slot} on an instance with {have} field(s) — \
             the chunk disagrees with the layout it was compiled against"
        ),
        span: proto.span_at(here),
    }
}

fn operand_err(got: &Value, want: &str, proto: &Proto, here: u32) -> RuntimeError {
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
fn jump(pc: usize, sbx: i32) -> usize {
    (pc as i64 + sbx as i64) as usize
}

#[inline]
fn int_in_range(i: i64, limit: i64, step: i64) -> bool {
    (step > 0 && i <= limit) || (step < 0 && i >= limit)
}

#[inline]
fn float_in_range(i: f64, limit: f64, step: f64) -> bool {
    (step > 0.0 && i <= limit) || (step < 0.0 && i >= limit)
}

/// Lua 5.3 shift semantics, as `ops::shift` implements them: zero-filled,
/// a negative count shifts the other way, and `|n| >= 64` is 0.
#[inline]
fn shift(a: i64, n: i64) -> i64 {
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
fn snapshot_pairs(t: &TableObject) -> Vec<Value> {
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

fn index_array(t: &TableObject, idx: i64) -> Value {
    if idx >= 1 && (idx as usize) <= t.array.len() {
        t.array[(idx - 1) as usize].clone()
    } else {
        t.get(&Value::Int(idx))
    }
}

fn max_frames_from_env() -> usize {
    std::env::var("SAULE_MAX_DEPTH")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_MAX_FRAMES)
}
