//! Operator implementations shared by expression evaluation.
//!
//! Every operator first offers itself to the operand's class through the
//! built-in `Op*` interfaces (see `stdlib::ops`) — the Saule equivalent of
//! Lua's `__add`, `__sub`, `__concat`, … metamethods. Dispatch here is
//! duck-typed: we look the contract method up on the instance's class chain
//! and call it if present. The `implements OpAdd<…>` clause is what
//! `saule-typeck` enforces statically; the runtime only needs the method.


use saule_ast::ops::{binary_contract, binop_symbol, unary_contract};
use saule_ast::{BinOp, UnaryOp};

use crate::error::RuntimeError;
use crate::eval::expr::{EvaluatedArg, invoke_method_multi};
use crate::value::{SauleStr, Value};

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
        // `~x` — one's complement. Integers only: a bit pattern is not a
        // property a `float` has, and Saule never coerces between the two.
        UnaryOp::BNot => match v {
            Value::Int(n) => Ok(Value::Int(!n)),
            other => Err(RuntimeError::TypeError {
                message: format!(
                    "cannot take the bitwise complement of a `{}` — `~` works on `integer`, \
                     or a class implementing `OpBNot`",
                    describe(&other)
                ),
                span,
            }),
        },
        UnaryOp::Len => match v {
            Value::Str(s) => Ok(Value::Int(crate::stdlib::string::char_len(s.as_str()) as i64)),
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

    // Only an instance can carry an operator overload — `has_overload` returns
    // `false` for every other variant, so every `Some`/`Err` path inside
    // `try_overload` is already gated on one. Checking here instead means the
    // overwhelmingly common `int + int` / `float * float` shapes skip the
    // contract lookup and the per-operator match entirely.
    if (matches!(l, Value::Instance(_)) || matches!(r, Value::Instance(_)))
        && let Some(v) = try_overload(op, &l, &r, &span)?
    {
        return Ok(v);
    }

    match op {
        Add | Sub | Mul | Div | Mod | Pow => arithmetic(op, l, r, span),

        BAnd | BOr | BXor | Shl | Shr => bitwise(op, l, r, span),

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
                Ok(Value::Str(SauleStr::new(s)))
            }
            _ => concat_mixed(&l, &r, span),
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
        // README: mixing `integer` and `float` is a compile error, and
        // `saule-typeck` does reject it. This arm still exists because the
        // low-level `run()` entry point can execute an unchecked module.
        (Value::Int(_), Value::Float(_)) | (Value::Float(_), Value::Int(_)) => {
            Err(RuntimeError::NumericMix { span })
        }
        (a, b) => Err(RuntimeError::TypeError {
            message: format!(
                "arithmetic requires numbers but got `{}` and `{}` — cast with `as integer` \
                 / `as float`, or implement `{}` on the class",
                describe(&a),
                describe(&b),
                binary_contract(op).map_or("OpAdd", |c| c.interface)
            ),
            span,
        }),
    }
}

/// Integer arithmetic. `integer` is an `i64` and **overflow wraps** — the
/// documented choice (README, "Integer Overflow"), matching Lua 5.4.
///
/// `wrapping_*` is deliberate rather than incidental: the plain operators
/// would panic in a debug build and wrap in release, so the same program
/// would behave differently depending on how the toolchain was compiled.
/// Wrapping everywhere at least makes the semantics one thing.
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
                         use floats (`(base as float) ^ {b}.0`) for a fractional result"
                    ),
                    span,
                });
            };
            Ok(Value::Int(a.wrapping_pow(exp)))
        }
        _ => unreachable!(),
    }
}

/// `&` `|` `~` `<<` `>>` — integers only.
///
/// A `float` is rejected outright rather than accepted-when-integral (Lua
/// 5.3's rule), because Saule already refuses to mix the two kinds anywhere
/// else; `float(x) & 1` silently working would be the odd one out.
fn bitwise(
    op: BinOp,
    l: Value,
    r: Value,
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    use BinOp::*;
    let (Value::Int(a), Value::Int(b)) = (&l, &r) else {
        return Err(RuntimeError::TypeError {
            message: format!(
                "`{}` requires `integer` operands but got `{}` and `{}` — cast with \
                 `as integer`, or implement `{}` on the class",
                binop_symbol(op),
                describe(&l),
                describe(&r),
                binary_contract(op).map_or("OpBAnd", |c| c.interface)
            ),
            span,
        });
    };
    let (a, b) = (*a, *b);
    Ok(Value::Int(match op {
        BAnd => a & b,
        BOr => a | b,
        BXor => a ^ b,
        Shl => shift(a, b),
        // A right shift is a left shift by the negated count, and negating
        // covers `i64::MIN` correctly on its own: `wrapping_neg` leaves it
        // as `i64::MIN`, which `shift` already reads as "everything shifted
        // out".
        Shr => shift(a, b.wrapping_neg()),
        _ => unreachable!(),
    }))
}

