//! Statement execution with control-flow propagation.
//!
//! The entry points are [`exec_block`] and [`exec`]. Heavyweight pieces
//! have been moved into sibling modules:
//!
//! | Module      | Contents                                            |
//! |-------------|-----------------------------------------------------|
//! | [`assign`]  | `=` / parallel `=` for ident / member / index       |
//! | [`classes`] | `class` and `interface` decls                       |
//! | [`enums`]   | `enum` decls                                        |
//! | [`imports`] | `import … from "path"`                              |
//! | [`loops`]   | numeric `for` and `for … in`                        |
//! | [`try_`]    | `try … catch` with runtime type matching            |

mod assign;
mod classes;
mod enums;
mod imports;
mod loops;
mod try_;

use std::cell::RefCell;
use std::rc::Rc;

use saule_ast::{Decl, Expr, Param, Spanned, Stmt};

use crate::env::Environment;
use crate::error::RuntimeError;
use crate::value::{FunctionBody, FunctionObject, Value};

use super::{Flow, expr};

/// Park the in-flight thrown `Value` so `RuntimeError::Thrown` can stay
/// `Send + Sync` (miette's requirement) while the actual value — which
/// contains non-`Send` `Rc`s — rides alongside in a thread-local slot.
pub(super) mod thrown_slot {
    use crate::value::Value;
    use std::cell::RefCell;

    thread_local! {
        static SLOT: RefCell<Option<Value>> = const { RefCell::new(None) };
    }

    pub fn set(v: Value) {
        SLOT.with(|s| *s.borrow_mut() = Some(v));
    }

    pub fn take() -> Option<Value> {
        SLOT.with(|s| s.borrow_mut().take())
    }
}

/// Execute a sequence of statements in `env`. Stops at the first non-`Normal`
/// outcome and propagates it.
pub fn exec_block(
    stmts: &[Spanned<Stmt>],
    env: &Rc<RefCell<Environment>>,
) -> Result<Flow, RuntimeError> {
    let mut last = Flow::nil();
    for stmt in stmts {
        match exec(stmt, env)? {
            Flow::Normal(v) => last = Flow::Normal(v),
            other => return Ok(other),
        }
    }
    Ok(last)
}

/// Run a block in a fresh child scope. The scope is dropped on return.
fn exec_scoped_block(
    stmts: &[Spanned<Stmt>],
    parent: &Rc<RefCell<Environment>>,
) -> Result<Flow, RuntimeError> {
    let scope = Environment::with_parent(parent.clone());
    exec_block(stmts, &scope)
}

