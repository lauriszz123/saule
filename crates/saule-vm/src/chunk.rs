//! The compiled form of a module: [`Chunk`], [`Proto`], and the class /
//! enum / handler tables hanging off them (`VM_DESIGN.md` §5.1, §8.1, §12.1).
//!
//! ## One rule governs the shape of everything here
//!
//! **A chunk stores indices, not runtime objects.** §14 wants a `Chunk` to be
//! serializable so compiled bytecode can be cached in `.saule/cache/`, and the
//! cheapest way to guarantee that is to never let a non-serializable thing in.
//! So where the design sketch writes `vtable: Vec<Rc<Closure>>`, this module
//! writes `vtable: Vec<ProtoIdx>` and builds the closures at load time; where
//! it writes `statics: Vec<Value>`, this writes a count plus an initializer
//! proto.
//!
//! The two deliberate exceptions are [`Chunk::constants`] (a `Vec<Value>`,
//! restricted by the compiler to the literal-shaped variants, which do
//! serialize) and [`Proto::caches`] (runtime scratch, rebuilt empty on load).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use saule_interpreter::Value;
// **The** field layout, not a copy of it. `saule-interpreter` owns the type
// and the runtime `ClassObject` holds the very same `Rc`, so the compiler and
// the runtime cannot disagree about which slot a field lives in — the failure
// §24.2 calls out as the worst this project could ship. A second definition
// here would reintroduce exactly that risk.
pub use saule_interpreter::value::FieldLayout;
use saule_interpreter::value::ClassObject;

use crate::op::Instruction;

pub type ProtoIdx = u32;
pub type ClassIdx = u32;
pub type EnumIdx = u32;
pub type ConstIdx = u32;
pub type InterfaceIdx = u32;
pub type TypeIdx = u32;
pub type JumpTableIdx = u32;

/// One compiled module.
#[derive(Debug)]
pub struct Chunk {
    /// Every function, method, and lambda in the module. `Rc` because a
    /// runtime [`Closure`](crate::vm::Closure) holds one and frames are hot.
    pub protos: Vec<Rc<Proto>>,
    /// The program's classes, enums and interfaces — **shared by every
    /// module of a program**, not owned by this chunk (`VM_DESIGN.md` §14,
    /// §24.2).
    ///
    /// A subclass in one module extending a parent in another needs the
    /// parent's real field slots and vtable numbering; computing them twice
    /// is exactly the divergence §24.2 names as the worst bug this project
    /// could ship. So `ClassIdx` is program-global and every module's chunk
    /// points at the same table.
    ///
    /// `Rc<Vec<_>>` rather than a plain `Vec` so the sharing costs one
    /// refcount per module instead of a deep copy of every layout. While a
    /// module is being compiled the driver holds the table with a refcount
    /// of exactly one and mutates it through [`Chunk::classes_mut`]; the
    /// `Rc`s are handed to the chunks only once compilation is finished, so
    /// nothing mutates a table anyone else can see.
    pub classes: Rc<Vec<ClassProto>>,
    pub enums: Rc<Vec<EnumProto>>,
    pub interfaces: Rc<Vec<InterfaceProto>>,
    /// Module-wide constant pool, deduplicated by the compiler.
    pub constants: Vec<Value>,
    /// Runtime type descriptors for `CHKTY` and `catch` clauses.
    pub type_descs: Vec<TypeDesc>,
    /// The types `CASTCHK` tests against, as the front end wrote them.
    ///
    /// Deliberately *not* a [`TypeDesc`]. `catch` filters on a shallow test
    /// — is this an instance of that class — but `x as T` is deep:
    /// `t as table<integer>` walks every element, and a `TypeDesc` cannot
    /// say that. Keeping the source `Type` lets `CASTCHK` call the
    /// tree-walker's own `cast`, so the two engines agree by construction
    /// rather than by care.
    pub cast_types: Vec<Rc<saule_ast::Type>>,
    /// Jump tables for `SWITCH`.
    pub jump_tables: Vec<JumpTable>,
    /// `(enum, tag)` pairs `VARIANT` refers to.
    ///
    /// An indirection rather than packing both into the instruction word:
    /// `ABx` has 16 bits, and splitting them 8/8 would cap a program at 256
    /// enums *and* 256 variants each. A table costs one load and has no
    /// limit worth stating.
    pub variant_refs: Vec<(EnumIdx, u32)>,
    /// Number of top-level bindings this module owns.
    pub module_slots: usize,
    /// Where this module's slots start in the **program's** flat slot space.
    ///
    /// One vector holds every module's top-level bindings, and each module's
    /// slot numbers are rebased onto it at compile time. That is what makes
    /// an import need no new opcode: the importing module's slot and the
    /// exporting module's slot are both indices into the same vector, so
    /// copying one to the other is an ordinary `GETMOD` + `SETMOD`.
    ///
    /// Zero for a single-module compile, where the two spaces coincide.
    pub module_slot_base: usize,
    /// This module's position in its program, and so the row of the VM's
    /// per-module closure cache it owns. Proto indices are per chunk, so one
    /// flat cache would have index 5 mean two different functions.
    pub module_index: usize,
    /// The module body.
    pub main: ProtoIdx,
    pub source: Rc<miette::NamedSource<String>>,
}