/// Shift `a` left by `n`, following Lua 5.3 exactly:
///
/// * the vacated bits are filled with **zeros**, so `>>` is a *logical*
///   shift — `-1 >> 1` is a large positive number, not `-1`. Use division
///   when you want the sign-preserving one;
/// * a negative `n` shifts the other way, which is what lets `>>` be
///   expressed as a negated `<<`;
/// * `|n| >= 64` is `0`, since every bit really has been shifted out. This
///   is the case a bare Rust `<<` would panic on.
fn shift(a: i64, n: i64) -> i64 {
    if !(-63..=63).contains(&n) {
        return 0;
    }
    if n >= 0 {
        ((a as u64) << n) as i64
    } else {
        ((a as u64) >> -n) as i64
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

/// `..` where at least one side is not already a string.
///
/// Either side may be an instance rendering itself through `OpToString`, so
/// this goes through the `display_*` helpers rather than
/// [`Value::to_display_string`] directly. Appending both into one buffer is
/// the same saving the `Str`/`Str` arm in [`binary`] is written for —
/// `"key" .. i` lands here, not there.
///
/// Out of line on purpose, for the reason `saule_vm`'s `Vm::concat` is:
/// [`binary`] is the tree-walker's operator hot path, run for every `+` and
/// every `<` in the program, and a `String` plus its capacity arithmetic
/// and drop paths living in one arm widens the frame that all of the other
/// arms are compiled under. Inlined here it cost ~9% on `closure`, a
/// benchmark that never concatenates anything.
#[inline(never)]
fn concat_mixed(l: &Value, r: &Value, span: std::ops::Range<usize>) -> Result<Value, RuntimeError> {
    let mut s = String::with_capacity(display_hint(l) + display_hint(r));
    display_into(l, span.clone(), &mut s)?;
    display_into(r, span, &mut s)?;
    Ok(Value::Str(SauleStr::new(s)))
}

/// [`display_value`] straight into an existing buffer.
///
/// `..` is building a `String` it already owns, so the `String` that
/// `display_value` returns is pure overhead on the way there: `"key" .. i`
/// allocated one for the left operand's clone and another for the right
/// operand's digits, only to copy both into the result and drop them.
/// Appending in place skips all of that.
///
/// Equivalent to `out.push_str(&display_value(v, span)?)` for every input.
/// The scalar arms are a shortcut and not a second rendering rule —
/// [`has_overload`] answers `false` for everything except
/// [`Value::Instance`], so no value handled here can have an `OpToString`
/// to honour, and anything that might still goes the long way round.
pub fn display_into(
    v: &Value,
    span: std::ops::Range<usize>,
    out: &mut String,
) -> Result<(), RuntimeError> {
    match v {
        Value::Str(s) => out.push_str(s),
        Value::Int(n) => crate::itoa::push_i64(out, *n),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Nil => out.push_str("nil"),
        other => out.push_str(&display_value(other, span)?),
    }
    Ok(())
}

/// A lower bound on the room `v` will need in [`display_into`], for sizing
/// the buffer before the appends start.
///
/// Deliberately inspects nothing that can run user code — an instance's
/// `toString` must be called exactly once, by `display_into` itself, so
/// this only guesses at the shapes it can measure for free and leaves the
/// rest to `String`'s own growth.
pub fn display_hint(v: &Value) -> usize {
    match v {
        Value::Str(s) => s.len(),
        // Wide enough for any `i64`, sign included.
        Value::Int(_) => 20,
        Value::Bool(_) => 5,
        Value::Nil => 3,
        _ => 16,
    }
}

/// Type name for diagnostics — the class name for instances (`Vec2`),
/// which is far more useful than the bare `instance`.
fn describe(v: &Value) -> String {
    match v {
        Value::Instance(_) => class_name(v),
        other => other.type_name().to_string(),
    }
}
