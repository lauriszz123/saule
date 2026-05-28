//! Reading members and indices (`obj.field`, `obj[index]`).

use std::rc::Rc;

use crate::error::RuntimeError;
use crate::value::Value;

use super::construct::make_tuple_variant_ctor;

#[allow(dead_code)]
pub(crate) fn table_index_to_slot(index: &Value) -> Result<Option<usize>, String> {
    match index {
        Value::Int(i) if *i <= 0 => Ok(None),
        Value::Int(i) => Ok(Some((*i as usize) - 1)),
        other => Err(format!(
            "table indices must be integers, got `{}`",
            other.type_name()
        )),
    }
}

pub(super) fn read_index(
    receiver: &Value,
    index: Value,
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    match receiver {
        Value::Table(items) => Ok(items.borrow().get(&index)),
        other => Err(RuntimeError::TypeError {
            message: format!(
                "cannot index a `{}` — only tables support `[index]` access",
                other.type_name()
            ),
            span,
        }),
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
pub(super) fn read_member(
    receiver: &Value,
    name: &str,
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    match receiver {
        Value::Instance(inst) => {
            let inst_ref = inst.borrow();
            if let Some(v) = inst_ref.fields.get(name) {
                return Ok(v.clone());
            }
            if let Some(m) = inst_ref.class.lookup_method(name) {
                return Ok(Value::Function(m));
            }
            if let Some(v) = inst_ref.class.lookup_static_field(name) {
                return Ok(v);
            }
            if let Some(m) = inst_ref.class.lookup_static_method(name) {
                return Ok(Value::Function(m));
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
                return Ok(Value::Function(m));
            }
            Err(RuntimeError::TypeError {
                message: format!(
                    "no static member `{name}` on class `{}` — try `{}:` method notation or check if this is an instance method",
                    class.name,
                    class.name
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
                .clone()
                .unwrap_or(Value::Str(Rc::new(variant.variant_name.clone())))),
            "name" => Ok(Value::Str(Rc::new(variant.variant_name.clone()))),
            _ => {
                if let Some(enum_obj) = variant.enum_obj.borrow().as_ref()
                    && let Some(m) = enum_obj.methods.get(name)
                {
                    return Ok(Value::Function(m.clone()));
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
        Value::Table(items) => Ok(items.borrow().get(&Value::Str(Rc::new(name.to_string())))),
        other => Err(RuntimeError::TypeError {
            message: format!(
                "cannot read field `{name}` on value of type `{}` — only instances, classes, enums, and tables have members",
                other.type_name()
            ),
            span,
        }),
    }
}