impl Chunk {
    /// An empty chunk with a single empty `main` — the base a hand-assembled
    /// test chunk or the compiler builds on.
    pub fn empty(name: &str) -> Chunk {
        Chunk {
            protos: Vec::new(),
            classes: Rc::new(Vec::new()),
            enums: Rc::new(Vec::new()),
            interfaces: Rc::new(Vec::new()),
            constants: Vec::new(),
            type_descs: Vec::new(),
            cast_types: Vec::new(),
            jump_tables: Vec::new(),
            variant_refs: Vec::new(),
            module_slots: 0,
            module_slot_base: 0,
            module_index: 0,
            main: 0,
            source: Rc::new(miette::NamedSource::new(name, String::new())),
        }
    }

    /// Append a proto and return its index.
    pub fn add_proto(&mut self, proto: Proto) -> ProtoIdx {
        self.protos.push(Rc::new(proto));
        (self.protos.len() - 1) as ProtoIdx
    }

    /// Intern a constant, reusing an equal existing entry.
    ///
    /// Equality here is `Value`'s own `PartialEq`, which compares reference
    /// types by pointer — exactly the behaviour wanted, since two distinct
    /// table literals must stay distinct constants.
    pub fn add_constant(&mut self, v: Value) -> ConstIdx {
        if let Some(i) = self.constants.iter().position(|k| *k == v) {
            return i as ConstIdx;
        }
        self.constants.push(v);
        (self.constants.len() - 1) as ConstIdx
    }

    pub fn proto(&self, idx: ProtoIdx) -> &Rc<Proto> {
        &self.protos[idx as usize]
    }

    /// Mutable access to the shared class table, during compilation.
    ///
    /// Deliberately `get_mut` and not `make_mut`: `make_mut` would silently
    /// *clone* the table if anyone else already held it, leaving the
    /// compiler writing vtable slots into a copy that the VM never sees —
    /// a wrong answer with no symptom, which is the failure mode §24.2 is
    /// about. Panicking says the driver shared the tables too early, which
    /// is a compiler bug and cannot be reached from user input.
    pub fn classes_mut(&mut self) -> &mut Vec<ClassProto> {
        Rc::get_mut(&mut self.classes).expect("class table shared before compilation finished")
    }

    pub fn enums_mut(&mut self) -> &mut Vec<EnumProto> {
        Rc::get_mut(&mut self.enums).expect("enum table shared before compilation finished")
    }

    pub fn interfaces_mut(&mut self) -> &mut Vec<InterfaceProto> {
        Rc::get_mut(&mut self.interfaces)
            .expect("interface table shared before compilation finished")
    }

    /// Intern a `CASTCHK` type, reusing an equal existing entry.
    ///
    /// Deduplication is what keeps `CASTCHK`'s 8-bit `C` operand roomy: a
    /// program casts to a handful of distinct types however many times it
    /// writes `as`.
    pub fn add_cast_type(&mut self, ty: &saule_ast::Type) -> TypeIdx {
        if let Some(i) = self.cast_types.iter().position(|t| t.as_ref() == ty) {
            return i as TypeIdx;
        }
        self.cast_types.push(Rc::new(ty.clone()));
        (self.cast_types.len() - 1) as TypeIdx
    }
}

