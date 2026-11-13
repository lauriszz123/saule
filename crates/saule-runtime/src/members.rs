//! Members and indices by name, at run time: `obj.field` and `obj[index]`,
//! read and written.
//!
//! The compiler resolves a member of a proved class to a slot and never
//! comes here. This is the rest — a receiver the front end could not prove
//! (`GETFX`, `SETFX`, `GETIDX`), a class held in a variable, a stdlib
//! value — and it covers every receiver kind in one place, so each kind
//! answers the same way wherever it is reached from.

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{RuntimeError, a_type};
use crate::value::{EnumObject, EnumVariantObject, NativeClosure, SauleStr, TableObject, Value};

/// A callable that constructs a fresh `EnumVariant` carrying its arguments
/// as an array-style table payload — what `Event.Click` is as a *value*,
/// read off an enum the compiler could not see. The arity is checked at
/// call time; pattern matching on `Enum.Variant(p1, p2, ...)` destructures
/// the payload positionally.
pub(crate) fn make_tuple_variant_ctor(
    enum_obj: Rc<EnumObject>,
    variant_name: String,
    arity: usize,
) -> Value {
    let label = format!("{}.{} (variant ctor)", enum_obj.name, variant_name);
    // Leak the descriptive name into a `&'static str` because `NativeClosure`
    // wants `&'static str` for its `name`. One leak per declared tuple
    // variant is fine — declarations happen once at startup.
    let static_name: &'static str = Box::leak(label.into_boxed_str());
    let enum_name = enum_obj.name.clone();
    // Resolved once, when the constructor is built rather than on every
    // call, so each `Event.Click(x, y)` carries its declaration's tag.
    let tag = enum_obj.tag_of(&variant_name).unwrap_or(u32::MAX);
    Value::NativeClosure(Rc::new(NativeClosure {
        name: static_name,
        func: Box::new(move |args: &[Value]| -> Result<Vec<Value>, String> {
            if args.len() != arity {
                return Err(format!(
                    "{}.{} expects {arity} argument(s), got {}",
                    enum_name,
                    variant_name,
                    args.len()
                ));
            }
            let payload = Value::Table(Rc::new(RefCell::new(TableObject::from_array(
                args.to_vec(),
            ))));
            let variant = Rc::new(EnumVariantObject {
                enum_name: SauleStr::new(enum_name.clone()),
                variant_name: SauleStr::new(variant_name.clone()),
                tag,
                value: std::cell::OnceCell::from(payload),
                enum_obj: RefCell::new(Some(enum_obj.clone())),
            });
            Ok(vec![Value::EnumVariant(variant)])
        }),
        param_names: Vec::new(),
    }))
}

/// Read `receiver.name`.
///
/// On an instance:
///   1. instance fields,
///   2. class methods,
///   3. class static fields,
///   4. class static methods.
///
/// On a class:
///   1. static fields,
///   2. static methods.
pub fn read_member(
    receiver: &Value,
    name: &str,
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    match receiver {
        Value::Instance(inst) => {
            let inst_ref = inst.borrow();
            if let Some(v) = inst_ref.field(name) {
                return Ok(v.clone());
            }
            if let Some(m) = inst_ref.class.lookup_method(name) {
                return Ok(m.to_value());
            }
            if let Some(v) = inst_ref.class.lookup_static_field(name) {
                return Ok(v);
            }
            if let Some(m) = inst_ref.class.lookup_static_method(name) {
                return Ok(m.to_value());
            }
            let class = &inst_ref.class.name;
            let fields = inst_ref.class.layout.names();
            let err = RuntimeError::fault(
                format!("`{class}` has no field or method `{name}`"),
                format!("this is {}", a_type(class)),
                span,
            );
            Err(err.with_help(member_help(class, fields)))
        }
        Value::Class(class) => {
            if let Some(v) = class.lookup_static_field(name) {
                return Ok(v);
            }
            if let Some(m) = class.lookup_static_method(name) {
                return Ok(m.to_value());
            }
            Err(no_static_member(&class.name, name, span))
        }
        Value::Enum(enum_obj) => {
            if let Some(variant) = enum_obj.variants.get(name) {
                return Ok(Value::EnumVariant(variant.clone()));
            }
            if let Some(&arity) = enum_obj.tuple_variants.get(name) {
                return Ok(make_tuple_variant_ctor(
                    enum_obj.clone(),
                    name.to_string(),
                    arity,
                ));
            }
            let mut variants: Vec<&str> = enum_obj
                .variants
                .keys()
                .chain(enum_obj.tuple_variants.keys())
                .map(String::as_str)
                .collect();
            variants.sort_unstable();
            Err(RuntimeError::fault(
                format!("enum `{}` has no variant `{name}`", enum_obj.name),
                format!("`{}` is an enum", enum_obj.name),
                span,
            )
            .with_help(format!("its variants are `{}`", variants.join("`, `"))))
        }
        Value::EnumVariant(variant) => match name {
            "value" => Ok(variant
                .value
                .get()
                .cloned()
                .unwrap_or(Value::Str(variant.variant_name.clone()))),
            "name" => Ok(Value::Str(variant.variant_name.clone())),
            _ => {
                if let Some(enum_obj) = variant.enum_obj.borrow().as_ref()
                    && let Some(m) = enum_obj.methods.get(name)
                {
                    return Ok(m.to_value());
                }
                Err(RuntimeError::fault(
                    format!(
                        "enum variant `{}.{}` has no property or method `{name}`",
                        variant.enum_name, variant.variant_name
                    ),
                    format!("this is `{}.{}`", variant.enum_name, variant.variant_name),
                    span,
                )
                .with_help(format!(
                    "a variant has `name` and `value`, plus the methods declared in `enum {}`",
                    variant.enum_name
                )))
            }
        },
        // Lua-style table access: `t.foo` is sugar for `t["foo"]`. Misses
        // produce `nil` (Lua semantics) rather than a runtime error, so
        // `t.maybe` is a safe probe.
        Value::Table(items) => Ok(items.borrow().get_str(name)),
        // An object of a native class: a property runs its getter; a method
        // reads as the method itself, taking the object first — what an
        // instance of a Saule class gives back for the same read.
        Value::Foreign(obj) => {
            if let Some(getter) = obj.class.getters.get(name) {
                let vs = crate::call::call_value(getter, std::slice::from_ref(receiver), span)?;
                return Ok(vs.into_iter().next().unwrap_or(Value::Nil));
            }
            if let Some(m) = obj.class.methods.get(name) {
                return Ok(m.clone());
            }
            Err(RuntimeError::fault(
                format!("`{}` has no property or method `{name}`", obj.class.name),
                format!("this is {}", a_type(&obj.class.name)),
                span,
            ))
        }
        other => Err(RuntimeError::fault(
            format!("cannot read `.{name}` from {}", a_type(other.type_name())),
            format!("this is {}, which has no members", a_type(other.type_name())),
            span,
        )
        .with_help(
            "only objects, classes, enums and tables have members; check that this holds the value you expect",
        )),
    }
}

