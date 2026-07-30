//! Operator implementations shared by expression evaluation.
//!
//! Every operator first offers itself to the operand's class through the
//! built-in `Op*` interfaces (see `stdlib::ops`) — the Saule equivalent of
//! Lua's `__add`, `__sub`, `__concat`, … metamethods. Dispatch here is
//! duck-typed: we look the contract method up on the instance's class chain
//! and call it if present. The `implements OpAdd<…>` clause is what
//! `saule-typeck` enforces statically; the runtime only needs the method.

use std::rc::Rc;

use saule_ast::ops::{binary_contract, binop_symbol, unary_contract};
use saule_ast::{BinOp, UnaryOp};

use crate::error::RuntimeError;
use crate::eval::expr::{EvaluatedArg, invoke_method_multi};
use crate::value::Value;

/// Does `v` carry an operator overload named `method`? Walks the class
/// chain, so a subclass inherits its parent's overloads.
fn has_overload(v: &Value, method: &str) -> bool {
    match v {
        Value::Instance(inst) => {
            let class = inst.borrow().class.clone();
            class.lookup_method(method).is_some()
        }
        _ => false,
    }
}

/// The class name behind an instance, for diagnostics.
fn class_name(v: &Value) -> String {
    match v {
        Value::Instance(inst) => inst.borrow().class.name.clone(),
        other => other.type_name().to_string(),
    }
}

/// Call an operator method, adapting to the single-value shape operators
/// need (a method returning multiple values contributes only its first).
fn call_overload(
    recv: &Value,
    method: &str,
    args: Vec<Value>,
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    let args = args.into_iter().map(EvaluatedArg::Positional).collect();
    let values = invoke_method_multi(recv, method, args, span)?;
    Ok(values.into_iter().next().unwrap_or(Value::Nil))
}

// ──────────────────────────────────────────────────────────────────────────────
// Unary
// ──────────────────────────────────────────────────────────────────────────────

