//! Statement execution with control-flow propagation.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use saule_ast::{ClassMember, Decl, EnumVariant, Expr, ImportNames, Method, Param, Spanned, Stmt, Type};

use crate::env::Environment;
use crate::error::RuntimeError;
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

        Stmt::Throw(e) => {
            let v = expr::eval(e, env)?;
            let display = v.to_display_string();
            thrown_slot::set(v);
            Err(RuntimeError::Thrown { value: display, span })
        }
        Stmt::Try {
            body,
            catch_var,
            catch_ty,
            catch_body,
        } => exec_try(body, catch_var, catch_ty, catch_body, env),
        Stmt::Decl(decl) => exec_decl(decl, env),
    }
}

/// Park the in-flight thrown `Value` so `RuntimeError::Thrown` can stay
/// `Send + Sync` (miette's requirement) while the actual value — which
/// contains non-`Send` `Rc`s — rides alongside in a thread-local slot.
mod thrown_slot {
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

/// Run a `try ... catch e: T ... end` block. The catch arm fires only when:
///   1. the body errored with a `RuntimeError::Thrown`, **and**
///   2. the thrown value's runtime type matches `catch_ty`.
///
/// Any other error — or a thrown value whose type doesn't match — is
/// re-propagated so an outer `try` (or the top-level driver) can see it.
fn exec_try(
    body: &[Spanned<Stmt>],
    catch_var: &str,
    catch_ty: &Type,
    catch_body: &[Spanned<Stmt>],
    env: &Rc<RefCell<Environment>>,
) -> Result<Flow, RuntimeError> {
    let body_scope = Environment::with_parent(env.clone());
    match exec_block(body, &body_scope) {
        Ok(flow) => Ok(flow),
        Err(RuntimeError::Thrown { value, span }) => {
            let thrown = thrown_slot::take().unwrap_or(Value::Nil);
            if runtime_matches_type(&thrown, catch_ty) {
                let catch_scope = Environment::with_parent(env.clone());
                catch_scope.borrow_mut().define(catch_var.to_string(), thrown);
                exec_block(catch_body, &catch_scope)
            } else {
                // Re-park and re-throw for an outer handler.
                thrown_slot::set(thrown);
                Err(RuntimeError::Thrown { value, span })
            }
        }
        Err(other) => Err(other),
    }
}

/// Best-effort runtime check that `value` satisfies the declared `catch_ty`.
/// Nullable, table-of, and function types are accepted structurally; classes
/// match by walking the parent chain; interfaces match by name lookup.
fn runtime_matches_type(value: &Value, ty: &Type) -> bool {
    match ty {
        Type::Nullable(inner) => {
            matches!(value, Value::Nil) || runtime_matches_type(value, inner)
        }
        Type::Tuple(_) => true, // multi-return shapes aren't introspectable here
        Type::Function { .. } => matches!(
            value,
            Value::Function(_) | Value::Native(_) | Value::NativeClosure(_)
        ),
        Type::Table { .. } => matches!(value, Value::Table(_)),
        Type::Named(name) => match name.as_str() {
            "any" => true,
            "nil" => matches!(value, Value::Nil),
            "boolean" => matches!(value, Value::Bool(_)),
            "integer" => matches!(value, Value::Int(_)),
            "float" => matches!(value, Value::Float(_)),
            "number" => matches!(value, Value::Int(_) | Value::Float(_)),
            "string" => matches!(value, Value::Str(_)),
            "table" => matches!(value, Value::Table(_)),
            "function" => matches!(
                value,
                Value::Function(_) | Value::Native(_) | Value::NativeClosure(_)
            ),
            other => match value {
                Value::Instance(inst) => {
                    let inst_ref = inst.borrow();
                    let mut cur = Some(inst_ref.class.clone());
                    while let Some(c) = cur {
                        if c.name == other {
                            return true;
                        }
                        cur = c.parent.clone();
                    }
                    false
                }
                Value::Class(c) => c.name == other,
                Value::EnumVariant(v) => v.enum_name == other,
                Value::Enum(e) => e.name == other,
                _ => false,
            },
        },
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
                owner_class: std::cell::RefCell::new(None),
                source: crate::module::active_module_source(),
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

    // Back-link every method to its owning class so calls to a method via a
    // bare `Value::Function` (e.g. inside another static method that resolved
    // a sibling via `inject_class_statics`) still see the class's statics.
    for f in class.methods.values() {
        f.set_owner_class(&class);
    }
    for f in class.static_methods.values() {
        f.set_owner_class(&class);
    }
    if let Some(c) = class.constructor.as_ref() {
        c.set_owner_class(&class);
    }

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
        owner_class: std::cell::RefCell::new(None),
        source: crate::module::active_module_source(),
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
    let mut tuple_variants: HashMap<String, usize> = HashMap::new();
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
            EnumVariant::Tuple { name, fields } => {
                tuple_variants.insert(name.clone(), fields.len());
            }
        }
    }

