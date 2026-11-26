//! Where a constant that is not a literal came from, so it can be found
//! again in another process.
//!
//! ## The problem
//!
//! The compiler folds the standard library and native packages into the
//! constant pool: `print`, `Math`, `Graphics.circle` and `IoMode.Read` are
//! **live runtime objects** in [`Chunk::constants`](crate::Chunk::constants)
//! — `Rc`s and function pointers that mean nothing outside the process that
//! made them. Every other constant is a literal and writes itself.
//!
//! ## The answer
//!
//! While a program is compiled for `saule build`, every value the compiler
//! could fold is **registered** here with a path to it:
//!
//! | source | registered as |
//! |---|---|
//! | a prelude name (`print`, `Math`) | [`Origin::Prelude`] |
//! | a native package's export (`Graphics`) | [`Origin::Package`] |
//! | a class's static field (`Math.floor`) | [`Origin::Member`] |
//! | an enum's variant (`IoMode.Read`) | [`Origin::Variant`] |
//! | a native the compiler makes itself | [`Origin::Builtin`] |
//!
//! The writer looks each non-literal constant up by identity and writes the
//! path; the reader follows the path through a fresh prelude and the
//! packages the program imports. A constant with no path is an error the
//! writer reports by name, never something it guesses at.
//!
//! Registering is off by default ([`enable`]): an ordinary `saule run` pays
//! nothing for it.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use saule_runtime::value::NativeFn;
use saule_runtime::{Prelude, Value};

/// A path to a runtime value, from something both processes have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// A prelude name.
    Prelude(Rc<str>),
    /// An export of a native package, static or dynamic.
    Package { package: Rc<str>, name: Rc<str> },
    /// A static field of the class at `of`.
    Member { of: Box<Origin>, name: Rc<str> },
    /// A variant of the enum at `of`.
    Variant { of: Box<Origin>, name: Rc<str> },
    /// A native the compiler creates rather than finds.
    Builtin(Builtin),
}

/// The natives the compiler builds itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    /// Gives a valued enum variant its value; see `Compiler::enum_values`.
    SetVariantValue,
}

impl Builtin {
    pub const ALL: [Builtin; 1] = [Builtin::SetVariantValue];

    pub fn tag(self) -> u8 {
        match self {
            Builtin::SetVariantValue => 0,
        }
    }

    pub fn from_tag(tag: u8) -> Option<Builtin> {
        Builtin::ALL.into_iter().find(|b| b.tag() == tag)
    }

    pub fn value(self) -> Value {
        match self {
            Builtin::SetVariantValue => Value::Native(Rc::new(NativeFn::new(
                "<enum variant value>",
                crate::compile::class::set_variant_value,
            ))),
        }
    }
}

// ─── Registry ───────────────────────────────────────────────────────────────

/// What identifies a value: its allocation, or for a plain native function
/// its code (the compiler builds a fresh `Rc<NativeFn>` around the same
/// function in places, so the allocation would miss).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    Alloc(usize),
    Code(usize),
}

#[derive(Default)]
struct Registry {
    paths: HashMap<Key, Origin>,
    /// Every registered value, kept alive: an address freed and reused by a
    /// different value would otherwise answer with the old value's path.
    keep: Vec<Value>,
}

thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(false) };
    static REGISTRY: RefCell<Registry> = RefCell::new(Registry::default());
}

/// Start recording origins on this thread — before compiling a program that
/// will be written as bytecode.
pub fn enable() {
    ENABLED.with(|e| e.set(true));
    for builtin in Builtin::ALL {
        register(&builtin.value(), Origin::Builtin(builtin));
    }
}

pub(crate) fn enabled() -> bool {
    ENABLED.with(Cell::get)
}

/// Record every binding of `prelude`, and what hangs off it.
pub(crate) fn register_prelude(prelude: &Prelude) {
    if !enabled() {
        return;
    }
    for (name, value) in prelude.entries() {
        register(value, Origin::Prelude(Rc::from(name)));
    }
}

/// Record a native package's exports.
pub(crate) fn register_package<'a>(
    package: &str,
    exports: impl IntoIterator<Item = (&'a String, &'a Value)>,
) {
    if !enabled() {
        return;
    }
    let package: Rc<str> = Rc::from(package);
    for (name, value) in exports {
        register(
            value,
            Origin::Package {
                package: Rc::clone(&package),
                name: Rc::from(name.as_str()),
            },
        );
    }
}

/// A short description of `value` for an error: its type, and its name
/// where it has one. Never `Debug`, which recurses through a class's
/// statics.
pub(crate) fn describe(value: &Value) -> String {
    match value {
        Value::Native(f) => format!("the native function `{}`", f.name),
        Value::NativeClosure(c) => format!("the native function `{}`", c.name),
        Value::Class(c) => format!("the class `{}`", c.name),
        Value::Enum(e) => format!("the enum `{}`", e.name),
        Value::EnumVariant(v) => format!("`{}.{}`", v.enum_name.as_str(), v.variant_name.as_str()),
        other => format!("a {} value", other.type_name()),
    }
}