pub fn unary(op: UnaryOp, v: Value, span: std::ops::Range<usize>) -> Result<Value, RuntimeError> {
    if let Some(contract) = unary_contract(op)
        && has_overload(&v, contract.method)
    {
        let out = call_overload(&v, contract.method, Vec::new(), span.clone())?;
        // `#x` is typed `integer` everywhere else in the language; keep that
        // true even when the length came from user code.
        if matches!(op, UnaryOp::Len) && !matches!(out, Value::Int(_)) {
            return Err(RuntimeError::TypeError {
                message: format!(
                    "`{}.len()` must return an `integer`, got `{}`",
                    class_name(&v),
                    out.type_name()
                ),
                span,
            });
        }
        return Ok(out);
    }

    match op {
        UnaryOp::Neg => match v {
            Value::Int(n) => Ok(Value::Int(-n)),
            Value::Float(f) => Ok(Value::Float(-f)),
            other => Err(RuntimeError::TypeError {
                message: format!(
                    "cannot negate a `{}` — negation only works on numbers, or a class implementing `OpNeg`",
                    describe(&other)
                ),
                span,
            }),
        },
        UnaryOp::Not => Ok(Value::Bool(!v.is_truthy())),
        UnaryOp::Len => match v {
            Value::Str(s) => Ok(Value::Int(s.chars().count() as i64)),
            Value::Table(items) => Ok(Value::Int(items.borrow().array_len() as i64)),
            other => Err(RuntimeError::TypeError {
                message: format!(
                    "cannot take length of a `{}` — only strings, tables, and classes implementing `OpLen` have a length",
                    describe(&other)
                ),
                span,
            }),
        },
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Binary
// ──────────────────────────────────────────────────────────────────────────────

pub fn binary(
    op: BinOp,
    l: Value,
    r: Value,
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    use BinOp::*;

    if let Some(v) = try_overload(op, &l, &r, &span)? {
        return Ok(v);
    }

    match op {
        Add | Sub | Mul | Div | Mod | Pow => arithmetic(op, l, r, span),

        Eq => Ok(Value::Bool(values_equal(&l, &r))),
        NotEq => Ok(Value::Bool(!values_equal(&l, &r))),

        Lt | LtEq | Gt | GtEq => comparison(op, l, r, span),

        // String-to-string is the overwhelmingly common shape, and the
        // generic path below would clone both operands via
        // `to_display_string` and then allocate a third buffer for the
        // result. Build the result directly instead.
        Concat => match (&l, &r) {
            (Value::Str(a), Value::Str(b)) => {
                let mut s = String::with_capacity(a.len() + b.len());
                s.push_str(a);
                s.push_str(b);
                Ok(Value::Str(Rc::new(s)))
            }
            // Either side may be an instance rendering itself through
            // `OpToString`, so go through `display_value` rather than
            // `to_display_string` directly.
            _ => Ok(Value::Str(Rc::new(format!(
                "{}{}",
                display_value(&l, span.clone())?,
                display_value(&r, span)?
            )))),
        },

        Coalesce | And | Or => unreachable!("and/or/?? are short-circuited in expr::eval"),
    }
}

/// Offer `op` to an operand's class. Returns `Ok(None)` when neither side
/// overloads it, leaving the built-in numeric/string behaviour in charge.
fn try_overload(
    op: BinOp,
    l: &Value,
    r: &Value,
    span: &std::ops::Range<usize>,
) -> Result<Option<Value>, RuntimeError> {
    use BinOp::*;
    let Some(contract) = binary_contract(op) else {
        return Ok(None);
    };
    let method = contract.method;

    // A `nil` operand never reaches an overload. `x == nil` is how
    // nullability is checked and has to keep answering that question
    // rather than calling `equals(other: Vec2)` with nothing in hand; the
    // rest report a built-in type error, which says more than whatever a
    // method body would do with a `nil`. Lua restricts `__eq` the same way.
    if matches!(l, Value::Nil) || matches!(r, Value::Nil) {
        return Ok(None);
    }

    match op {
        // `==` is symmetric, so either operand may carry the overload.
        Eq | NotEq => {
            let (receiver, other) = if has_overload(l, method) {
                (l, r)
            } else if has_overload(r, method) {
                (r, l)
            } else {
                return Ok(None);
            };
            let out = call_overload(receiver, method, vec![other.clone()], span.clone())?;
            let equal = out.is_truthy();
            Ok(Some(Value::Bool(if matches!(op, Eq) {
                equal
            } else {
                !equal
            })))
        }

        // `compare(other)` returns a negative / zero / positive `integer`,
        // so one method covers all four ordering operators. When only the
        // *right* operand implements it, compare the other way round and
        // flip the result — a total order reversed is still a total order.
        Lt | LtEq | Gt | GtEq => {
            let (out, flipped) = if has_overload(l, method) {
                (
                    call_overload(l, method, vec![r.clone()], span.clone())?,
                    false,
                )
            } else if has_overload(r, method) {
                (
                    call_overload(r, method, vec![l.clone()], span.clone())?,
                    true,
                )
            } else {
                return Ok(None);
            };
            let Value::Int(n) = out else {
                return Err(RuntimeError::TypeError {
                    message: format!(
                        "`compare()` must return an `integer` (negative, zero, or positive), got `{}`",
                        out.type_name()
                    ),
                    span: span.clone(),
                });
            };
            let n = if flipped { n.wrapping_neg() } else { n };
            Ok(Some(Value::Bool(match op {
                Lt => n < 0,
                LtEq => n <= 0,
                Gt => n > 0,
                GtEq => n >= 0,
                _ => unreachable!(),
            })))
        }

        // Arithmetic and `..` are not symmetric, so they dispatch on the
        // left operand only — dispatching on the right would silently turn
        // `2 - vec` into `vec - 2`.
        _ => {
            if has_overload(l, method) {
                return Ok(Some(call_overload(
                    l,
                    method,
                    vec![r.clone()],
                    span.clone(),
                )?));
            }
            // `..` has a second chance: an `OpToString` class stringifies
            // itself and falls through to ordinary concatenation.
            if matches!(op, Concat) {
                return Ok(None);
            }
            if has_overload(r, method) {
                return Err(RuntimeError::TypeError {
                    message: format!(
                        "`{}` overloads `{}`, but `{}` dispatches on its left operand, which is a `{}` — \
                         put the `{}` value on the left",
                        class_name(r),
                        binop_symbol(op),
                        binop_symbol(op),
                        describe(l),
                        class_name(r)
                    ),
                    span: span.clone(),
                });
            }
            Ok(None)
        }
    }
}

fn arithmetic(
    op: BinOp,
    l: Value,
    r: Value,
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    match (l, r) {
        (Value::Int(a), Value::Int(b)) => int_op(op, a, b, span),
        (Value::Float(a), Value::Float(b)) => Ok(Value::Float(float_op(op, a, b))),
        // README: mixing `integer` and `float` is a compile error. We
        // surface it at runtime since the typechecker isn't built yet.
        (Value::Int(_), Value::Float(_)) | (Value::Float(_), Value::Int(_)) => {
            Err(RuntimeError::NumericMix { span })
        }
        (a, b) => Err(RuntimeError::TypeError {
            message: format!(
                "arithmetic requires numbers but got `{}` and `{}` — use int()/float() to convert, \
                 or implement `{}` on the class",
                describe(&a),
                describe(&b),
                binary_contract(op).map_or("OpAdd", |c| c.interface)
            ),
            span,
        }),
    }
}

fn int_op(op: BinOp, a: i64, b: i64, span: std::ops::Range<usize>) -> Result<Value, RuntimeError> {
    use BinOp::*;
    match op {
        Add => Ok(Value::Int(a.wrapping_add(b))),
        Sub => Ok(Value::Int(a.wrapping_sub(b))),
        Mul => Ok(Value::Int(a.wrapping_mul(b))),
        Div => {
            if b == 0 {
                Err(RuntimeError::DivisionByZero { span })
            } else {
                Ok(Value::Int(a.wrapping_div(b)))
            }
        }
        Mod => {
            if b == 0 {
                Err(RuntimeError::DivisionByZero { span })
            } else {
                Ok(Value::Int(a.wrapping_rem(b)))
            }
        }
        // `integer ^ integer` stays an integer, the same way `integer /
        // integer` stays an integer. A negative exponent has no integer
        // answer, so it's an error rather than a silent 0.
        Pow => {
            let Ok(exp) = u32::try_from(b) else {
                return Err(RuntimeError::TypeError {
                    message: format!(
                        "`^` on integers requires a non-negative exponent, got {b} — \
                         use floats (`float(base) ^ {b}.0`) for a fractional result"
                    ),
                    span,
                });
            };
            Ok(Value::Int(a.wrapping_pow(exp)))
        }
        _ => unreachable!(),
    }
}

fn float_op(op: BinOp, a: f64, b: f64) -> f64 {
    use BinOp::*;
    match op {
        Add => a + b,
        Sub => a - b,
        Mul => a * b,
        Div => a / b,
        Mod => a % b,
        Pow => a.powf(b),
        _ => unreachable!(),
    }
}

fn comparison(
    op: BinOp,
    l: Value,
    r: Value,
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    use BinOp::*;
    use std::cmp::Ordering;

    let ord: Ordering = match (&l, &r) {
        (Value::Int(a), Value::Int(b)) => a.cmp(b),
        (Value::Float(a), Value::Float(b)) => {
            a.partial_cmp(b).ok_or_else(|| RuntimeError::TypeError {
                message:
                    "cannot compare NaN (not-a-number) — comparison is undefined for NaN values"
                        .into(),
                span: span.clone(),
            })?
        }
        (Value::Str(a), Value::Str(b)) => a.cmp(b),
        (Value::Int(_), Value::Float(_)) | (Value::Float(_), Value::Int(_)) => {
            return Err(RuntimeError::NumericMix { span });
        }
        (a, b) => {
            return Err(RuntimeError::TypeError {
                message: format!(
                    "cannot compare `{}` with `{}` — comparison works on numbers, strings, \
                     or classes implementing `OpCompare`",
                    describe(a),
                    describe(b)
                ),
                span,
            });
        }
    };

    use Ordering::*;
    Ok(Value::Bool(matches!(
        (op, ord),
        (Lt, Less) | (LtEq, Less | Equal) | (Gt, Greater) | (GtEq, Greater | Equal)
    )))
}

pub fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Nil, Value::Nil) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Table(x), Value::Table(y)) => std::rc::Rc::ptr_eq(x, y),
        (Value::Native(x), Value::Native(y)) => std::rc::Rc::ptr_eq(x, y),
        (Value::Function(x), Value::Function(y)) => std::rc::Rc::ptr_eq(x, y),
        (Value::Class(x), Value::Class(y)) => std::rc::Rc::ptr_eq(x, y),
        (Value::Instance(x), Value::Instance(y)) => std::rc::Rc::ptr_eq(x, y),
        (Value::EnumVariant(x), Value::EnumVariant(y)) => std::rc::Rc::ptr_eq(x, y),
        (Value::Enum(x), Value::Enum(y)) => std::rc::Rc::ptr_eq(x, y),
        // Cross-type comparisons are always false (Lua semantics).
        _ => false,
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Stringification
// ──────────────────────────────────────────────────────────────────────────────

