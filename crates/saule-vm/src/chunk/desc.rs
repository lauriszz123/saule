//! What a class, interface, enum, static field or type test *is*, as compiled.
//!
//! These are the layout decisions Pass 1 made, frozen into the chunk: which
//! slot a field occupies, which vtable index a method answers to, which tag
//! an enum variant carries. The VM materialises runtime objects from them
//! without ever re-deriving any of it.

use std::collections::HashMap;
use std::rc::Rc;

use saule_interpreter::value::FieldLayout;

use super::{ClassIdx, ConstIdx, EnumIdx, InterfaceIdx, ProtoIdx};

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