/// The path to `value`, if it was registered.
pub(crate) fn origin_of(value: &Value) -> Option<Origin> {
    let key = key_of(value)?;
    REGISTRY.with(|r| r.borrow().paths.get(&key).cloned())
}

/// Record `value` under `origin`, and its members under paths through it.
/// The first path recorded for a value is kept: it is the shortest.
fn register(value: &Value, origin: Origin) {
    let Some(key) = key_of(value) else {
        return;
    };
    let fresh = REGISTRY.with(|r| {
        let mut r = r.borrow_mut();
        if r.paths.contains_key(&key) {
            return false;
        }
        r.paths.insert(key, origin.clone());
        r.keep.push(value.clone());
        true
    });
    if !fresh {
        return;
    }

    match value {
        Value::Class(class) => {
            let fields: Vec<(String, Value)> = class
                .static_fields
                .borrow()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            for (name, field) in fields {
                register(
                    &field,
                    Origin::Member {
                        of: Box::new(origin.clone()),
                        name: Rc::from(name.as_str()),
                    },
                );
            }
        }
        Value::Enum(e) => {
            for (name, variant) in &e.variants {
                register(
                    &Value::EnumVariant(Rc::clone(variant)),
                    Origin::Variant {
                        of: Box::new(origin.clone()),
                        name: Rc::from(name.as_str()),
                    },
                );
            }
        }
        _ => {}
    }
}

fn key_of(value: &Value) -> Option<Key> {
    let alloc = |p: *const ()| Some(Key::Alloc(p as usize));
    match value {
        Value::Native(f) => Some(Key::Code(f.func as usize)),
        Value::NativeClosure(c) => alloc(Rc::as_ptr(c).cast()),
        Value::Class(c) => alloc(Rc::as_ptr(c).cast()),
        Value::Enum(e) => alloc(Rc::as_ptr(e).cast()),
        Value::EnumVariant(v) => alloc(Rc::as_ptr(v).cast()),
        Value::Interface(i) => alloc(Rc::as_ptr(i).cast()),
        Value::Table(t) => alloc(Rc::as_ptr(t).cast()),
        Value::Foreign(f) => alloc(Rc::as_ptr(f).cast()),
        _ => None,
    }
}

// ─── Resolving ──────────────────────────────────────────────────────────────

/// Finds origins again in the process that runs the bytecode.
pub(crate) struct World {
    prelude: Rc<RefCell<Prelude>>,
    packages: HashMap<Rc<str>, HashMap<String, Value>>,
}

impl World {
    pub(crate) fn new() -> World {
        saule_runtime::init();
        World {
            prelude: Prelude::with_std(),
            packages: HashMap::new(),
        }
    }

    pub(crate) fn resolve(&mut self, origin: &Origin) -> Result<Value, String> {
        let value = self.follow(origin)?;
        // So a decoded program can be written again.
        if enabled() {
            register(&value, origin.clone());
        }
        Ok(value)
    }

    fn follow(&mut self, origin: &Origin) -> Result<Value, String> {
        match origin {
            Origin::Prelude(name) => self
                .prelude
                .borrow()
                .get(name)
                .ok_or_else(|| format!("the standard library has no `{name}`")),
            Origin::Package { package, name } => {
                let exports = self.package(package)?;
                exports
                    .get(name.as_ref())
                    .cloned()
                    .ok_or_else(|| format!("package `{package}` has no export `{name}`"))
            }
            Origin::Member { of, name } => match self.follow(of)? {
                Value::Class(class) => class
                    .lookup_static_field(name)
                    .ok_or_else(|| format!("`{}` has no static `{name}`", class.name)),
                other => Err(format!("`{name}` was read from a {}", other.type_name())),
            },
            Origin::Variant { of, name } => match self.follow(of)? {
                Value::Enum(e) => e
                    .variants
                    .get(name.as_ref())
                    .map(|v| Value::EnumVariant(Rc::clone(v)))
                    .ok_or_else(|| format!("enum `{}` has no variant `{name}`", e.name)),
                other => Err(format!("`{name}` was read from a {}", other.type_name())),
            },
            Origin::Builtin(builtin) => Ok(builtin.value()),
        }
    }

    fn package(&mut self, package: &Rc<str>) -> Result<&HashMap<String, Value>, String> {
        if !self.packages.contains_key(package) {
            let exports = if let Some(pkg) = saule_runtime::native_packages::lookup(package) {
                saule_runtime::native_packages::build_exports(pkg).values
            } else {
                saule_runtime::dynamic_packages::discover();
                saule_runtime::dynamic_packages::build_exports_deferred(package)
                    .ok_or_else(|| {
                        saule_runtime::dynamic_packages::rejection(package).unwrap_or_else(|| {
                            format!("native package `{package}` is not installed")
                        })
                    })?
                    .values
            };
            self.packages
                .insert(Rc::clone(package), exports.into_iter().collect());
        }
        Ok(&self.packages[package])
    }
}
