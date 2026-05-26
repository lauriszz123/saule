//! Statement execution with control-flow propagation.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use saule_ast::{ClassMember, Decl, EnumVariant, Expr, ImportNames, Method, Param, Spanned, Stmt};

use crate::env::Environment;
use crate::error::{RuntimeError, unsupported};
use crate::module;
use crate::value::{self, ClassObject, FieldDef, FunctionBody, FunctionObject, InterfaceObject, Value};

use super::{Flow, expr};

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

        Stmt::Assign { target, value } => exec_assign(target, value, env),

        Stmt::AssignMulti { targets, values } => {
            // Evaluate all RHS expressions first to support parallel
            // semantics (e.g. `a, b = b, a + b`). The final expression may
            // expand into multiple return values.
            let evaluated = eval_expr_list(values, env)?;
            for (i, target) in targets.iter().enumerate() {
                let v = evaluated.get(i).cloned().unwrap_or(Value::Nil);
                assign_target(target, v, env)?;
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
        } => exec_for_numeric(var, from, to, step.as_ref(), body, env, span),

        Stmt::ForIn { vars, iter, body } => exec_for_in(vars, iter, body, env, span),

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

        Stmt::Throw(_) => Err(unsupported("throw", span)),
        Stmt::Try { .. } => Err(unsupported("try/catch", span)),
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

// ─── Declarations ────────────────────────────────────────────────────────────

fn exec_decl(
    decl: &Spanned<Decl>,
    env: &std::rc::Rc<std::cell::RefCell<Environment>>,
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
            };
            env.borrow_mut()
                .define(name.clone(), Value::Function(std::rc::Rc::new(func)));
            Ok(Flow::nil())
        }
        Decl::Class { .. } => exec_class_decl(decl, env),
         Decl::Interface { .. } => exec_interface_decl(decl, env),
         Decl::Enum { name, variants, methods, .. } => exec_enum_decl(name, variants, methods, env, span),
        Decl::Import { names, path } => exec_import(names, path, env, span),
    }
}

