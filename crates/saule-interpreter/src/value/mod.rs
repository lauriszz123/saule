//! Runtime values for the Saule interpreter.
//!
//! The [`Value`] enum lives here; each variant's payload is defined in its
//! own submodule:
//!
//! | Module       | Contents                                       |
//! |--------------|------------------------------------------------|
//! | [`function`] | `NativeFn`, `NativeClosure`, `FunctionObject`  |
//! | [`class`]    | `ClassObject`, `InstanceObject`, `FieldDef`    |
//! | [`enum_`]    | `EnumObject`, `EnumVariantObject`              |
//! | [`interface`]| `InterfaceObject`                              |
//! | [`table`]    | `TableObject`, `TableKey`                      |
//! | [`file`]     | `FileHandle`                                   |
//!
//! When adding a new variant, remember to extend [`Value::type_name`],
//! [`Value::is_truthy`], [`Value::to_display_string`], and the `PartialEq`
//! impl below.

pub mod class;
pub mod enum_;
pub mod file;
pub mod function;
pub mod interface;
pub mod table;

use std::cell::RefCell;
use std::rc::Rc;

pub use class::{ClassObject, FieldDef, FieldLayout, InstanceObject};
pub use enum_::{EnumObject, EnumVariantObject};
pub use file::FileHandle;
pub use function::{
    FunctionBody, FunctionObject, NativeClosure, NativeFn, VmFunction, VmFunctionRef,
};
pub use interface::InterfaceObject;
pub use table::{TableKey, TableObject};

/// A Saule runtime value.
#[derive(Debug, Clone)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    /// Interned via `Rc` so cloning a value is cheap.
    Str(Rc<String>),
    /// Hybrid table storage: a dense array part plus a hashmap part. `table<T>`
    /// uses only the array; `table<K, V>` uses the map (and may also use the
    /// array when keys happen to be positive integers). Shared by reference so
    /// aliasing behaves like Lua tables.
    Table(Rc<RefCell<TableObject>>),
    /// Built-in function written in Rust (e.g. `print`).
    Native(Rc<NativeFn>),
    /// Built-in function with captured Rust state (e.g. iterators). Same call
    /// shape as `Native` but stateful and may return multiple values.
    NativeClosure(Rc<NativeClosure>),
    /// User-defined function or lambda. The `Rc` makes cloning cheap and
    /// gives recursive closures something stable to point at.
    Function(Rc<FunctionObject>),
    /// A user-defined function that was compiled to bytecode instead of
    /// being kept as an AST. Opaque here — only `saule-vm` can call one; see
    /// [`VmFunction`]. The tree-walker never constructs this variant, so a
    /// program run under the tree-walker will never observe it.
    VmFunction(Rc<VmFunctionRef>),
    /// A class declaration — carries its statics, instance template, and
    /// optional parent. Compared by identity.
    Class(Rc<ClassObject>),
    /// An instance produced by calling a class: `ClassName(args)`. Fields are
    /// mutable
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
    /// An open (or closed) file handle returned by `Io.open` and the
    /// `Io.stdin`/`stdout`/`stderr` statics. Methods are dispatched via a
    /// static table inside `stdlib::io`.
    File(Rc<RefCell<FileHandle>>),
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
            Value::Native(_)
            | Value::NativeClosure(_)
            | Value::Function(_)
            | Value::VmFunction(_) => "function",
            Value::Class(_) => "class",
            Value::Instance(_) => "instance",
            Value::EnumVariant(_) => "enum",
            Value::Enum(_) => "enum",
            Value::Interface(_) => "interface",
            Value::File(_) => "file",
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
                let t = items.borrow();
                let array_parts = t.array.iter().map(Value::to_display_string);
                let map_parts = t
                    .map
                    .iter()
                    .map(|(k, v)| format!("{}={}", k.display(), v.to_display_string()));
                let parts: Vec<String> = array_parts.chain(map_parts).collect();
                format!("{{{}}}", parts.join(", "))
            }
            Value::EnumVariant(ev) => {
                format!("{}.{}", ev.enum_name, ev.variant_name)
            }
            Value::Enum(e) => format!("<enum {}>", e.name),
            Value::Interface(iface) => format!("<interface {}>", iface.name),
            Value::Native(nf) => format!("<native fn {}>", nf.name),
            Value::NativeClosure(nc) => format!("<native fn {}>", nc.name),
            Value::Function(f) => match &f.name {
                Some(n) => format!("<fn {n}>"),
                None => "<lambda>".into(),
            },
            Value::VmFunction(f) => match f.vm_name() {
                Some(n) => format!("<fn {n}>"),
                None => "<lambda>".into(),
            },
            Value::Class(c) => format!("<class {}>", c.name),
            Value::Instance(i) => format!("<instance of {}>", i.borrow().class.name),
            Value::File(h) => format!("{:?}", h.borrow()),
        }
    }
}

// Equality is used by tests and by the `==` / `!=` operators. Functions
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
            (Value::NativeClosure(a), Value::NativeClosure(b)) => Rc::ptr_eq(a, b),
            (Value::Function(a), Value::Function(b)) => Rc::ptr_eq(a, b),
            // Compare the data pointers rather than the fat pointers: two
            // `Rc<dyn VmFunction>` to the same closure can carry vtable
            // pointers that a linker was free to duplicate.
            (Value::VmFunction(a), Value::VmFunction(b)) => {
                std::ptr::eq(Rc::as_ptr(a) as *const (), Rc::as_ptr(b) as *const ())
            }
            (Value::Class(a), Value::Class(b)) => Rc::ptr_eq(a, b),
            (Value::Instance(a), Value::Instance(b)) => Rc::ptr_eq(a, b),
            (Value::EnumVariant(a), Value::EnumVariant(b)) => Rc::ptr_eq(a, b),
            (Value::Enum(a), Value::Enum(b)) => Rc::ptr_eq(a, b),
            (Value::Interface(a), Value::Interface(b)) => Rc::ptr_eq(a, b),
            (Value::File(a), Value::File(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }
}