/// A compiled function body: the direct replacement for `FunctionObject`'s
/// `FunctionBody::Block(Arc<[Spanned<Stmt>]>)`.
#[derive(Debug)]
pub struct Proto {
    pub name: Option<Rc<str>>,
    pub n_params: u8,
    pub is_variadic: bool,
    /// Frame size: the register high-water mark recorded by §18's allocator.
    pub max_regs: u8,
    pub code: Vec<Instruction>,
    pub upvals: Vec<UpvalDesc>,
    /// Nested closures, as indices into [`Chunk::protos`]. `CLOSURE Bx`
    /// indexes *this* vector, not the chunk's.
    pub protos: Vec<ProtoIdx>,
    /// `try`/`catch` ranges, sorted by `pc_start` so unwinding binary-searches.
    pub handlers: Vec<Handler>,
    /// `pc -> span`, sorted by `pc`. Out of band: it never touches the
    /// instruction stream, so it costs nothing until something fails (§12.3).
    pub lines: Vec<LineEntry>,
    /// Per-call-site inline caches (§8.5). Runtime scratch, not serialized.
    pub caches: RefCell<Vec<InlineCache>>,
    pub owner_class: Option<ClassIdx>,
    /// Per-arity entry points for callees with defaulted parameters (§19).
    /// `entries[n]` is the pc to start at when called with `n` arguments;
    /// empty means "always start at 0".
    pub entries: Vec<u32>,
    /// The module-relative source this proto was compiled from. Carried per
    /// proto so an error inside an imported module renders against the right
    /// file — the job `FunctionObject.source` does today.
    pub source: Option<Rc<miette::NamedSource<String>>>,
}

impl Proto {
    /// A proto with nothing in it but a name and a body.
    pub fn new(name: Option<&str>, n_params: u8, max_regs: u8, code: Vec<Instruction>) -> Proto {
        Proto {
            name: name.map(Rc::from),
            n_params,
            is_variadic: false,
            max_regs,
            code,
            upvals: Vec::new(),
            protos: Vec::new(),
            handlers: Vec::new(),
            lines: Vec::new(),
            caches: RefCell::new(Vec::new()),
            owner_class: None,
            entries: Vec::new(),
            source: None,
        }
    }

    /// Source span for a program counter, for diagnostics. Binary search
    /// over the line table; an empty table yields `0..0`, which renders as
    /// a spanless error rather than pointing at the wrong text.
    pub fn span_at(&self, pc: u32) -> std::ops::Range<usize> {
        if self.lines.is_empty() {
            return 0..0;
        }
        let i = match self.lines.binary_search_by_key(&pc, |e| e.pc) {
            Ok(i) => i,
            // `pc` sits between entries: the covering entry is the one before.
            Err(0) => return 0..0,
            Err(i) => i - 1,
        };
        let e = &self.lines[i];
        e.span_start as usize..e.span_end as usize
    }

    /// The pc to enter at for a call with `n_args` arguments (§19's entry
    /// stubs). Falls back to 0 when the proto has no defaulted parameters.
    pub fn entry_for(&self, n_args: u8) -> u32 {
        self.entries.get(n_args as usize).copied().unwrap_or(0)
    }

    /// Human-readable name for diagnostics and the disassembler.
    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or("<lambda>")
    }
}

/// Where a closure's upvalue comes from at the moment the closure is built.
#[derive(Debug, Clone)]
pub struct UpvalDesc {
    /// `true`: a register of the *parent* frame. `false`: an upvalue of the
    /// parent closure.
    pub from_parent_stack: bool,
    pub index: u8,
    /// Diagnostics only.
    pub name: Rc<str>,
}

/// A `try`/`catch` range (§12.1). The happy path costs zero instructions:
/// entering a `try` emits nothing, and only a `throw` consults this table.
#[derive(Debug, Clone)]
pub struct Handler {
    pub pc_start: u32,
    /// Exclusive.
    pub pc_end: u32,
    /// Catch-block entry.
    pub target: u32,
    /// Register the caught value lands in.
    pub err_reg: u8,
    /// Type the `catch` clause filters on.
    pub catch_ty: TypeIdx,
}

/// `pc -> source span`, sorted by `pc`.
#[derive(Debug, Clone, Copy)]
pub struct LineEntry {
    pub pc: u32,
    pub span_start: u32,
    pub span_end: u32,
}

