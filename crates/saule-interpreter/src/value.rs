//! Runtime values for the Saule interpreter.
//!
//! When adding a new variant, remember to extend [`Value::type_name`],
//! [`Value::is_truthy`], [`Value::to_display_string`], and the `PartialEq`
//! impl below.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use saule_ast::{Expr, Param, Spanned, Stmt};

use crate::env::Environment;

/// A Saule runtime value.
#[derive(Debug, Clone)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    /// Interned via `Rc` so cloning a value is cheap.
    Str(Rc<String>),
    /// Array-style table storage. Shared by reference so aliasing behaves like
    /// Lua tables and indexed mutation is visible through every alias.
    Table(Rc<RefCell<Vec<Value>>>),
    /// Built-in function written in Rust (e.g. `print`).
    Native(Rc<NativeFn>),
    /// User-defined function or lambda. The `Rc` makes cloning cheap and
    /// gives recursive closures something stable to point at.
    Function(Rc<FunctionObject>),
    /// A class declaration — carries its statics, instance template, and
    /// optional parent. Compared by identity.
    Class(Rc<ClassObject>),
    /// An instance produced by `new ClassName(args)`. Fields are mutable
    /// behind a `RefCell`; identity is the `Rc` pointer.
    Instance(Rc<RefCell<InstanceObject>>),
    /// An enum variant: carries the enum name, variant name, and optional
    /// value. Compared by identity so each access to `Status.Alive` returns
    /// the same `Rc` pointer.
    EnumVariant(Rc<EnumVariantObject>),
    /// An enum declaration — carries its variants and methods.
    Enum(Rc<EnumObject>),
    /// An interface declaration — carries method signatures.
    Interface(Rc<InterfaceObject>),
}

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
    /// Methods defined on the enum, keyed by name.
    pub methods: HashMap<String, Rc<FunctionObject>>,
}

/// Rust-implemented function exposed to Saule.
///
/// The function returns `Result<Value, String>` where the error string is
/// surfaced as a `RuntimeError::TypeError` at the call site.
pub struct NativeFn {
    pub name: &'static str,
    pub func: fn(&[Value]) -> Result<Value, String>,
}

impl fmt::Debug for NativeFn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<native fn {}>", self.name)
    }
}

/// A user-defined function carrying its body and lexical closure.
#[derive(Debug)]
pub struct FunctionObject {
    /// `Some(name)` for named declarations, `None` for lambdas.
    pub name: Option<String>,
    pub params: Vec<Param>,
    pub body: FunctionBody,
    /// The environment that was in scope when the function was created.
    /// Captured by reference so inner functions see live bindings.
    pub closure: Rc<RefCell<Environment>>,
}

/// Function bodies come in two shapes:
///   * a block of statements (named `fn` decls, block-body lambdas),
///   * a single expression (arrow-style lambdas like `(x) => x + 1`).
#[derive(Debug, Clone)]
pub enum FunctionBody {
    Block(Vec<Spanned<Stmt>>),
    Expr(Box<Spanned<saule_ast::Expr>>),
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

impl Value {
    /// Human-readable type name for error messages — matches the names used
    /// in the README's type system.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Nil => "nil",
            Value::Bool(_) => "boolean",
            Value::Int(_) => "integer",
            Value::Float(_) => "float",
            Value::Str(_) => "string",
            Value::Table(_) => "table",
            Value::Native(_) | Value::Function(_) => "function",
            Value::Class(_) => "class",
            Value::Instance(_) => "instance",
            Value::EnumVariant(_) => "enum",
            Value::Enum(_) => "enum",
            Value::Interface(_) => "interface",
        }
    }

    /// Lua-style truthiness: only `nil` and `false` are falsy.
    pub fn is_truthy(&self) -> bool {
        !matches!(self, Value::Nil | Value::Bool(false))
    }

    /// Format used by `print` and the `..` operator.
    pub fn to_display_string(&self) -> String {
        match self {
            Value::Nil => "nil".into(),
            Value::Bool(b) => b.to_string(),
            Value::Int(n) => n.to_string(),
            Value::Float(f) => {
                // Always show a decimal point so floats are visually distinct
                // from ints, matching the README's display style.
                if f.fract() == 0.0 && f.is_finite() {
                    format!("{f:.1}")
                } else {
                    f.to_string()
                }
            }
            Value::Str(s) => (**s).clone(),
            Value::Table(items) => {
                let parts: Vec<String> = items
                    .borrow()
                    .iter()
                    .map(Value::to_display_string)
                    .collect();
                format!("{{{}}}", parts.join(", "))
            }
            Value::EnumVariant(ev) => {
                format!("{}.{}", ev.enum_name, ev.variant_name)
            }
            Value::Enum(e) => format!("<enum {}>", e.name),
            Value::Interface(iface) => format!("<interface {}>", iface.name),
            Value::Native(nf) => format!("<native fn {}>", nf.name),
            Value::Function(f) => match &f.name {
                Some(n) => format!("<fn {n}>"),
                None => "<lambda>".into(),
            },
            Value::Class(c) => format!("<class {}>", c.name),
            Value::Instance(i) => format!("<instance of {}>", i.borrow().class.name),
        }
    }
}

// Equality is used by tests and by the `==` / `~=` operators. Functions
// (native or user-defined) compare by identity.
impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Nil, Value::Nil) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Table(a), Value::Table(b)) => Rc::ptr_eq(a, b),
            (Value::Native(a), Value::Native(b)) => Rc::ptr_eq(a, b),
            (Value::Function(a), Value::Function(b)) => Rc::ptr_eq(a, b),
            (Value::Class(a), Value::Class(b)) => Rc::ptr_eq(a, b),
            (Value::Instance(a), Value::Instance(b)) => Rc::ptr_eq(a, b),
            (Value::EnumVariant(a), Value::EnumVariant(b)) => Rc::ptr_eq(a, b),
            (Value::Enum(a), Value::Enum(b)) => Rc::ptr_eq(a, b),
            (Value::Interface(a), Value::Interface(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }
}

/// Runtime representation of an `interface` declaration.
///
/// Carries the interface's method signatures. Used for compile-time and
/// runtime verification that implementing classes have the required methods.
#[derive(Debug)]
pub struct InterfaceObject {
    pub name: String,
    /// Parent interfaces (for interface extension).
    pub extends: Vec<String>,
    /// Method signatures required by this interface.
    /// Key is method name, value is (param_count, has_return_type).
    pub methods: HashMap<String, (usize, bool)>,
}
