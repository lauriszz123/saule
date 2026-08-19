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

pub(crate) fn read_index(
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
            crate::eval::index_hooks::call_index(receiver, index, span)
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

/// Does `v` carry `method` on its class chain? Mirrors `eval::ops`'
/// duck-typed dispatch: the `implements` clause is the static opt-in, and by
/// the time a value reaches here the checker has already had its say.
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
pub(crate) fn read_member(
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
                .clone()
                .unwrap_or(Value::Str(Rc::new(variant.variant_name.clone())))),
            "name" => Ok(Value::Str(Rc::new(variant.variant_name.clone()))),
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
        other => Err(RuntimeError::TypeError {
            message: format!(
                "cannot read field `{name}` on value of type `{}` — only instances, classes, enums, and tables have members",
                other.type_name()
            ),
            span,
        }),
    }
}
