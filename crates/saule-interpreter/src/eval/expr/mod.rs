//! Expression evaluation.
//!
//! The entry point is [`eval`]. Heavyweight pieces have been moved into
//! sibling modules:
//!
//! | Module        | Contents                                              |
//! |---------------|-------------------------------------------------------|
//! | [`calls`]     | call/dispatch/super machinery, `bind_params`          |
//! | [`construct`] | `new ClassName(...)`, tuple-variant constructors      |
//! | [`match_`]    | `match` evaluation and pattern matching               |
//! | [`members`]   | `obj.field` and `obj[index]` reads                    |

mod calls;
mod construct;
mod match_;
mod members;

pub(crate) use calls::{call_function, call_static_method_public, call_value_multi, eval_values, invoke_method_multi};
#[allow(unused_imports)]
pub(crate) use members::table_index_to_slot;

use std::cell::RefCell;
use std::rc::Rc;

use saule_ast::{BinOp, Expr, LambdaBody, Spanned, TableEntry, Type};

use crate::env::Environment;
use crate::error::RuntimeError;
use crate::value::{FunctionBody, FunctionObject, Value};

use super::ops;

/// Scope binding key used to remember which class a method body is
/// "speaking for" when it calls `self.super(...)`.
const SUPER_OWNER_BINDING: &str = "__saule_super_owner";

#[derive(Clone)]
pub(crate) enum EvaluatedArg {
    Positional(Value),
    Named { name: String, value: Value },
}

pub fn eval(expr: &Spanned<Expr>, env: &Rc<RefCell<Environment>>) -> Result<Value, RuntimeError> {
    let span = expr.span.clone();
    match &expr.value {
        Expr::Int(n) => Ok(Value::Int(*n)),
        Expr::Float(f) => Ok(Value::Float(*f)),
        Expr::Bool(b) => Ok(Value::Bool(*b)),
        Expr::Str(s) => Ok(Value::Str(Rc::new(s.clone()))),
        Expr::Nil => Ok(Value::Nil),

        Expr::Ident(name) => env
            .borrow()
            .get(name)
            .ok_or_else(|| RuntimeError::Undefined {
                name: name.clone(),
                span,
            }),

        Expr::Unary { op, rhs } => {
            let v = eval(rhs, env)?;
            ops::unary(*op, v, span)
        }

        Expr::Binary { op, lhs, rhs } => match op {
            // `and` / `or` short-circuit, so evaluate lazily.
            BinOp::And => {
                let l = eval(lhs, env)?;
                if l.is_truthy() { eval(rhs, env) } else { Ok(l) }
            }
            BinOp::Or => {
                let l = eval(lhs, env)?;
                if l.is_truthy() { Ok(l) } else { eval(rhs, env) }
            }
            // `??` short-circuits: only evaluate RHS when LHS is nil.
            BinOp::Coalesce => {
                let l = eval(lhs, env)?;
                if matches!(l, Value::Nil) { eval(rhs, env) } else { Ok(l) }
            }
            _ => {
                let l = eval(lhs, env)?;
                let r = eval(rhs, env)?;
                ops::binary(*op, l, r, span)
            }
        },

        Expr::Call { callee, args } => {
            // Dotted calls (`obj.method(args)`) and `self.super(args)` need
            // special handling so we can auto-bind `self` and route to the
            // parent constructor respectively.
            if let Expr::Member { obj, name } = &callee.value {
                if name == "super" {
                    return calls::super_call(obj, args, env, span);
                }
                let receiver = eval(obj, env)?;
                let vs = calls::eval_call_args_pub(args, env)?;
                return calls::dispatch_member_call(&receiver, name, vs, span);
            }

            let cv = eval(callee, env)?;
            let vs = calls::eval_call_args_pub(args, env)?;
            calls::call_value_pub(cv, &vs, span)
        }

        Expr::Member { obj, name } => {
            let receiver = eval(obj, env)?;
            members::read_member(&receiver, name, span)
        }
        Expr::SafeMember { obj, name } => {
            let receiver = eval(obj, env)?;
            if matches!(receiver, Value::Nil) {
                Ok(Value::Nil)
            } else {
                members::read_member(&receiver, name, span)
            }
        }
        Expr::Index { obj, index } => {
            let receiver = eval(obj, env)?;
            let index_value = eval(index, env)?;
            members::read_index(&receiver, index_value, span)
        }
        Expr::MethodCall { obj, method, args } => {
            let receiver = eval(obj, env)?;
            let evaled = calls::eval_call_args_pub(args, env)?;
            calls::invoke_method(&receiver, method, evaled, span)
        }
        Expr::ForceUnwrap(inner) => {
            let v = eval(inner, env)?;
            if matches!(v, Value::Nil) {
                Err(RuntimeError::ForceUnwrapNil { span })
            } else {
                Ok(v)
            }
        }
        Expr::Table(items) => {
            // Build the array part from positional entries in order, then
            // apply field entries via `set` so the existing array/map split
            // logic (positive contiguous ints land in the array part) is
            // reused uniformly.
            let mut table = crate::value::TableObject::new();
            for item in items {
                match item {
                    TableEntry::Positional(e) => {
                        let v = eval(e, env)?;
                        table.array.push(v);
                    }
                    TableEntry::Field { key, value } => {
                        let k = eval(key, env)?;
                        let v = eval(value, env)?;
                        table.set(&k, v).map_err(|msg| RuntimeError::TypeError {
                            message: msg,
                            span: key.span.clone(),
                        })?;
                    }
                }
            }
            Ok(Value::Table(Rc::new(RefCell::new(table))))
        }

        Expr::Lambda { params, body, .. } => {
            let body = match body {
                LambdaBody::Expr(e) => FunctionBody::Expr(e.clone()),
                LambdaBody::Block(stmts) => FunctionBody::Block(stmts.clone()),
            };
            Ok(Value::Function(Rc::new(FunctionObject {
                name: None,
                params: params.clone(),
                body,
                closure: env.clone(),
                owner_class: std::cell::RefCell::new(None),
                source: crate::module::active_module_source(),
            })))
        }

        Expr::Self_ => env
            .borrow()
            .get("self")
            .ok_or_else(|| RuntimeError::Undefined {
                name: "self".to_string(),
                span,
            }),

        Expr::Match { scrutinee, arms } => match_::eval_match(scrutinee, arms, env, span),
    }
}

fn first_or_nil(values: Vec<Value>) -> Value {
    values.into_iter().next().unwrap_or(Value::Nil)
}

fn is_nullable_type(ty: &Type) -> bool {
    matches!(ty, Type::Nullable(_)) || matches!(ty, Type::Named(n) if n == "nil")
}