/// Render `v` the way `tostring`, `print`, and `..` should see it: through
/// the class's `OpToString` overload when it has one, and through
/// [`Value::to_display_string`] otherwise.
///
/// The overload applies to the value itself, not to values nested inside a
/// table — printing a `table` still uses the structural rendering.
pub fn display_value(v: &Value, span: std::ops::Range<usize>) -> Result<String, RuntimeError> {
    if !has_overload(v, "toString") {
        return Ok(v.to_display_string());
    }
    let out = call_overload(v, "toString", Vec::new(), span.clone())?;
    match out {
        Value::Str(s) => Ok((*s).clone()),
        other => Err(RuntimeError::TypeError {
            message: format!(
                "`{}.toString()` must return a `string`, got `{}`",
                class_name(v),
                other.type_name()
            ),
            span,
        }),
    }
}

/// [`display_value`] for native builtins, which report failures as plain
/// strings and have no span to attribute them to.
pub fn display_value_native(v: &Value) -> Result<String, String> {
    display_value(v, 0..0).map_err(|e| e.to_string())
}

/// Type name for diagnostics — the class name for instances (`Vec2`),
/// which is far more useful than the bare `instance`.
fn describe(v: &Value) -> String {
    match v {
        Value::Instance(_) => class_name(v),
        other => other.type_name().to_string(),
    }
}
