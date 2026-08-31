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
//!
//! ## What lives where
//!
//! | file | holds |
//! |---|---|
//! | `mod.rs` | [`Chunk`]: one compiled module, and the pools everything indexes into |
//! | [`proto`] | [`Proto`] and the per-function tables — upvalues, handlers, lines, caches |
//! | [`desc`] | what a class, interface, enum, static or type *is*, as compiled |

pub mod desc;
pub mod proto;

pub use desc::{
    CastFast, CastTest, ClassProto, EnumProto, InterfaceProto, JumpTable, StaticSlot, TypeDesc,
    VariantProto,
};
pub use proto::{Handler, InlineCache, LineEntry, Proto, UpvalDesc};

use std::rc::Rc;

use saule_interpreter::Value;
// **The** field layout, not a copy of it. `saule-interpreter` owns the type
// and the runtime `ClassObject` holds the very same `Rc`, so the compiler and
// the runtime cannot disagree about which slot a field lives in — the failure
// §24.2 calls out as the worst this project could ship. A second definition
// here would reintroduce exactly that risk.
pub use saule_interpreter::value::FieldLayout;


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
    /// [`cast_types`](Self::cast_types) pre-resolved to a tag compare, one
    /// entry per entry, maintained by [`add_cast_type`](Self::add_cast_type).
    ///
    /// **Derived, not data.** Like [`Proto::caches`] this is recomputed from
    /// the chunk rather than carried by it, so §14's bytecode cache can
    /// rebuild it on load and never has to serialize it.
    pub cast_fast: Vec<CastFast>,
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
    /// Dynamic native packages this module `import`s, in source order, each
    /// with the span of the `import` that named it.
    ///
    /// Compiling one folds its exports into constants from the package's
    /// *manifest*, which loads nothing. The shared library behind it is a
    /// runtime side effect, so it stays one:
    /// [`run_program`](crate::run_program) loads these immediately before
    /// this module's body runs — the point at which the tree-walker resolves
    /// the same `import`, so a package that fails to load fails at the same
    /// place under both engines.
    pub dynamic_imports: Vec<(String, std::ops::Range<usize>)>,
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
            cast_fast: Vec::new(),
            jump_tables: Vec::new(),
            variant_refs: Vec::new(),
            module_slots: 0,
            module_slot_base: 0,
            dynamic_imports: Vec::new(),
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
        // Pushed in lockstep: this is the only place either vector grows, and
        // the dispatch loop indexes them with the same operand.
        self.cast_fast.push(CastFast::of(ty));
        debug_assert_eq!(self.cast_types.len(), self.cast_fast.len());
        (self.cast_types.len() - 1) as TypeIdx
    }
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
