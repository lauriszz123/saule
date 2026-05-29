//! `class` declarations, field templates, and instance state.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use saule_ast::{Expr, Spanned};

use super::Value;
use super::function::FunctionObject;

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
    /// Instance methods, keyed by name. First parameter is the user-written
    /// `self`, so calling `obj:method(a)` is the same as `method(obj, a)`.
    pub methods: HashMap<String, Rc<FunctionObject>>,
    /// Static fields. Mutable through `ClassName.field = …`.
    pub static_fields: RefCell<HashMap<String, Value>>,
    /// Static methods (no implicit `self`).
    pub static_methods: HashMap<String, Rc<FunctionObject>>,
    /// `constructor(args) … end`. None means the class has no explicit
    /// constructor — `new` still produces a valid instance.
    pub constructor: Option<Rc<FunctionObject>>,
}

#[derive(Debug, Clone)]
pub struct FieldDef {
    pub name: String,
    /// Evaluated in the constructor scope each time an instance is built.
    pub default: Option<Spanned<Expr>>,
}

#[derive(Debug)]
pub struct InstanceObject {
    pub class: Rc<ClassObject>,
    pub fields: HashMap<String, Value>,
}

impl ClassObject {
    /// Walk the inheritance chain for a method.
    pub fn lookup_method(self: &Rc<Self>, name: &str) -> Option<Rc<FunctionObject>> {
        if let Some(m) = self.methods.get(name) {
            return Some(m.clone());
        }
        self.parent.as_ref().and_then(|p| p.lookup_method(name))
    }

    /// Walk the inheritance chain for a static method.
    pub fn lookup_static_method(self: &Rc<Self>, name: &str) -> Option<Rc<FunctionObject>> {
        if let Some(m) = self.static_methods.get(name) {
            return Some(m.clone());
        }
        self.parent
            .as_ref()
            .and_then(|p| p.lookup_static_method(name))
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
}
