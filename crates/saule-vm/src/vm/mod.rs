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

//!
//! ## What lives where
//!
//! | file | holds |
//! |---|---|
//! | `mod.rs` | [`Vm`], [`VmShared`], and the ways in: `run`, `call`, `invoke` |
//! | [`dispatch`] | the interpreter loop — one function, deliberately whole |
//! | [`call`] | frames: pushing, popping, tail calls, natives, vtable lookup |
//! | [`unwind`] | throwing: finding a handler, and the type tests it applies |
//! | [`build`] | turning a chunk's class and enum protos into runtime objects |
//! | [`upval`] | capturing and closing upvalues |
//! | [`frame`] | the [`Frame`] record itself |
//! | [`ops`] | reading operands out of registers, and the small numeric helpers |
//!
//! **The dispatch loop is not to be broken up.** Its arms borrow loop-local
//! state — `pc`, `base`, the decoded `code` slice, the profiling pair
//! tracker — and it is monomorphised twice over `PROFILE`. The second copy
//! alone measured 2-3% on the call-heavy benchmarks through code layout
//! (see this crate's `Cargo.toml`), which is how sensitive this function is
//! to its own shape. Lifting an arm into a method changes inlining and
//! register pressure; if one has to move, measure it.

pub mod build;
pub mod call;
pub mod dispatch;
pub mod frame;
pub mod ops;
pub mod unwind;
pub mod upval;

use std::cell::RefCell;
use std::rc::Rc;

use saule_interpreter::value::VmFunctionRef;
use saule_interpreter::{RuntimeError, Value};

use crate::chunk::Chunk;

pub use frame::{ALL_RESULTS, Closure, Frame};
pub use upval::Upvalue;

use build::{build_classes, build_enums};
use ops::max_frames_from_env;

/// Frame-depth limit, deliberately **equal to the tree-walker's**
/// `MAX_EVAL_DEPTH`.
///
/// §6.4 argues this can be two orders of magnitude higher, and the argument
/// is sound: `MAX_EVAL_DEPTH = 10_000` exists because a Saule call is a
/// *native* stack frame in the tree-walker, where an overflow is a `SIGSEGV`
/// rather than a catchable error, while here a call is a `Vec` push and the
/// limit is pure policy.
///
/// It was set to `1_000_000` on that reasoning, and that made the engines
/// disagree: `depth(50_000)` returned `50000` under `--vm` and raised
/// `StackOverflow` without it. While the tree-walker is the default engine
/// and the VM is an opt-in accelerator behind a silent fallback, "works with
/// `--vm`, crashes without it" is precisely the surprise that fallback
/// exists to prevent — and a limit is observable behaviour, not an
/// implementation detail.
///
/// *Deviation from §6.4, argued rather than accidental:* the raise is
/// deferred to Phase 4, where flipping the default makes the VM the
/// definition of the language and the new limit an announced improvement
/// rather than a difference between two engines that are supposed to agree.
/// Pinned by `deep_recursion_hits_the_same_limit_under_both_engines`.
pub const DEFAULT_MAX_FRAMES: usize = saule_interpreter::eval::MAX_EVAL_DEPTH as usize;