/// The help for a member that isn't there: what is there, and the usual
/// reason — a typo. Shared by a read and a method call.
pub(crate) fn member_help(class: &str, fields: &[Rc<str>]) -> String {
    if fields.is_empty() {
        format!(
            "check the spelling against `class {class}` (names are case-sensitive), or add the member there"
        )
    } else {
        format!(
            "`{class}` has the field{} `{}`. Check the spelling (names are case-sensitive), or add the member to `class {class}`",
            if fields.len() == 1 { "" } else { "s" },
            fields.join("`, `")
        )
    }
}

/// `Class.name` where the class has no static `name`. Shared by a read and a
/// call, which have to say the same thing.
pub(crate) fn no_static_member(
    class: &str,
    name: &str,
    span: std::ops::Range<usize>,
) -> RuntimeError {
    RuntimeError::fault(
        format!("class `{class}` has no static member `{name}`"),
        format!("`{class}` here is the class itself, not an object made from it"),
        span,
    )
    .with_help(format!(
        "if `{name}` belongs to each object, make one first and use that: `local x = {class}(…)` then `x.{name}`"
    ))
}

/// Write `receiver.name = value`.
pub fn write_member(
    receiver: &Value,
    name: &str,
    value: Value,
    span: std::ops::Range<usize>,
) -> Result<(), RuntimeError> {
    match receiver {
        Value::Instance(inst) => {
            // Instances have a fixed shape, so there is no slot to conjure
            // for a name the class never declared. The typechecker already
            // rejects that (`tests/errors/classes/unknown_field.sau`); this fires for a
            // receiver it could not prove, where silently creating an
            // invisible field would be a worse answer than saying so.
            if inst.borrow_mut().set_field(name, value) {
                return Ok(());
            }
            let class = inst.borrow().class.name.clone();
            let names = inst.borrow().class.layout.names().to_vec();
            let known = format!(
                "the field{} `{}`",
                if names.len() == 1 { "" } else { "s" },
                names.join("`, `")
            );
            let err = RuntimeError::fault(
                format!("class `{class}` has no field `{name}`"),
                "assigned here",
                span,
            );
            Err(if names.is_empty() {
                err.with_help(format!(
                    "`{class}` declares no fields; declare `{name}` in `class {class}` to give its objects one"
                ))
            } else {
                err.with_help(format!(
                    "`{class}` has {known}. Check the spelling, or declare `{name}` in `class {class}` — an object only has the fields its class declares"
                ))
            })
        }
        Value::Class(class) => {
            // Walk the chain — `Child.staticField = …` updates the declaring
            // class so the change is visible to every sibling — and failing
            // that, define a fresh static on the most-derived class.
            if !class.set_static_field(name, value.clone()) {
                class
                    .static_fields
                    .borrow_mut()
                    .insert(name.to_string(), value);
            }
            Ok(())
        }
        // Lua-style table write: `t.foo = v` is sugar for `t["foo"] = v`.
        Value::Table(items) => {
            let key = Value::Str(SauleStr::new(name.to_string()));
            items
                .borrow_mut()
                .set(&key, value)
                .map_err(|message| table_write_failed(message, span))
        }
        // An object of a native class: a property with a setter.
        Value::Foreign(obj) => {
            if let Some(setter) = obj.class.setters.get(name) {
                crate::call::call_value(setter, &[receiver.clone(), value], span)?;
                return Ok(());
            }
            let class = &obj.class.name;
            Err(if obj.class.getters.contains_key(name) {
                RuntimeError::fault(
                    format!("`{class}.{name}` is read-only"),
                    "assigned here",
                    span,
                )
                .with_help(format!("`{class}` lets `{name}` be read, but defines no setter for it"))
            } else {
                RuntimeError::fault(
                    format!("`{class}` has no property `{name}`"),
                    "assigned here",
                    span,
                )
            })
        }
        other => Err(RuntimeError::fault(
            format!("cannot set `.{name}` on {}", a_type(other.type_name())),
            format!("this is {}, which has no fields", a_type(other.type_name())),
            span,
        )
        .with_help(
            "only objects, classes and tables have fields to set; check that this holds the value you expect",
        )),
    }
}

/// A table refusing a write — a `nil` or NaN key. The table says why; this
/// says where.
pub fn table_write_failed(message: String, span: std::ops::Range<usize>) -> RuntimeError {
    RuntimeError::native(message, None, span).relabel("the key used here")
}