/// Materialize a `Decl::Class` into a [`ClassObject`] and install it under
/// the class's name in `env`. Method closures all capture `env` so they can
/// see the class itself (used by static calls) and other top-level names.
fn exec_class_decl(
    decl: &Spanned<Decl>,
    env: &Rc<RefCell<Environment>>,
) -> Result<Flow, RuntimeError> {
    let span = decl.span.clone();
    let Decl::Class {
        name,
        extends,
        implements,
        members,
        ..
    } = &decl.value
    else {
        unreachable!("exec_class_decl called with non-class decl");
    };

    // Resolve parent class, if any. Must already exist in scope.
    let parent = if let Some(pname) = extends {
        match env.borrow().get(pname) {
            Some(Value::Class(c)) => Some(c),
            Some(other) => {
                return Err(RuntimeError::TypeError {
                    message: format!(
                        "cannot extend `{}`: expected a class but got `{}` — check class definition",
                        pname, other.type_name()
                    ),
                    span,
                });
            }
            None => {
                return Err(RuntimeError::Undefined {
                    name: pname.clone(),
                    span,
                });
            }
        }
    } else {
        None
    };

    let mut field_defs: Vec<FieldDef> = Vec::new();
    let mut methods: HashMap<String, Rc<FunctionObject>> = HashMap::new();
    let mut static_fields: HashMap<String, Value> = HashMap::new();
    let mut static_methods: HashMap<String, Rc<FunctionObject>> = HashMap::new();
    let mut constructor: Option<Rc<FunctionObject>> = None;

    // Scan once so we know whether the class has a constructor (`fn init`).
    // When it doesn't, `local field = expr` declarations are promoted to
    // statics so callers can read them via `ClassName.field` (and through
    // the class-as-`self` convention used inside `static fn`s).
    let has_init_method = members.iter().any(|m| match &m.value {
        ClassMember::Method(meth) => meth.name == "init" && !meth.is_static,
        _ => false,
    });

    for member in members {
        match &member.value {
            ClassMember::Field {
                is_static,
                name: fname,
                default,
                ..
            } => {
                // Promote `local field = expr` to a static when there's no
                // constructor — otherwise we'd never be able to read it.
                let treat_as_static = *is_static || (!has_init_method && default.is_some());
                if treat_as_static {
                    // Static defaults are evaluated once, at class
                    // declaration time, in the enclosing scope.
                    let value = match default {
                        Some(e) => expr::eval(e, env)?,
                        None => Value::Nil,
                    };
                    static_fields.insert(fname.clone(), value);
                } else {
                    field_defs.push(FieldDef {
                        name: fname.clone(),
                        default: default.clone(),
                    });
                }
            }
            ClassMember::Method(m) => {
                let func = Rc::new(make_function(
                    Some(format!("{name}.{}", m.name)),
                    m.params.clone(),
                    m.body.clone(),
                    env,
                ));
                if m.is_static {
                    static_methods.insert(m.name.clone(), func);
                } else if m.name == "init" {
                    // `init` is the only constructor spelling — always promote.
                    constructor = Some(func);
                } else {
                    methods.insert(m.name.clone(), func);
                }
            }
        }
    }

    // Validate that all implemented interfaces' required methods are present.
    // Collect ALL missing methods across ALL interfaces so we report them together.
    let mut missing_methods: Vec<(String, String)> = Vec::new(); // (interface_name, method_name)

    for interface_name in implements {
        match env.borrow().get(interface_name) {
            Some(Value::Interface(iface)) => {
                for required_method in &iface.methods {
                    if !methods.contains_key(required_method.0) {
                        missing_methods.push((interface_name.clone(), required_method.0.clone()));
                    }
                }
            }
            Some(_) => {
                return Err(RuntimeError::TypeError {
                    message: format!(
                        "cannot implement `{}`: expected an interface but got something else",
                        interface_name
                    ),
                    span,
                });
            }
            None => {
                return Err(RuntimeError::Undefined {
                    name: interface_name.clone(),
                    span,
                });
            }
        }
    }

    // Report all missing methods at once
    if !missing_methods.is_empty() {
        let missing_list = missing_methods
            .iter()
            .map(|(iface, method)| format!("`{}` from interface `{}`", method, iface))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(RuntimeError::TypeError {
            message: format!(
                "class `{}` is missing method{}: {}",
                name,
                if missing_methods.len() == 1 { "" } else { "s" },
                missing_list
            ),
            span,
        });
    }

    let class = Rc::new(ClassObject {
        name: name.clone(),
        parent,
        field_defs,
        methods,
        static_fields: RefCell::new(static_fields),
        static_methods,
        constructor,
    });

    env.borrow_mut().define(name.clone(), Value::Class(class));
    Ok(Flow::nil())
 }

 /// Execute an interface declaration and install it in the environment.
 fn exec_interface_decl(
     decl: &Spanned<Decl>,
     env: &Rc<RefCell<Environment>>,
 ) -> Result<Flow, RuntimeError> {
     let Decl::Interface {
         name,
         extends,
         methods,
         ..
     } = &decl.value
     else {
         unreachable!("exec_interface_decl called with non-interface decl");
     };

     // Build a map of method signatures from the interface.
     // For now, we just store the method name and parameter count for basic validation.
     let mut method_sigs = HashMap::new();
     for method in methods {
         let param_count = method.params.len();
         let has_return_type = method.return_ty.is_some();
         method_sigs.insert(method.name.clone(), (param_count, has_return_type));
     }

     let interface_obj = Rc::new(InterfaceObject {
         name: name.clone(),
         extends: extends.clone(),
         methods: method_sigs,
     });

     env.borrow_mut()
         .define(name.clone(), Value::Interface(interface_obj));
     Ok(Flow::nil())
 }

 fn make_function(
    name: Option<String>,
    params: Vec<Param>,
    body: Vec<Spanned<Stmt>>,
    closure: &Rc<RefCell<Environment>>,
) -> FunctionObject {
    let _ = closure; // silence unused if we change capture
    FunctionObject {
        name,
        params,
        body: FunctionBody::Block(body),
        closure: closure.clone(),
    }
}