/// Everything about a running program that is **not** per-invocation: the
/// code, the module slots, the statics, the classes and the enums (§5.1).
///
/// Split out from [`Vm`] so a callback can re-enter. A native that invokes
/// its argument — `Table.sort`'s comparator, an `OpAdd` overload, a
/// `toString` — is reached from inside the dispatch loop, which holds
/// `&mut self` across the `CALLNAT`. That borrow is what made a bytecode
/// closure uncallable from the tree-walker. Behind an `Rc`, a second `Vm`
/// can run over the same state with a register file of its own, and the
/// borrow never has to be handed out.
pub struct VmShared {
    /// Every module of the program, in post-order. Index 0 exists for a
    /// single-module compile too, so nothing has to special-case it.
    chunks: Vec<Rc<Chunk>>,
    /// Flat module slots — top-level bindings, no hashing. Behind a
    /// `RefCell` because a re-entrant call can both read and write them.
    modules: RefCell<Vec<Value>>,
    /// Lazily-built closures for protos that capture nothing, so `CALLK`
    /// does not allocate one per call. Indexed `[module][proto]`, because a
    /// proto index only means something within its own chunk.
    closure_cache: RefCell<Vec<Vec<Option<Rc<VmFunctionRef>>>>>,
    /// One runtime class per `ClassProto`, built once at start-up.
    classes: Vec<Rc<saule_interpreter::value::ClassObject>>,
    /// Class identity -> index, so `CALLM` can find a receiver's vtable.
    ///
    /// A hash probe per dynamic dispatch, which is what §8.5's inline cache
    /// exists to remove. Correct first; the cache is a Phase 5 line item
    /// with a benchmark attached. Until then the probe at least uses the
    /// same `FxHashMap` the interpreter's other hot maps do — the key is an
    /// `Rc::as_ptr`, so SipHash's resistance bought nothing and cost ~4% of
    /// the `oop` benchmark.
    class_of: saule_interpreter::fxhash::FxHashMap<usize, u32>,
    /// Static fields, flat per class — the `GETSTAT`/`SETSTAT` form. Kept
    /// beside the class rather than inside it because a static is a slot,
    /// not a named entry, once the compiler has resolved it.
    statics: Vec<RefCell<Vec<Value>>>,
    /// One runtime enum per `EnumProto`, built once at start-up.
    enums: Vec<Rc<saule_interpreter::value::EnumObject>>,
    max_frames: usize,
    /// Register files parked for reuse by the next re-entrant call.
    ///
    /// A callback needs a `Vm` of its own, and building one means two heap
    /// allocations — a 256-register stack and a frame list. On a sort
    /// comparator that is per *comparison*, which measured as `sort.sau`
    /// running slower under this engine than under the tree-walker even
    /// though the comparator body itself is faster.
    ///
    /// Nesting is a stack discipline, so a free list is all this needs: a
    /// `Vm` in use is simply not in the pool. Only a cleanly-returned `Vm`
    /// goes back — one unwound by an error still has frames and possibly
    /// open upvalues, and sorting that out is not worth an allocation.
    reentry_pool: RefCell<Vec<Vm>>,
}

impl std::fmt::Debug for VmShared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The chunk alone is enormous and a `Closure`'s `Debug` prints this
        // field; name it rather than dump it.
        f.debug_struct("VmShared")
            .field("modules", &self.chunks.len())
            .field("classes", &self.classes.len())
            .finish_non_exhaustive()
    }
}

/// The register machine: one invocation's stack and frames over an
/// [`Rc<VmShared>`](VmShared).
pub struct Vm {
    shared: Rc<VmShared>,
    /// One contiguous register file. Grown, never shrunk.
    stack: Vec<Value>,
    frames: Vec<Frame>,
    /// Open upvalues, sorted ascending by the stack index they point at, so
    /// `CLOSEUP` can pop from the back.
    open_upvals: Vec<Rc<RefCell<Upvalue>>>,
}

impl Vm {
    pub fn new(chunk: Rc<Chunk>) -> Vm {
        Vm::for_chunks(vec![chunk])
    }

    /// A VM over a whole program's modules, in post-order.
    ///
    /// One `VmShared` for the program rather than one per module, because
    /// the classes, the statics and the module slots are all program-wide:
    /// building them per module would give each module its *own*
    /// `Rc<ClassObject>` for the same class, and `class_of` — which maps
    /// class identity to a vtable — would then answer differently depending
    /// on which module asked.
    ///
    /// Every chunk shares the same class/enum tables, so reading them
    /// through `chunks[0]` is not a choice of module; it is the one table.
    pub fn for_chunks(chunks: Vec<Rc<Chunk>>) -> Vm {
        // One flat slot vector for the program: each module's slots were
        // rebased onto it at compile time, which is what lets an import be a
        // plain `GETMOD` + `SETMOD` with no cross-module opcode.
        let module_slots = chunks
            .iter()
            .map(|c| c.module_slot_base + c.module_slots)
            .max()
            .unwrap_or(0);
        let cache = chunks.iter().map(|c| vec![None; c.protos.len()]).collect();
        // `new_cyclic` because the classes built here carry method closures,
        // and a closure needs a `Weak<VmShared>` to be able to run itself —
        // which does not exist until the `Rc` does.
        let shared = Rc::new_cyclic(|weak: &std::rc::Weak<VmShared>| {
            let (classes, class_of, statics) = build_classes(&chunks, weak);
            VmShared {
                enums: build_enums(&chunks, weak),
                modules: RefCell::new(vec![Value::Nil; module_slots]),
                closure_cache: RefCell::new(cache),
                chunks,
                classes,
                class_of,
                statics,
                max_frames: max_frames_from_env(),
                reentry_pool: RefCell::new(Vec::new()),
            }
        });
        Vm::from_shared(shared)
    }

    /// A fresh register file over already-built engine state — the
    /// re-entrant entry point.
    pub fn from_shared(shared: Rc<VmShared>) -> Vm {
        Vm {
            shared,
            stack: Vec::with_capacity(256),
            frames: Vec::with_capacity(32),
            open_upvals: Vec::new(),
        }
    }
}