    // Create the final enum object with all variants
    let final_enum = Rc::new(value::EnumObject {
        name: enum_name.to_string(),
        variants: variant_dict.clone(),
        tuple_variants,
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
            items
                .borrow_mut()
                .set(&index, value)
                .map_err(|message| RuntimeError::TypeError {
                    message,
                    span: span.clone(),
                })?;
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
            // Snapshot to allow the table to mutate during iteration without
            // breaking the loop. Yield array entries first, then map entries.
            let (array, map_entries) = {
                let t = items.borrow();
                let array = t.array.clone();
                let mut map_entries: Vec<(crate::value::TableKey, Value)> =
                    t.map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                // Deterministic order: int keys ascending, then strings, then bools.
                map_entries.sort_by(|a, b| match (&a.0, &b.0) {
                    (crate::value::TableKey::Int(x), crate::value::TableKey::Int(y)) => x.cmp(y),
                    (crate::value::TableKey::Int(_), _) => std::cmp::Ordering::Less,
                    (_, crate::value::TableKey::Int(_)) => std::cmp::Ordering::Greater,
                    (crate::value::TableKey::Str(x), crate::value::TableKey::Str(y)) => x.cmp(y),
                    (crate::value::TableKey::Str(_), _) => std::cmp::Ordering::Less,
                    (_, crate::value::TableKey::Str(_)) => std::cmp::Ordering::Greater,
                    (crate::value::TableKey::Bool(x), crate::value::TableKey::Bool(y)) => x.cmp(y),
                });
                (array, map_entries)
            };

            // Helper to bind one (key, value) pair and run the body.
            let run_iter = |key: Value, value: Value| -> Result<Flow, RuntimeError> {
                let scope = Environment::with_parent(env.clone());
                match vars {
                    [(name, _)] => {
                        scope.borrow_mut().define(name.clone(), value);
                    }
                    [(key_name, _), (value_name, _)] => {
                        scope.borrow_mut().define(key_name.clone(), key);
                        scope.borrow_mut().define(value_name.clone(), value);
                    }
                    _ => {
                        return Err(RuntimeError::TypeError {
                            message: format!(
                                "for-in loops support one value variable or a key/value pair, got {} variables",
                                vars.len()
                            ),
                            span: span.clone(),
                        });
                    }
                }
                exec_block(body, &scope)
            };

            for (i, value) in array.into_iter().enumerate() {
                match run_iter(Value::Int((i + 1) as i64), value)? {
                    Flow::Normal(_) | Flow::Continue => {}
                    Flow::Break => return Ok(Flow::nil()),
                    ret @ Flow::Return(_) => return Ok(ret),
                }
            }
            for (k, v) in map_entries {
                match run_iter(k.to_value(), v)? {
                    Flow::Normal(_) | Flow::Continue => {}
                    Flow::Break => return Ok(Flow::nil()),
                    ret @ Flow::Return(_) => return Ok(ret),
                }
            }
            Ok(Flow::nil())
        }
        other => {
            // For functions and instances we drive a closure-based iterator.
            // Instances must expose an `iter()` method that returns the closure.
            let driver: Value = match &other {
                Value::Function(_) | Value::Native(_) | Value::NativeClosure(_) => other.clone(),
                Value::Instance(_) => {
                    let result = expr::invoke_method_multi(
                        &other,
                        "iter",
                        Vec::new(),
                        span.clone(),
                    )?;
                    let Some(driver) = result.into_iter().next() else {
                        return Err(RuntimeError::TypeError {
                            message: format!(
                                "`{}.iter()` returned no value — it must return a function",
                                other.type_name()
                            ),
                            span,
                        });
                    };
                    if !matches!(driver, Value::Function(_) | Value::Native(_) | Value::NativeClosure(_)) {
                        return Err(RuntimeError::TypeError {
                            message: format!(
                                "`iter()` must return a function, got `{}`",
                                driver.type_name()
                            ),
                            span,
                        });
                    }
                    driver
                }
                _ => {
                    return Err(RuntimeError::TypeError {
                        message: format!(
                            "cannot iterate over a `{}` with `for ... in` — use a table, a function, or a class that implements `Iterable`",
                            other.type_name()
                        ),
                        span,
                    });
                }
            };

            // Drive the closure: call repeatedly with no arguments. Stop when
            // the first returned value is `nil` (Lua's nil-terminator). Each
            // step's returns are bound positionally across the loop variables
            // (extras → nil, surplus values dropped).
            loop {
                let values = expr::call_value_multi(driver.clone(), &[], span.clone())?;
                if values.first().is_none_or(|v| matches!(v, Value::Nil)) {
                    break;
                }
                let scope = Environment::with_parent(env.clone());
                {
                    let mut scope_mut = scope.borrow_mut();
                    for (i, (name, _)) in vars.iter().enumerate() {
                        let v = values.get(i).cloned().unwrap_or(Value::Nil);
                        scope_mut.define(name.clone(), v);
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
    }
}