fn exec_enum_decl(
    enum_name: &str,
    variants: &[Spanned<EnumVariant>],
    methods: &[Method],
    env: &Rc<RefCell<Environment>>,
    _span: std::ops::Range<usize>,
) -> Result<Flow, RuntimeError> {
    let mut variant_dict = HashMap::new();
    let mut enum_methods = HashMap::new();

    for method in methods {
        let func = Rc::new(make_function(
            Some(format!("{enum_name}.{}", method.name)),
            method.params.clone(),
            method.body.clone(),
            env,
        ));
        enum_methods.insert(method.name.clone(), func);
    }

    // Create all variants (without enum references initially)
    for variant in variants {
        match &variant.value {
            EnumVariant::Bare(name) => {
                let variant_obj = Rc::new(value::EnumVariantObject {
                    enum_name: enum_name.to_string(),
                    variant_name: name.clone(),
                    value: None,
                    enum_obj: RefCell::new(None),
                });
                variant_dict.insert(name.clone(), variant_obj);
            }
            EnumVariant::Valued(name, expr) => {
                let val = expr::eval(expr, env)?;
                let variant_obj = Rc::new(value::EnumVariantObject {
                    enum_name: enum_name.to_string(),
                    variant_name: name.clone(),
                    value: Some(val),
                    enum_obj: RefCell::new(None),
                });
                variant_dict.insert(name.clone(), variant_obj);
            }
        }
    }

    // Create the final enum object with all variants
    let final_enum = Rc::new(value::EnumObject {
        name: enum_name.to_string(),
        variants: variant_dict.clone(),
        methods: enum_methods,
    });

    // Now update each variant to reference the enum
    for variant in variant_dict.values() {
        *variant.enum_obj.borrow_mut() = Some(final_enum.clone());
    }

    env.borrow_mut()
        .define(enum_name.to_string(), Value::Enum(final_enum));
    Ok(Flow::nil())
}

// ─── Imports ─────────────────────────────────────────────────────────────────

/// Execute `import ... from "path"`:
///   1. Resolve `path` relative to the importing file's directory.
///   2. Load (or fetch cached) exports for that module via the shared
///      [`module::ModuleLoader`].
///   3. Bind the requested names — optionally aliased — into `env`.
fn exec_import(
    names: &ImportNames,
    path: &str,
    env: &Rc<RefCell<Environment>>,
    span: std::ops::Range<usize>,
) -> Result<Flow, RuntimeError> {
    let loader = env.borrow().loader().ok_or_else(|| RuntimeError::ImportError {
        message: "no module loader available — running this file with `saule run` should attach one".to_string(),
        span: span.clone(),
    })?;
    let dir = env.borrow().module_dir().ok_or_else(|| RuntimeError::ImportError {
        message: "cannot resolve relative import: current file has no known directory".to_string(),
        span: span.clone(),
    })?;

    let abs = module::resolve_import_path(&dir, path).ok_or_else(|| RuntimeError::ImportError {
        message: format!(
            "could not find module `{path}` (looked for `.sau` / `.saule` / `init.sau`)"
        ),
        span: span.clone(),
    })?;

    let exports = module::load_module(&abs, &loader, span.clone())?;

    match names {
        ImportNames::All => {
            for (n, v) in &exports.values {
                env.borrow_mut().define(n.clone(), v.clone());
            }
        }
        ImportNames::List(list) => {
            for (n, alias) in list {
                let v = exports.values.get(n).cloned().ok_or_else(|| {
                    RuntimeError::ImportError {
                        message: format!(
                            "`{n}` is not exported from `{}`",
                            abs.display()
                        ),
                        span: span.clone(),
                    }
                })?;
                let bind = alias.clone().unwrap_or_else(|| n.clone());
                env.borrow_mut().define(bind, v);
            }
        }
    }

    Ok(Flow::nil())
}

// ─── Assignment ──────────────────────────────────────────────────────────────

