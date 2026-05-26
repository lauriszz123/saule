//! Operator implementations shared by expression evaluation.

use std::rc::Rc;

use saule_ast::{BinOp, UnaryOp};

use crate::error::RuntimeError;
use crate::value::Value;

pub fn unary(op: UnaryOp, v: Value, span: std::ops::Range<usize>) -> Result<Value, RuntimeError> {
    match op {
        UnaryOp::Neg => match v {
            Value::Int(n) => Ok(Value::Int(-n)),
            Value::Float(f) => Ok(Value::Float(-f)),
            other => Err(RuntimeError::TypeError {
                message: format!("cannot negate `{}`", other.type_name()),
                span,
            }),
        },
        UnaryOp::Not => Ok(Value::Bool(!v.is_truthy())),
        UnaryOp::Len => match v {
            Value::Str(s) => Ok(Value::Int(s.chars().count() as i64)),
            other => Err(RuntimeError::TypeError {
                message: format!("cannot take length of `{}`", other.type_name()),
                span,
            }),
        },
    }
}

pub fn binary(
    op: BinOp,
    l: Value,
    r: Value,
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    use BinOp::*;
    match op {
        Add | Sub | Mul | Div | Mod => arithmetic(op, l, r, span),

        Eq => Ok(Value::Bool(values_equal(&l, &r))),
        NotEq => Ok(Value::Bool(!values_equal(&l, &r))),

        Lt | LtEq | Gt | GtEq => comparison(op, l, r, span),

        Concat => Ok(Value::Str(Rc::new(format!(
            "{}{}",
            l.to_display_string(),
            r.to_display_string()
        )))),

        Coalesce => Ok(if matches!(l, Value::Nil) { r } else { l }),

        And | Or => unreachable!("and/or are short-circuited in expr::eval"),
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
                "arithmetic requires numbers, got `{}` and `{}`",
                a.type_name(),
                b.type_name()
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
        (Value::Float(a), Value::Float(b)) => a
            .partial_cmp(b)
            .ok_or_else(|| RuntimeError::TypeError {
                message: "NaN in comparison".into(),
                span: span.clone(),
            })?,
        (Value::Str(a), Value::Str(b)) => a.cmp(b),
        (Value::Int(_), Value::Float(_)) | (Value::Float(_), Value::Int(_)) => {
            return Err(RuntimeError::NumericMix { span });
        }
        (a, b) => {
            return Err(RuntimeError::TypeError {
                message: format!("cannot compare `{}` with `{}`", a.type_name(), b.type_name()),
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
        // Cross-type comparisons are always false (Lua semantics).
        _ => false,
    }
}
