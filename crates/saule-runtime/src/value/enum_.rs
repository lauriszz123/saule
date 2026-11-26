//! `enum` declarations and their variants.

use crate::fxhash::FxHashMap as HashMap;
use std::rc::Rc;

use super::class::MethodRef;
use super::{SauleStr, Value};

/// A tuple variant's fields. One and two are what most variants carry and
/// are kept in place; more go to an allocation of their own.
#[derive(Debug)]
enum Fields {
    None,
    One([Value; 1]),
    Two([Value; 2]),
    Many(Box<[Value]>),
}

impl Fields {
    fn of(mut values: Vec<Value>) -> Self {
        match values.len() {
            0 => Fields::None,
            1 => Fields::One([values.remove(0)]),
            2 => {
                let second = values.remove(1);
                Fields::Two([values.remove(0), second])
            }
            _ => Fields::Many(values.into_boxed_slice()),
        }
    }

    #[inline]
    fn as_slice(&self) -> &[Value] {
        match self {
            Fields::None => &[],
            Fields::One(one) => one,
            Fields::Two(two) => two,
            Fields::Many(many) => many,
        }
    }
}

#[derive(Debug)]
pub struct EnumVariantObject {
    /// Shared, not owned: `NEWVAR` builds one of these per tuple-variant
    /// construction, and these two fields were a `String` allocation each
    /// for text the compiler has known since the enum was declared.
    pub enum_name: SauleStr,
    pub variant_name: SauleStr,
    /// Dense index in declaration order, `0..variant_count`.
    ///
    /// Every variant of an enum has one, including tuple variants — a
    /// freshly-constructed `Event.Click(x, y)` carries the same tag as the
    /// declaration it came from. This is what lets a `match` over an enum
    /// compile to an indexed jump instead of a chain of tests
    /// (`VM_DESIGN.md` §9.1–9.2).
    pub tag: u32,
    /// A valued variant's value. Set once: at construction, or — for a
    /// valued variant whose value is an expression rather than a literal —
    /// when its declaration runs, which is when the expression may first be
    /// evaluated.
    ///
    /// For a tuple variant it is the table `.value` answers with, made from
    /// `fields` the first time it is asked for and the truth from then on:
    /// the table can be written to, `fields` cannot. See [`Self::value`].
    value: std::cell::OnceCell<Value>,
    /// What a tuple variant carries, `Event.Click(x, y)`'s `x` and `y`, in
    /// the object itself: a `match` that names them reads them one load
    /// after the tag, with no table in between. Empty for any other
    /// variant.
    fields: Fields,
    /// Reference to the enum so we can access methods. Stored in RefCell to
    /// allow updating after enum creation (breaking the circular reference issue).
    pub enum_obj: std::cell::RefCell<Option<Rc<EnumObject>>>,
}

impl EnumVariantObject {
    /// A variant with a value of its own (`Red = "red"`), or with none yet.
    pub fn valued(
        enum_name: SauleStr,
        variant_name: SauleStr,
        tag: u32,
        value: Option<Value>,
        enum_obj: Option<Rc<EnumObject>>,
    ) -> Self {
        EnumVariantObject {
            enum_name,
            variant_name,
            tag,
            value: value.map(std::cell::OnceCell::from).unwrap_or_default(),
            fields: Fields::None,
            enum_obj: std::cell::RefCell::new(enum_obj),
        }
    }

    /// A tuple variant carrying `fields`, in the order they were given.
    pub fn carrying(
        enum_name: SauleStr,
        variant_name: SauleStr,
        tag: u32,
        fields: Vec<Value>,
        enum_obj: Option<Rc<EnumObject>>,
    ) -> Self {
        let mut value = std::cell::OnceCell::new();
        if fields.is_empty() {
            // Nothing to keep in place, and `.value` is still a table.
            value = std::cell::OnceCell::from(table_of(&[]));
        }
        EnumVariantObject {
            enum_name,
            variant_name,
            tag,
            value,
            fields: Fields::of(fields),
            enum_obj: std::cell::RefCell::new(enum_obj),
        }
    }

    /// The variant's value: what it was declared with, or, for a tuple
    /// variant, a table of its fields. `None` for a variant with neither.
    ///
    /// The table of a tuple variant is made here, once. It is an ordinary
    /// table and may be written to, so from then on it is what the variant
    /// carries and [`Self::field`] steps aside.
    pub fn value(&self) -> Option<&Value> {
        let fields = self.fields.as_slice();
        if fields.is_empty() {
            return self.value.get();
        }
        Some(self.value.get_or_init(|| table_of(fields)))
    }

    /// Give a valued variant the value its declaration computed. Does
    /// nothing to a variant that has one.
    pub fn set_value(&self, value: Value) {
        let _ = self.value.set(value);
    }

    /// Field `i` (from 0) of a tuple variant nothing has asked the table
    /// of. `None` otherwise, and the answer is then in [`Self::value`].
    #[inline]
    pub fn field(&self, i: usize) -> Option<&Value> {
        match self.fields.as_slice().get(i) {
            Some(field) if self.value.get().is_none() => Some(field),
            _ => None,
        }
    }
}

fn table_of(fields: &[Value]) -> Value {
    Value::Table(Rc::new(std::cell::RefCell::new(
        super::TableObject::from_array(fields.to_vec()),
    )))
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
    /// Methods defined on the enum, keyed by name; each takes the variant
    /// as its receiver, like an instance method.
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