/// A monomorphic call-site cache (§8.5).
///
/// The design sketch stores a `*const ClassObject`. This stores the `Rc`
/// instead: a raw pointer that outlives its class can be matched by a *new*
/// class allocated at the same address, which would silently read the wrong
/// field. Holding the `Rc` makes that impossible, and the hit path still
/// only does a pointer compare — `Rc::as_ptr` touches no refcount.
///
/// No invalidation is needed: Saule has no metatables and no runtime class
/// mutation, so a `(class, slot)` pair is permanently valid once observed.
#[derive(Debug, Clone, Default)]
pub enum InlineCache {
    #[default]
    Empty,
    Mono {
        class: Rc<ClassObject>,
        slot: u16,
    },
}

/// A compiled class (§8.1). Field slots and vtable slots are both
/// *prefix-extensions* of the parent's, which is what makes an instruction
/// compiled against a static type correct for any subclass receiver.
#[derive(Debug)]
pub struct ClassProto {
    pub name: Rc<str>,
    /// Which module declared this class.
    ///
    /// The class table is program-global, but `vtable` and `static_methods`
    /// hold **proto** indices and those are per chunk. Without this a
    /// `CALLM` on a class from another module would load proto 7 of the
    /// *calling* module — a different function, silently.
    pub module: usize,
    pub parent: Option<ClassIdx>,
    pub layout: Rc<FieldLayout>,
    /// Constant indices for each field slot, present when every default is a
    /// constant. `NEW` then clones a template instead of running code.
    pub field_template: Option<Vec<ConstIdx>>,
    /// Synthetic proto initializing fields whose defaults are not constant.
    /// Runs the parent's first, mirroring `init_fields`'s recursion.
    pub field_init: Option<ProtoIdx>,
    /// Instance methods by slot; parent slots inherited, overrides written
    /// in place, new methods appended.
    pub vtable: Vec<ProtoIdx>,
    /// name -> vtable slot. Compile-time use; the VM indexes directly.
    pub vindex: HashMap<Rc<str>, u16>,
    /// Whether this class declares `implements Assignable<T>`, so a bare `T`
    /// in a slot declared as this class is built with its `of` static.
    ///
    /// On the **program-global** class table rather than in a per-module set,
    /// because the coercion fires at the *binding site*, which is very often
    /// in a module that only imported the class.
    pub assignable: bool,
    pub n_statics: u16,
    /// name -> the class that *declares* the static, and its slot there.
    ///
    /// The declaring class, not this one. Static storage is one vector per
    /// class index, so an inherited name resolved against the *subclass*
    /// would address a second, never-initialized cell — `Derived.total`
    /// would read `nil` where the tree-walker reads the parent's value.
    /// `ClassObject::declaring_static_field` states the same rule for the
    /// tree-walker; carrying the owner in the index is how the compiler
    /// cannot forget it.
    pub sindex: HashMap<Rc<str>, StaticSlot>,
    /// Synthetic proto evaluating static-field initializers, run once when
    /// the class is created.
    pub statics_init: Option<ProtoIdx>,
    pub static_methods: Vec<ProtoIdx>,
    /// name -> the class that *declares* the static method, and its slot
    /// there.
    ///
    /// Flattened like `vindex`, and for the same reason: a subclass must
    /// find an inherited `static fn` in one probe. The declaring class is
    /// carried because `static_methods` is one vector per class index — an
    /// inherited name resolved against the *subclass* would index its own
    /// (empty) vector. The same rule `sindex` follows for static fields.
    pub smindex: HashMap<Rc<str>, StaticSlot>,
    /// interface -> (interface method slot -> this class's vtable slot).
    pub itables: HashMap<InterfaceIdx, Vec<u16>>,
    /// Vtable slot of `init`, resolved through the inheritance chain.
    pub init: Option<u16>,

    /// vtable slot -> index of the `ClassMember` that fills it.
    ///
    /// Compile-time only: Pass 1 assigns the slots, codegen compiles the
    /// bodies, and nothing at run time consults this. It is not part of the
    /// serializable shape.
    pub member_of_vslot: HashMap<u16, usize>,
    /// static-method slot -> index of the `ClassMember` that fills it.
    pub member_of_sslot: Vec<usize>,
}