impl VmShared {
    /// A `Vm` for a re-entrant call — recycled if one is parked.
    fn take_vm(self: &Rc<Self>) -> Vm {
        match self.reentry_pool.borrow_mut().pop() {
            Some(vm) => vm,
            None => Vm::from_shared(Rc::clone(self)),
        }
    }

    /// Park a `Vm` whose call returned cleanly.
    ///
    /// The stack is cleared rather than left as it was: a stale register
    /// would keep every value the last callback touched alive for as long
    /// as the program runs. `clear` drops the values and keeps the
    /// capacity, which is the whole point of the pool.
    fn give_vm(&self, mut vm: Vm) {
        debug_assert!(vm.frames.is_empty(), "a clean return pops every frame");
        debug_assert!(
            vm.open_upvals.is_empty(),
            "popping the outermost frame closes every upvalue"
        );
        vm.stack.clear();
        // Bounded so a deeply nested program does not park a register file
        // per level for the rest of the run.
        if self.reentry_pool.borrow().len() < 8 {
            self.reentry_pool.borrow_mut().push(vm);
        }
    }
}

impl Vm {
    /// Execute the entry module's `main` proto.
    pub fn run(&mut self) -> Result<Vec<Value>, RuntimeError> {
        let last = self.shared.chunks.len() - 1;
        self.run_module(last)
    }

    /// Execute one module's top level.
    ///
    /// A program runs these in post-order — every module after the ones it
    /// imports — which is both what compilation requires and what the
    /// tree-walker does, since it runs an imported module's top level on
    /// first import. A module that prints at the top level makes that
    /// ordering observable, so the two engines have to agree about it.
    pub fn run_module(&mut self, module: usize) -> Result<Vec<Value>, RuntimeError> {
        let chunk = Rc::clone(&self.shared.chunks[module]);
        let main_idx = chunk.main;
        let handle = self.closure_for(&chunk, main_idx);
        // Start above whatever an earlier module left behind: module slots
        // are shared, but registers are not, and a later module must not
        // scribble on a frame an earlier one is still described by.
        let base = self.stack.len() as u32;
        self.push_frame(handle, base, 0, base, ALL_RESULTS, 0..0)?;
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
        self.invoke(&handle, args, 0..0)
    }

    /// Run `handle` over `args` on this VM's own register file.
    ///
    /// The body of both [`Vm::call`] and the re-entrant
    /// [`VmFunction::call`](saule_interpreter::value::VmFunction::call).
    ///
    /// **Guarded by the tree-walker's depth counter, not `max_frames`.**
    /// Each re-entrant call is a fresh `Vm` with `frames` of its own, so
    /// `max_frames` — which counts frames within *one* `Vm` — cannot see the
    /// nesting. What nesting actually consumes is the native stack, one Rust
    /// frame per level, which is exactly what `DepthGuard` bounds and what
    /// the tree-walker already counts. Sharing the counter also means a
    /// program bouncing between engines is bounded once rather than twice.
    pub(crate) fn invoke(
        &mut self,
        handle: &Rc<VmFunctionRef>,
        args: &[Value],
        span: std::ops::Range<usize>,
    ) -> Result<Vec<Value>, RuntimeError> {
        let _depth = saule_interpreter::enter_call_depth(&span)?;
        let base = self.stack.len() as u32;
        self.ensure_stack(base as usize + args.len());
        for (i, a) in args.iter().enumerate() {
            self.stack[base as usize + i] = a.clone();
        }
        self.push_frame(
            Rc::clone(handle),
            base,
            args.len(),
            base,
            ALL_RESULTS,
            span,
        )?;
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
        // The class table is program-wide, so the entry point is found by
        // name wherever it was declared — and loaded from *that* module's
        // chunk, since a proto index only means something within one.
        let table = Rc::clone(&self.shared.chunks[0].classes);
        let (module, proto_idx) = table.iter().find_map(|c| {
            // `s.class`, not `c`: `smindex` is flattened, so an entry may
            // name a parent — and `static_methods` is one vector per class.
            (c.name.as_ref() == class)
                .then(|| c.smindex.get(method))
                .flatten()
                .and_then(|s| {
                    let owner = &table[s.class as usize];
                    Some((owner.module, *owner.static_methods.get(s.slot as usize)?))
                })
        })?;
        if proto_idx == u32::MAX {
            return None;
        }
        let chunk = Rc::clone(&self.shared.chunks[module]);
        let handle = self.closure_for(&chunk, proto_idx);
        // Start above whatever the module body left behind, so its module
        // slots and any live registers are untouched.
        let base = self.stack.len() as u32;
        if let Err(e) = self.push_frame(handle, base, 0, base, ALL_RESULTS, 0..0) {
            return Some(Err(e));
        }
        Some(self.execute())
    }

}
