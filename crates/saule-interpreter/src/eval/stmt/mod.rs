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

/// Park a `Flow` that escaped from inside an expression-context evaluator
/// (e.g. a `return` inside a `match` arm). Same trick as `thrown_slot`:
/// the marker error rides through `Result` while the non-`Send` `Flow`
/// stays in a thread-local. The surrounding `exec` boundary takes it
/// back and resumes normal control-flow propagation.
pub(super) mod pending_flow {
    use super::Flow;
    use std::cell::RefCell;

    thread_local! {
        static SLOT: RefCell<Option<Flow>> = const { RefCell::new(None) };
    }

    pub fn set(f: Flow) {
        SLOT.with(|s| *s.borrow_mut() = Some(f));
    }

    pub fn take() -> Option<Flow> {
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

/// Whether running `stmts` needs a scope of its own.
///
/// Only three statements bind a name into the block they appear in: `local`,
/// its multi-assign form, and a declaration (`fn` / `class` / `interface` /
/// `enum` / `import`). Everything else either binds nothing or builds its own
/// scope — a `for` makes one per iteration for the loop variable, `try` makes
/// one each for the body and the catch.
///
/// A block that binds nothing cannot shadow anything, so reads and writes
/// resolve identically with or without the extra scope. Skipping it matters
/// because the scope is per *iteration*: `while i < n do i = i + 1 end` was
/// allocating an `Rc<RefCell<Environment>>` every time round a loop whose body
/// declares nothing.
fn block_binds_names(stmts: &[Spanned<Stmt>]) -> bool {
    stmts.iter().any(|s| {
        matches!(
            s.value,
            Stmt::Local { .. } | Stmt::LocalMulti { .. } | Stmt::Decl(_)
        )
    })
}

/// Run a block in a fresh child scope. The scope is dropped on return.
fn exec_scoped_block(
    stmts: &[Spanned<Stmt>],
    parent: &Rc<RefCell<Environment>>,
) -> Result<Flow, RuntimeError> {
    if !block_binds_names(stmts) {
        return exec_block(stmts, parent);
    }
    let scope = Environment::with_parent(parent.clone());
    let flow = exec_block(stmts, &scope);
    Environment::release(scope);
    flow
}

/// Execute a single statement.
pub fn exec(stmt: &Spanned<Stmt>, env: &Rc<RefCell<Environment>>) -> Result<Flow, RuntimeError> {
    match exec_inner(stmt, env) {
        // A `return`/`break`/`continue` that escaped through an expression
        // context (e.g. a `match` arm body) parks itself in `pending_flow`
        // and signals via `PendingFlow`. Convert it back into a real
        // `Flow` here so the surrounding block keeps propagating it.
        Err(RuntimeError::PendingFlow { .. }) => Ok(pending_flow::take().unwrap_or(Flow::nil())),
        other => other,
    }
}

fn exec_inner(stmt: &Spanned<Stmt>, env: &Rc<RefCell<Environment>>) -> Result<Flow, RuntimeError> {
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
            for (i, (name, _, _)) in names.iter().enumerate() {
                let v = evaluated.get(i).cloned().unwrap_or(Value::Nil);
                env.borrow_mut().define(name.clone(), v);
            }
            Ok(Flow::nil())
        }

        Stmt::Assign { target, value } => assign::exec_assign(target, value, env),

        Stmt::CompoundAssign { target, op, value } => {
            assign::exec_compound_assign(target, *op, value, env)
        }

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
            // One scope for the whole loop rather than one per iteration: a
            // body that binds nothing needs none at all, and one that does
            // gets the same scope handed back each time round unless a
            // closure captured it. See `Environment::recycle`.
            let mut scope = block_binds_names(body).then(|| Environment::with_parent(env.clone()));
            while expr::eval(cond, env)?.is_truthy() {
                let flow = match &scope {
                    Some(s) => exec_block(body, s)?,
                    None => exec_block(body, env)?,
                };
                if let Some(spent) = scope.take() {
                    scope = Some(Environment::recycle(spent, env));
                }
                match flow {
                    Flow::Normal(_) | Flow::Continue => continue,
                    Flow::Break => break,
                    ret @ Flow::Return(_) => return Ok(ret),
                }
            }
            if let Some(spent) = scope {
                Environment::release(spent);
            }
            Ok(Flow::nil())
        }

        Stmt::Repeat { body, cond } => {
            // Lua semantics: the `until` condition sees locals declared in
            // the body, so condition and body must share the same scope.
            let mut scope = Environment::with_parent(env.clone());
            loop {
                match exec_block(body, &scope)? {
                    Flow::Normal(_) | Flow::Continue => {}
                    Flow::Break => break,
                    ret @ Flow::Return(_) => return Ok(ret),
                }
                if expr::eval(cond, &scope)?.is_truthy() {
                    break;
                }
                scope = Environment::recycle(scope, env);
            }
            Environment::release(scope);
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
                crate::recycle::values_of(Value::Nil)
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
    let mut out = crate::recycle::take_values();
    for (i, expr_node) in exprs.iter().enumerate() {
        if i + 1 == exprs.len() {
            // `append` rather than `extend` so the carrier the call returned
            // can go back to the free list instead of being dropped — see
            // [`crate::recycle`]. `return f(x)` takes this path.
            let mut tail = expr::eval_values(expr_node, env)?;
            out.append(&mut tail);
            crate::recycle::give_values(tail);
        } else {
            out.push(expr::eval(expr_node, env)?);
        }
    }
    Ok(out)
}

fn exec_decl(decl: &Spanned<Decl>, env: &Rc<RefCell<Environment>>) -> Result<Flow, RuntimeError> {
    let span = decl.span.clone();
    match &decl.value {
        Decl::Function {
            name, params, body, ..
        } => {
            let func = FunctionObject {
                name: Some(name.clone()),
                param_keys: FunctionObject::intern_params(params),
                params: params.clone(),
                body: FunctionBody::Block(body.as_slice().into()),
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
        Decl::Variable { name, value, .. } => {
            // No initializer means `nil` — the same rule locals follow. The
            // typechecker only lets that through for a nullable type.
            let v = match value {
                Some(expr_node) => expr::eval(expr_node, env)?,
                None => Value::Nil,
            };
            env.borrow_mut().define(name.clone(), v);
            Ok(Flow::nil())
        }
        Decl::Import { names, path, .. } => imports::exec_import(names, path, env, span),
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
        param_keys: FunctionObject::intern_params(&params),
        params,
        body: FunctionBody::Block(body.into()),
        closure: closure.clone(),
        owner_class: std::cell::RefCell::new(None),
        source: crate::module::active_module_source(),
    }
}