/// A compiled interface (§8.4).
///
/// An interface has **no layout of its own** — no fields, no vtable. It is a
/// numbering: method name to a slot, so a call site that only knows
/// "something implementing `Drawable`" can name `draw` by index. Each
/// implementing class carries an `itable` translating those slots into its
/// own vtable slots.
#[derive(Debug)]
pub struct InterfaceProto {
    pub name: Rc<str>,
    /// Method names in slot order, including any inherited through
    /// `extends`.
    pub methods: Vec<Rc<str>>,
    pub index: HashMap<Rc<str>, u16>,
}

/// A compiled enum. Variants are dense by tag, in declaration order (§9.1).
#[derive(Debug)]
pub struct EnumProto {
    pub name: Rc<str>,
    /// Which module declared this enum — a variant's value is an index into
    /// that module's constant pool.
    pub module: usize,
    pub variants: Vec<VariantProto>,
    pub by_name: HashMap<Rc<str>, u32>,
    /// Methods declared on the enum: name -> proto index in the declaring
    /// module's chunk.
    ///
    /// Laid out in pass 1 with a `u32::MAX` placeholder and filled by
    /// `enum_decl` in pass 2, the same two-step every class method takes —
    /// a method may mention a class or enum declared further down the file.
    pub methods: HashMap<Rc<str>, u32>,
}

#[derive(Debug, Clone)]
pub struct VariantProto {
    pub name: Rc<str>,
    /// Number of payload fields; 0 for a bare variant.
    pub arity: u8,
    /// Constant holding the declared value of `Variant = <literal>`.
    ///
    /// Only literals are representable: the tree-walker evaluates the
    /// expression once at declaration time, and a chunk stores constants,
    /// not code. A non-literal is refused rather than mis-evaluated.
    pub value: Option<ConstIdx>,
}

/// Where a static field actually lives: the class that declares it, and its
/// slot in that class's storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaticSlot {
    pub class: ClassIdx,
    pub slot: u16,
}

/// A `SWITCH` target table (§9.2). `default` catches out-of-range tags,
/// which is how a `match` with a wildcard arm compiles.
#[derive(Debug, Clone)]
pub struct JumpTable {
    pub targets: Vec<u32>,
    pub default: u32,
}

/// A runtime type test, used by `catch`, `as`, and `is`.
#[derive(Debug, Clone)]
pub enum TypeDesc {
    Any,
    Nil,
    Bool,
    Int,
    Float,
    Str,
    Table,
    Function,
    Class(ClassIdx),
    Enum(EnumIdx),
    /// Named rather than indexed: a `catch` may filter on a class that lives
    /// in another module and has no `ClassProto` in this chunk.
    Named(Rc<str>),
    /// `T?` — nil, or whatever the descriptor at this index says.
    ///
    /// An index into the same `type_descs` pool rather than a `Box`, so the
    /// pool stays flat and a chunk stays as serializable as it was.
    ///
    /// **This collapsed to `Any` and that was a live divergence.**
    /// `runtime_matches_type` reads `Type::Nullable(inner)` as
    /// `nil || inner`, so `catch e: string?` does *not* catch a thrown
    /// integer under the tree-walker — while `Any` caught everything under
    /// the VM. Silent: the program ran and printed, it just printed where
    /// the oracle raised. `Type::Tuple` really is `true` on both sides
    /// (`multi-return shapes aren't introspectable here`), so that one stays
    /// `Any`.
    Nullable(u32),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::op::{Instruction, Op};

    #[test]
    fn span_lookup_picks_the_covering_entry() {
        let mut p = Proto::new(Some("f"), 0, 1, vec![Instruction::abc(Op::RET0, 0, 0, 0)]);
        p.lines = vec![
            LineEntry { pc: 0, span_start: 10, span_end: 20 },
            LineEntry { pc: 4, span_start: 30, span_end: 40 },
        ];
        assert_eq!(p.span_at(0), 10..20);
        assert_eq!(p.span_at(3), 10..20);
        assert_eq!(p.span_at(4), 30..40);
        assert_eq!(p.span_at(99), 30..40);
    }

    #[test]
    fn constants_are_interned() {
        let mut c = Chunk::empty("t");
        let a = c.add_constant(Value::Int(7));
        let b = c.add_constant(Value::Int(7));
        assert_eq!(a, b);
        assert_eq!(c.constants.len(), 1);
    }
}
