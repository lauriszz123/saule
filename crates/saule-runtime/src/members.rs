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

use crate::error::RuntimeError;
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

pub fn read_index(
    receiver: &Value,
    index: Value,
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    match receiver {
        Value::Table(items) => Ok(items.borrow().get(&index)),
        // `obj[key]` on a class instance is `OpIndex` — Saule's `__index`.
        // Unlike Lua's, it is not a miss handler over stored keys: an
        // instance has no key space of its own, so the method *is* the
        // lookup and it runs on every read.
        Value::Instance(_) if has_index_overload(receiver, saule_ast::ops::OP_INDEX.method) => {
            crate::index_hooks::call_index(receiver, index, span)
        }
        other => Err(RuntimeError::TypeError {
            message: format!(
                "cannot index a `{}` — only tables and classes implementing `OpIndex` \
                 support `[index]` access",
                other.type_name()
            ),
            span,
        }),
    }
}

/// Does `v` carry `method` on its class chain? Mirrors `ops`' duck-typed
/// dispatch: the `implements` clause is the static opt-in, and by the time a
/// value reaches here the checker has already had its say.
pub(crate) fn has_index_overload(v: &Value, method: &str) -> bool {
    match v {
        Value::Instance(inst) => {
            let class = inst.borrow().class.clone();
            class.lookup_method(method).is_some()
        }
        _ => false,
    }
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
            Err(RuntimeError::TypeError {
                message: format!(
                    "no field or method `{name}` on instance of class `{}` — available fields: (check class definition)",
                    inst_ref.class.name
                ),
                span,
            })
        }
        Value::Class(class) => {
            if let Some(v) = class.lookup_static_field(name) {
                return Ok(v);
            }
            if let Some(m) = class.lookup_static_method(name) {
                return Ok(m.to_value());
            }
            Err(RuntimeError::TypeError {
                message: format!(
                    "no static member `{name}` on class `{}` — try `{}:` method notation or check if this is an instance method",
                    class.name, class.name
                ),
                span,
            })
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
            Err(RuntimeError::TypeError {
                message: format!(
                    "no variant `{name}` on enum `{}` — check enum definition",
                    enum_obj.name
                ),
                span,
            })
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
                Err(RuntimeError::TypeError {
                    message: format!(
                        "no property or method `{name}` on enum variant `{}.{}`",
                        variant.enum_name, variant.variant_name
                    ),
                    span,
                })
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
            Err(RuntimeError::TypeError {
                message: format!(
                    "no property or method `{name}` on instance of class `{}`",
                    obj.class.name
                ),
                span,
            })
        }
        other => Err(RuntimeError::TypeError {
            message: format!(
                "cannot read field `{name}` on value of type `{}` — only instances, classes, enums, and tables have members",
                other.type_name()
            ),
            span,
        }),
    }
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
            // rejects that (`tests/ui/unknown_field.sau`); this fires for a
            // receiver it could not prove, where silently creating an
            // invisible field would be a worse answer than saying so.
            if inst.borrow_mut().set_field(name, value) {
                return Ok(());
            }
            let class = inst.borrow().class.name.clone();
            let known = inst.borrow().class.layout.names().join("`, `");
            Err(RuntimeError::TypeError {
                message: if known.is_empty() {
                    format!("class `{class}` declares no instance field `{name}`")
                } else {
                    format!(
                        "class `{class}` declares no instance field `{name}` — it has `{known}`"
                    )
                },
                span,
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
                .map_err(|message| RuntimeError::TypeError { message, span })
        }
        // An object of a native class: a property with a setter.
        Value::Foreign(obj) => {
            if let Some(setter) = obj.class.setters.get(name) {
                crate::call::call_value(setter, &[receiver.clone(), value], span)?;
                return Ok(());
            }
            let class = &obj.class.name;
            Err(RuntimeError::TypeError {
                message: if obj.class.getters.contains_key(name) {
                    format!("`{class}.{name}` is read-only: the class defines no setter for it")
                } else {
                    format!("class `{class}` has no property `{name}` to assign")
                },
                span,
            })
        }
        other => Err(RuntimeError::TypeError {
            message: format!(
                "cannot assign field `{name}` on value of type `{}` — only instances and classes can have fields assigned",
                other.type_name()
            ),
            span,
        }),
    }
}

/// Write `receiver[index] = value`.
pub fn write_index(
    receiver: &Value,
    index: Value,
    value: Value,
    span: std::ops::Range<usize>,
) -> Result<(), RuntimeError> {
    match receiver {
        Value::Table(items) => items
            .borrow_mut()
            .set(&index, value)
            .map_err(|message| RuntimeError::TypeError { message, span }),
        // `obj[key] = v` on a class instance is `OpNewIndex` — Saule's
        // `__newindex`. As with `OpIndex`, it runs on every write rather
        // than only on a miss: an instance has no key space to miss in.
        Value::Instance(_) if has_index_overload(receiver, saule_ast::ops::OP_NEW_INDEX.method) => {
            crate::index_hooks::call_new_index(receiver, index, value, span)
        }
        other => Err(RuntimeError::TypeError {
            message: format!(
                "cannot assign through `[index]` on a `{}` — only tables and classes \
                 implementing `OpNewIndex` support indexed assignment",
                other.type_name()
            ),
            span,
        }),
    }
}