fn exec_assign(
    target: &Spanned<Expr>,
    value: &Spanned<Expr>,
    env: &Rc<RefCell<Environment>>,
) -> Result<Flow, RuntimeError> {
    let v = expr::eval(value, env)?;
    assign_target(target, v, env)
}

fn assign_target(
    target: &Spanned<Expr>,
    v: Value,
    env: &Rc<RefCell<Environment>>,
) -> Result<Flow, RuntimeError> {
    match &target.value {
        Expr::Ident(name) => {
            if env.borrow_mut().assign(name, v) {
                Ok(Flow::nil())
            } else {
                Err(RuntimeError::AssignUndeclared {
                    name: name.clone(),
                    span: target.span.clone(),
                })
            }
        }
        // `obj.field = v` / `Class.static = v`
        Expr::Member { obj, name } => {
            let receiver = expr::eval(obj, env)?;
            assign_member(&receiver, name, v, target.span.clone())
        }
        Expr::Index { obj, index } => {
            let receiver = expr::eval(obj, env)?;
            let index_value = expr::eval(index, env)?;
            assign_index(&receiver, index_value, v, target.span.clone())
        }
        _ => Err(RuntimeError::InvalidAssignTarget {
            span: target.span.clone(),
        }),
    }
}

fn assign_member(
    receiver: &Value,
    name: &str,
    value: Value,
    span: std::ops::Range<usize>,
) -> Result<Flow, RuntimeError> {
    match receiver {
        Value::Instance(inst) => {
            inst.borrow_mut().fields.insert(name.to_string(), value);
            Ok(Flow::nil())
        }
        Value::Class(class) => {
            // Walk the chain — `Child.staticField = …` should update the
            // declaring class so the change is visible to every sibling.
            if set_static_in_chain(class, name, value.clone()) {
                Ok(Flow::nil())
            } else {
                // Define a fresh static on the most-derived class.
                class
                    .static_fields
                    .borrow_mut()
                    .insert(name.to_string(), value);
                Ok(Flow::nil())
            }
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

fn set_static_in_chain(class: &Rc<crate::value::ClassObject>, name: &str, value: Value) -> bool {
    if class.static_fields.borrow().contains_key(name) {
        class
            .static_fields
            .borrow_mut()
            .insert(name.to_string(), value);
        return true;
    }
    if let Some(parent) = &class.parent {
        return set_static_in_chain(parent, name, value);
    }
    false
}

fn assign_index(
    receiver: &Value,
    index: Value,
    value: Value,
    span: std::ops::Range<usize>,
) -> Result<Flow, RuntimeError> {
    match receiver {
        Value::Table(items) => {
            let Some(slot) = expr::table_index_to_slot(&index).map_err(|message| RuntimeError::TypeError {
                message,
                span: span.clone(),
            })? else {
                return Err(RuntimeError::TypeError {
                    message: "table assignment index must be a positive integer".to_string(),
                    span,
                });
            };

            let mut items = items.borrow_mut();
            if slot >= items.len() {
                items.resize(slot + 1, Value::Nil);
            }
            items[slot] = value;
            Ok(Flow::nil())
        }
        other => Err(RuntimeError::TypeError {
            message: format!(
                "cannot assign through `[index]` on a `{}` — only tables support indexed assignment",
                other.type_name()
            ),
            span,
        }),
    }
}

// ─── Numeric for ─────────────────────────────────────────────────────────────

fn exec_for_numeric(
    var: &str,
    from: &Spanned<Expr>,
    to: &Spanned<Expr>,
    step: Option<&Spanned<Expr>>,
    body: &[Spanned<Stmt>],
    env: &Rc<RefCell<Environment>>,
    span: std::ops::Range<usize>,
) -> Result<Flow, RuntimeError> {
    let from_v = expr::eval(from, env)?;
    let to_v = expr::eval(to, env)?;
    let step_v = match step {
        Some(e) => expr::eval(e, env)?,
        // Default step matches the loop's numeric type.
        None => match &from_v {
            Value::Float(_) => Value::Float(1.0),
            _ => Value::Int(1),
        },
    };

    match (from_v, to_v, step_v) {
        (Value::Int(f), Value::Int(t), Value::Int(s)) => {
            if s == 0 {
                return Err(RuntimeError::ZeroStep { span });
            }
            run_numeric_loop_int(var, f, t, s, body, env)
        }
        (Value::Float(f), Value::Float(t), Value::Float(s)) => {
            if s == 0.0 {
                return Err(RuntimeError::ZeroStep { span });
            }
            run_numeric_loop_float(var, f, t, s, body, env)
        }
        (f, t, s) => Err(RuntimeError::TypeError {
            message: format!(
                "numeric `for` loop requires all bounds (from, to, step) to be the same numeric type — got `{}`, `{}`, `{}` (use matching integer or float bounds)",
                f.type_name(),
                t.type_name(),
                s.type_name()
            ),
            span,
        }),
    }
}

fn run_numeric_loop_int(
    var: &str,
    from: i64,
    to: i64,
    step: i64,
    body: &[Spanned<Stmt>],
    parent: &Rc<RefCell<Environment>>,
) -> Result<Flow, RuntimeError> {
    let mut i = from;
    while (step > 0 && i <= to) || (step < 0 && i >= to) {
        let scope = Environment::with_parent(parent.clone());
        scope.borrow_mut().define(var.to_string(), Value::Int(i));
        match exec_block(body, &scope)? {
            Flow::Normal(_) | Flow::Continue => {}
            Flow::Break => return Ok(Flow::nil()),
            ret @ Flow::Return(_) => return Ok(ret),
        }
        // Detect overflow so a too-large step doesn't loop forever.
        let (next, overflow) = i.overflowing_add(step);
        if overflow {
            break;
        }
        i = next;
    }
    Ok(Flow::nil())
}

fn run_numeric_loop_float(
    var: &str,
    from: f64,
    to: f64,
    step: f64,
    body: &[Spanned<Stmt>],
    parent: &Rc<RefCell<Environment>>,
) -> Result<Flow, RuntimeError> {
    let mut i = from;
    while (step > 0.0 && i <= to) || (step < 0.0 && i >= to) {
        let scope = Environment::with_parent(parent.clone());
        scope.borrow_mut().define(var.to_string(), Value::Float(i));
        match exec_block(body, &scope)? {
            Flow::Normal(_) | Flow::Continue => {}
            Flow::Break => return Ok(Flow::nil()),
            ret @ Flow::Return(_) => return Ok(ret),
        }
        i += step;
    }
    Ok(Flow::nil())
}

fn exec_for_in(
    vars: &[(String, Option<saule_ast::Type>)],
    iter: &Spanned<Expr>,
    body: &[Spanned<Stmt>],
    env: &Rc<RefCell<Environment>>,
    span: std::ops::Range<usize>,
) -> Result<Flow, RuntimeError> {
    let iter_value = expr::eval(iter, env)?;
    match iter_value {
        Value::Table(items) => {
            let snapshot = items.borrow().clone();
            for (i, value) in snapshot.into_iter().enumerate() {
                let scope = Environment::with_parent(env.clone());
                match vars {
                    [(name, _)] => {
                        scope.borrow_mut().define(name.clone(), value);
                    }
                    [(index_name, _), (value_name, _)] => {
                        scope
                            .borrow_mut()
                            .define(index_name.clone(), Value::Int((i + 1) as i64));
                        scope.borrow_mut().define(value_name.clone(), value);
                    }
                    _ => {
                        return Err(RuntimeError::TypeError {
                            message: format!(
                                "for-in loops support one value variable or an index/value pair, got {} variables",
                                vars.len()
                            ),
                            span,
                        });
                    }
                }

                match exec_block(body, &scope)? {
                    Flow::Normal(_) | Flow::Continue => {}
                    Flow::Break => return Ok(Flow::nil()),
                    ret @ Flow::Return(_) => return Ok(ret),
                }
            }
            Ok(Flow::nil())
        }
        other => Err(RuntimeError::TypeError {
            message: format!(
                "cannot iterate over a `{}` with `for ... in` — use a table value",
                other.type_name()
            ),
            span,
        }),
    }
}