/// Execute a single statement.
pub fn exec(stmt: &Spanned<Stmt>, env: &Rc<RefCell<Environment>>) -> Result<Flow, RuntimeError> {
    let span = stmt.span.clone();
    match &stmt.value {
        Stmt::Local { name, value, .. } => {
            let v = match value {
                Some(e) => expr::eval(e, env)?,
                None => Value::Nil,
            };
            env.borrow_mut().define(name.clone(), v);
            Ok(Flow::nil())
        }

        Stmt::LocalMulti { names, values } => {
            // Evaluate every RHS first so `local a, b = b, a` works at the
            // outer scope. The final expression may expand into multiple
            // return values (Lua-style destructuring semantics).
            let evaluated = eval_expr_list(values, env)?;
            for (i, (name, _)) in names.iter().enumerate() {
                let v = evaluated.get(i).cloned().unwrap_or(Value::Nil);
                env.borrow_mut().define(name.clone(), v);
            }
            Ok(Flow::nil())
        }

        Stmt::Assign { target, value } => assign::exec_assign(target, value, env),

        Stmt::AssignMulti { targets, values } => {
            // Evaluate all RHS expressions first to support parallel
            // semantics (e.g. `a, b = b, a + b`).
            let evaluated = eval_expr_list(values, env)?;
            for (i, target) in targets.iter().enumerate() {
                let v = evaluated.get(i).cloned().unwrap_or(Value::Nil);
                assign::assign_target(target, v, env)?;
            }
            Ok(Flow::nil())
        }

        Stmt::Expr(e) => Ok(Flow::Normal(expr::eval(e, env)?)),

        Stmt::If {
            cond,
            then_block,
            elseifs,
            else_block,
        } => {
            if expr::eval(cond, env)?.is_truthy() {
                return exec_scoped_block(then_block, env);
            }
            for (econd, ebody) in elseifs {
                if expr::eval(econd, env)?.is_truthy() {
                    return exec_scoped_block(ebody, env);
                }
            }
            if let Some(eb) = else_block {
                return exec_scoped_block(eb, env);
            }
            Ok(Flow::nil())
        }

        Stmt::While { cond, body } => {
            while expr::eval(cond, env)?.is_truthy() {
                match exec_scoped_block(body, env)? {
                    Flow::Normal(_) | Flow::Continue => continue,
                    Flow::Break => break,
                    ret @ Flow::Return(_) => return Ok(ret),
                }
            }
            Ok(Flow::nil())
        }

        Stmt::Repeat { body, cond } => {
            // Lua semantics: the `until` condition sees locals declared in
            // the body, so condition and body must share the same scope.
            loop {
                let scope = Environment::with_parent(env.clone());
                match exec_block(body, &scope)? {
                    Flow::Normal(_) | Flow::Continue => {}
                    Flow::Break => break,
                    ret @ Flow::Return(_) => return Ok(ret),
                }
                if expr::eval(cond, &scope)?.is_truthy() {
                    break;
                }
            }
            Ok(Flow::nil())
        }

        Stmt::ForNumeric {
            var,
            var_ty: _,
            from,
            to,
            step,
            body,
        } => loops::exec_for_numeric(var, from, to, step.as_ref(), body, env, span),

        Stmt::ForIn { vars, iter, body } => loops::exec_for_in(vars, iter, body, env, span),

        Stmt::Return(exprs) => {
            let values = if exprs.is_empty() {
                vec![Value::Nil]
            } else {
                eval_expr_list(exprs, env)?
            };
            Ok(Flow::Return(values))
        }

        Stmt::Break => Ok(Flow::Break),
        Stmt::Continue => Ok(Flow::Continue),

        Stmt::Throw(e) => {
            let v = expr::eval(e, env)?;
            let display = v.to_display_string();
            thrown_slot::set(v);
            Err(RuntimeError::Thrown {
                value: display,
                span,
            })
        }
        Stmt::Try {
            body,
            catch_var,
            catch_ty,
            catch_body,
        } => try_::exec_try(body, catch_var, catch_ty, catch_body, env),
        Stmt::Decl(decl) => exec_decl(decl, env),
    }
}

fn eval_expr_list(
    exprs: &[Spanned<Expr>],
    env: &Rc<RefCell<Environment>>,
) -> Result<Vec<Value>, RuntimeError> {
    let mut out = Vec::new();
    for (i, expr_node) in exprs.iter().enumerate() {
        if i + 1 == exprs.len() {
            out.extend(expr::eval_values(expr_node, env)?);
        } else {
            out.push(expr::eval(expr_node, env)?);
        }
    }
    Ok(out)
}

fn exec_decl(
    decl: &Spanned<Decl>,
    env: &Rc<RefCell<Environment>>,
) -> Result<Flow, RuntimeError> {
    let span = decl.span.clone();
    match &decl.value {
        Decl::Function {
            name, params, body, ..
        } => {
            let func = FunctionObject {
                name: Some(name.clone()),
                params: params.clone(),
                body: FunctionBody::Block(body.clone()),
                closure: env.clone(),
                owner_class: std::cell::RefCell::new(None),
                source: crate::module::active_module_source(),
            };
            env.borrow_mut()
                .define(name.clone(), Value::Function(Rc::new(func)));
            Ok(Flow::nil())
        }
        Decl::Class { .. } => classes::exec_class_decl(decl, env),
        Decl::Interface { .. } => classes::exec_interface_decl(decl, env),
        Decl::Enum {
            name,
            variants,
            methods,
            ..
        } => enums::exec_enum_decl(name, variants, methods, env, span),
        Decl::Import { names, path } => imports::exec_import(names, path, env, span),
    }
}

/// Shared factory used by class/enum decl execution to build a
/// [`FunctionObject`] from parsed method pieces.
pub(super) fn make_function(
    name: Option<String>,
    params: Vec<Param>,
    body: Vec<Spanned<Stmt>>,
    closure: &Rc<RefCell<Environment>>,
) -> FunctionObject {
    FunctionObject {
        name,
        params,
        body: FunctionBody::Block(body),
        closure: closure.clone(),
        owner_class: std::cell::RefCell::new(None),
        source: crate::module::active_module_source(),
    }
}
