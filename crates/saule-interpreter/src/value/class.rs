//! `class` declarations, field templates, and instance state.

use crate::fxhash::FxHashMap as HashMap;
use std::cell::RefCell;
use std::rc::Rc;

use saule_ast::{Expr, Spanned};

use super::Value;
use super::function::{FunctionObject, VmFunctionRef};

/// A method body, whichever engine compiled it.
///
/// A `ClassObject` is built by exactly one engine — the tree-walker's
/// `exec_class_decl` or the VM's start-up pass — so in any given class every
/// entry is the same arm. The enum exists so a *reader* cannot forget the
/// other one: before it, `methods` was `Rc<FunctionObject>` and a VM-built
/// class simply had an empty map, which made `lookup_method` answer "no
/// `toString`" and `display_value` print `<instance of Money>` with no error
/// at all. Making the two shapes representable in one type turns that silent
/// wrong answer into a `match` the compiler demands.
#[derive(Debug, Clone)]
pub enum MethodRef {
    /// A tree-walker [`FunctionObject`], closing over its defining scope.
    Tree(Rc<FunctionObject>),
    /// A bytecode closure, opaque behind [`VmFunctionRef`].
    Vm(Rc<VmFunctionRef>),
}

impl MethodRef {
    /// The `FunctionObject` behind a tree-walker method.
    ///
    /// `None` for a bytecode method — which is the right answer for the
    /// sites that need one: they want a *defining environment* (field
    /// defaults, `owner_class` back-links), and a bytecode method has no
    /// `Environment` at all.
    pub fn as_tree(&self) -> Option<&Rc<FunctionObject>> {
        match self {
            MethodRef::Tree(f) => Some(f),
            MethodRef::Vm(_) => None,
        }
    }

    /// This method as a first-class value — what `local f = obj.method`
    /// binds.
    pub fn to_value(&self) -> Value {
        match self {
            MethodRef::Tree(f) => Value::Function(Rc::clone(f)),
            MethodRef::Vm(f) => Value::VmFunction(Rc::clone(f)),
        }
    }
}

impl From<Rc<FunctionObject>> for MethodRef {
    fn from(f: Rc<FunctionObject>) -> MethodRef {
        MethodRef::Tree(f)
    }
}

impl From<Rc<VmFunctionRef>> for MethodRef {
    fn from(f: Rc<VmFunctionRef>) -> MethodRef {
        MethodRef::Vm(f)
    }
}

/// Runtime representation of a `class` declaration.
///
/// Instance fields live on the [`InstanceObject`]; statics live here (behind
/// a `RefCell` because they can be reassigned). Methods are stored as
/// already-constructed [`FunctionObject`]s capturing the module-level
/// environment so they can refer to other top-level names (including the
/// class itself for static calls).
#[derive(Debug)]
pub struct ClassObject {
    pub name: String,
    pub parent: Option<Rc<ClassObject>>,
    /// Instance-field templates evaluated on construction.
    pub field_defs: Vec<FieldDef>,
    /// Slot assignment for instance fields, shared by every instance of this
    /// class. See [`FieldLayout`].
    pub layout: Rc<FieldLayout>,
    /// Instance methods, keyed by name. First parameter is the user-written
    /// `self`, so calling `obj.method(a)` is the same as `method(obj, a)`.
    ///
    /// **Flattened**: this map starts as a copy of the parent's and has
    /// overrides written into it, so a lookup is one probe regardless of how
    /// deep the hierarchy is. Inherited entries are the same `Rc` the parent
    /// holds, so flattening costs one map slot per inherited method, not a
    /// second copy of the function.
    pub methods: HashMap<String, MethodRef>,
    /// Static fields. Mutable through `ClassName.field = …`.
    ///
    /// Deliberately *not* flattened: a static is a single mutable slot shared
    /// by the whole chain, and copying it into each subclass would give each
    /// one its own. Writes have to find the declaring class — see
    /// [`ClassObject::declaring_static_field`].
    pub static_fields: RefCell<HashMap<String, Value>>,
    /// Static methods (no implicit `self`). Flattened, like `methods`.
    pub static_methods: HashMap<String, MethodRef>,
    /// `init(args) … end`. None means the class has no explicit
    /// constructor — calling the class still produces a valid instance.
    pub constructor: Option<Rc<FunctionObject>>,
}

#[derive(Debug, Clone)]
pub struct FieldDef {
    pub name: String,
    /// Evaluated in the constructor scope each time an instance is built.
    pub default: Option<Spanned<Expr>>,
}

/// Where each instance field lives, computed once per class and shared by
/// every instance of it.
///
/// The point is that an instance is a `Vec<Value>`, not a hash map: reading
/// `p.health` is one probe of a **per-class** map for the slot, then an
/// indexed load — instead of a probe of a **per-instance** map whose
/// `String` keys were cloned when the object was built.
///
/// **Parent fields occupy the low slots.** A subclass's layout is therefore
/// a prefix-extension of its parent's, never a reordering. That is what
/// makes a slot resolved against a static type still correct when the value
/// turns out to be a subclass — and it is the invariant the bytecode
/// compiler's `GETF` will depend on (`VM_DESIGN.md` §8.2).
#[derive(Debug, Default)]
pub struct FieldLayout {
    index: HashMap<Rc<str>, u16>,
    names: Vec<Rc<str>>,
}

