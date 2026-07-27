//! `enum` declarations and their variants.

use crate::fxhash::FxHashMap as HashMap;
use std::rc::Rc;

use super::Value;
use super::function::FunctionObject;

#[derive(Debug)]
pub struct EnumVariantObject {
    pub enum_name: String,
    pub variant_name: String,
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
    /// Tuple-style variants and their arity. These don't have a singleton
    /// instance; each call produces a fresh `EnumVariantObject` whose
    /// `value` is an array-style table of the positional arguments.
    pub tuple_variants: HashMap<String, usize>,
    /// Methods defined on the enum, keyed by name.
    pub methods: HashMap<String, Rc<FunctionObject>>,
}
