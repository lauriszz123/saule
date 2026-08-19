//! `enum` declarations and their variants.

use crate::fxhash::FxHashMap as HashMap;
use std::rc::Rc;

use super::Value;
use super::class::MethodRef;

#[derive(Debug)]
pub struct EnumVariantObject {
    pub enum_name: String,
    pub variant_name: String,
    /// Dense index in declaration order, `0..variant_count`.
    ///
    /// Every variant of an enum has one, including tuple variants — a
    /// freshly-constructed `Event.Click(x, y)` carries the same tag as the
    /// declaration it came from. This is what lets a `match` over an enum
    /// compile to an indexed jump instead of a chain of tests
    /// (`VM_DESIGN.md` §9.1–9.2).
    pub tag: u32,
    pub value: Option<Value>,
    /// Reference to the enum so we can access methods. Stored in RefCell to
    /// allow updating after enum creation (breaking the circular reference issue).
    pub enum_obj: std::cell::RefCell<Option<Rc<EnumObject>>>,
}

#[derive(Debug)]
pub struct EnumObject {
    pub name: String,
    /// Enum variants, keyed by name. Each variant is cached so identity is stable.
    pub variants: HashMap<String, Rc<EnumVariantObject>>,
    /// Every variant by tag, in declaration order.
    ///
    /// `None` for a tuple variant: it has no singleton, because each call
    /// constructs a fresh object carrying its own payload.
    pub by_tag: Vec<Option<Rc<EnumVariantObject>>>,
    /// name -> tag. Covers tuple variants too, which is why it is separate
    /// from `variants`.
    pub tags: HashMap<String, u32>,
    /// Tuple-style variants and their arity. These don't have a singleton
    /// instance; each call produces a fresh `EnumVariantObject` whose
    /// `value` is an array-style table of the positional arguments.
    pub tuple_variants: HashMap<String, usize>,
    /// Methods defined on the enum, keyed by name.
    ///
    /// A [`MethodRef`] for the same reason a class's methods are one: an
    /// enum is built by exactly one engine, and before this the map could
    /// only hold a tree-walker `FunctionObject`. The VM's answer was an
    /// empty map, so the bytecode compiler had to refuse any enum with
    /// methods rather than ship a `no property or method` where the
    /// tree-walker succeeds.
    pub methods: HashMap<String, MethodRef>,
}

impl EnumObject {
    pub fn tag_of(&self, variant: &str) -> Option<u32> {
        self.tags.get(variant).copied()
    }

    pub fn variant_by_tag(&self, tag: u32) -> Option<&Rc<EnumVariantObject>> {
        self.by_tag.get(tag as usize)?.as_ref()
    }

    pub fn variant_count(&self) -> usize {
        self.by_tag.len()
    }
}