impl FieldLayout {
    /// Extend `parent`'s layout with this class's own fields.
    ///
    /// A field redeclared in a subclass keeps the parent's slot rather than
    /// taking a new one, so the prefix invariant survives shadowing.
    pub fn build(parent: Option<&FieldLayout>, own: &[FieldDef]) -> FieldLayout {
        let mut layout = match parent {
            Some(p) => FieldLayout {
                index: p.index.clone(),
                names: p.names.clone(),
            },
            None => FieldLayout::default(),
        };
        for def in own {
            if layout.index.contains_key(def.name.as_str()) {
                continue;
            }
            let name: Rc<str> = Rc::from(def.name.as_str());
            let slot = layout.names.len() as u16;
            layout.names.push(Rc::clone(&name));
            layout.index.insert(name, slot);
        }
        layout
    }

    /// Build a layout from an already-computed slot assignment.
    ///
    /// Exists for the bytecode compiler's Pass 1, which computes the layout
    /// before any runtime class object exists. The two then **share** the
    /// resulting `Rc` rather than each computing their own — see
    /// `VM_DESIGN.md` §24.2, where a compiler and a runtime disagreeing
    /// about field slots is named as the worst bug the project could ship.
    pub fn from_parts(index: HashMap<Rc<str>, u16>, names: Vec<Rc<str>>) -> FieldLayout {
        debug_assert_eq!(index.len(), names.len(), "slot map and name list disagree");
        FieldLayout { index, names }
    }

    #[inline]
    pub fn slot(&self, name: &str) -> Option<u16> {
        self.index.get(name).copied()
    }

    /// slot -> name, for diagnostics and display.
    pub fn names(&self) -> &[Rc<str>] {
        &self.names
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

#[derive(Debug)]
pub struct InstanceObject {
    pub class: Rc<ClassObject>,
    /// One `Value` per slot in `class.layout`, positionally.
    pub fields: Vec<Value>,
}

impl InstanceObject {
    /// A fresh instance with every field slot at `nil`.
    pub fn new(class: Rc<ClassObject>) -> InstanceObject {
        let n = class.layout.len();
        InstanceObject {
            class,
            fields: vec![Value::Nil; n],
        }
    }

    #[inline]
    pub fn field(&self, name: &str) -> Option<&Value> {
        let slot = self.class.layout.slot(name)?;
        self.fields.get(slot as usize)
    }

    /// Write a declared field. Returns `false` when the class declares no
    /// such field — instances have a fixed shape, so there is no slot to
    /// create. The typechecker already rejects this (`unknown_field.sau`);
    /// the runtime check only catches unchecked `run()` callers.
    #[inline]
    pub fn set_field(&mut self, name: &str, value: Value) -> bool {
        match self.class.layout.slot(name) {
            Some(slot) => {
                // The tree-walker's instance write. See `crate::gc`.
                crate::gc::on_store(&value);
                self.fields[slot as usize] = value;
                true
            }
            None => false,
        }
    }

    /// `(name, value)` pairs in slot order, for diagnostics and display.
    pub fn iter_fields(&self) -> impl Iterator<Item = (&Rc<str>, &Value)> {
        self.class.layout.names().iter().zip(self.fields.iter())
    }
}

impl ClassObject {
    /// Look up an instance method.
    ///
    /// One probe: `methods` is flattened at class-construction time, so this
    /// no longer walks the parent chain hashing the name once per level.
    pub fn lookup_method(self: &Rc<Self>, name: &str) -> Option<MethodRef> {
        self.methods.get(name).cloned()
    }

    /// Look up a static method. Flattened, like [`Self::lookup_method`].
    pub fn lookup_static_method(self: &Rc<Self>, name: &str) -> Option<MethodRef> {
        self.static_methods.get(name).cloned()
    }

    /// Walk the inheritance chain for a static field.
    pub fn lookup_static_field(self: &Rc<Self>, name: &str) -> Option<Value> {
        if let Some(v) = self.static_fields.borrow().get(name) {
            return Some(v.clone());
        }
        self.parent
            .as_ref()
            .and_then(|p| p.lookup_static_field(name))
    }

    /// The class in this chain that actually declares static field `name`.
    ///
    /// Writes target the *declaring* class rather than the most-derived one
    /// so `Child.counter = 1` and a bare-name `counter = 1` inside a method
    /// both update the single shared slot every sibling reads from.
    pub fn declaring_static_field(self: &Rc<Self>, name: &str) -> Option<Rc<ClassObject>> {
        if self.static_fields.borrow().contains_key(name) {
            return Some(self.clone());
        }
        self.parent
            .as_ref()
            .and_then(|p| p.declaring_static_field(name))
    }
}
