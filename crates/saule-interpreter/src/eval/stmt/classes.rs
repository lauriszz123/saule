//! `class` and `interface` declaration execution.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use saule_ast::{ClassMember, Decl, Spanned};

use crate::env::Environment;
use crate::error::RuntimeError;
use crate::value::{ClassObject, FieldDef, FunctionObject, InterfaceObject, Value};

use super::super::{Flow, expr};
use super::make_function;

/// Materialize a `Decl::Class` into a [`ClassObject`] and install it under
/// the class's name in `env`. Method closures all capture `env` so they can
/// see the class itself (used by static calls) and other top-level names.
pub(super) fn exec_class_decl(
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
                        pname,
                        other.type_name()
                    ),
                    span,
                });
            }
            None => {
                return Err(RuntimeError::TypeError {
                    message: format!(
                        "cannot extend `{}`: no class with that name is in scope at runtime \
                         (was it imported and visible at this point?)",
                        pname
                    ),
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
    // statics so callers can read them via `ClassName.field`.
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
                let treat_as_static = *is_static || (!has_init_method && default.is_some());
                if treat_as_static {
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
                    constructor = Some(func);
                } else {
                    methods.insert(m.name.clone(), func);
                }
            }
        }
    }

    // Validate that all implemented interfaces' required methods are present.
    let mut missing_methods: Vec<(String, String)> = Vec::new();
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
                return Err(RuntimeError::TypeError {
                    message: format!(
                        "cannot implement `{}`: no interface with that name is in scope at \
                         runtime (was it imported and visible at this point?)",
                        interface_name
                    ),
                    span,
                });
            }
        }
    }

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
    // bare `Value::Function` still see the class's statics.
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
pub(super) fn exec_interface_decl(
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
