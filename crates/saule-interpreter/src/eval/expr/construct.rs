//! Instance construction (`Class(args)`) and enum tuple-variant
//! constructors.

use std::cell::RefCell;
use std::rc::Rc;

use crate::env::Environment;
use crate::error::RuntimeError;
use crate::value::{
    ClassObject, EnumObject, EnumVariantObject, FunctionObject, InstanceObject, NativeClosure,
    TableObject, Value,
};

use super::calls::{bind_params, inject_class_statics, run_function_body, user_params};
use super::{EvaluatedArg, SUPER_OWNER_BINDING, eval};

/// Build a callable that constructs a fresh `EnumVariant` carrying its
/// arguments as an array-style table payload. The arity is checked at call
/// time; pattern matching on `Enum.Variant(p1, p2, ...)` destructures the
/// payload positionally.
pub(super) fn make_tuple_variant_ctor(
    enum_obj: Rc<EnumObject>,
    variant_name: String,
    arity: usize,
) -> Value {
    let label = format!("{}.{} (variant ctor)", enum_obj.name, variant_name);
    // Leak the descriptive name into a `&'static str` because `NativeClosure`
    // wants `&'static str` for its `name`. One leak per declared tuple
    // variant is fine — declarations happen once at startup.
    let static_name: &'static str = Box::leak(label.into_boxed_str());
    let enum_name = enum_obj.name.clone();
    // Resolved once, when the constructor is built rather than on every
    // call, so each `Event.Click(x, y)` carries its declaration's tag.
    let tag = enum_obj.tag_of(&variant_name).unwrap_or(u32::MAX);
    Value::NativeClosure(Rc::new(NativeClosure {
        name: static_name,
        func: Box::new(move |args: &[Value]| -> Result<Vec<Value>, String> {
            if args.len() != arity {
                return Err(format!(
                    "{}.{} expects {arity} argument(s), got {}",
                    enum_name,
                    variant_name,
                    args.len()
                ));
            }
            let payload = Value::Table(Rc::new(RefCell::new(TableObject::from_array(
                args.to_vec(),
            ))));
            let variant = Rc::new(EnumVariantObject {
                enum_name: enum_name.clone(),
                variant_name: variant_name.clone(),
                tag,
                value: Some(payload),
                enum_obj: RefCell::new(Some(enum_obj.clone())),
            });
            Ok(vec![Value::EnumVariant(variant)])
        }),
        param_names: Vec::new(),
    }))
}

/// `Class(args)` — create an instance, populate field defaults, then
/// run the constructor (if any) with `self` bound to the new object.
pub(crate) fn construct(
    class: Rc<ClassObject>,
    args: &[EvaluatedArg],
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    let inst = Rc::new(RefCell::new(InstanceObject::new(class.clone())));

    init_fields(&class, &inst)?;

    if let Some(ctor) = constructor_chain(&class) {
        let scope = Environment::with_parent(ctor.closure.clone());
        inject_class_statics(&scope, &class);
        scope
            .borrow_mut()
            .define("self".to_string(), Value::Instance(inst.clone()));
        scope
            .borrow_mut()
            .define(SUPER_OWNER_BINDING.to_string(), Value::Class(class.clone()));
        bind_params(
            &scope,
            user_params(&ctor),
            ctor.user_param_keys(),
            args,
            &span,
        )?;
        let result = run_function_body(&ctor, &scope, span);
        Environment::release(scope);
        result?;
    }

    Ok(Value::Instance(inst))
}

fn init_fields(
    class: &Rc<ClassObject>,
    inst: &Rc<RefCell<InstanceObject>>,
) -> Result<(), RuntimeError> {
    if let Some(parent) = &class.parent {
        init_fields(parent, inst)?;
    }
    // Field defaults are evaluated against *this* class's defining
    // environment, so the closure we borrow has to come from a method this
    // class declared itself. Since `methods` is flattened, it also holds the
    // parent's methods — whose closures capture the parent's module scope,
    // which for an imported parent is a different file entirely. Filtering
    // on the owner class is what keeps that from leaking in.
    let owned = |m: &Rc<FunctionObject>| {
        m.resolved_owner()
            .is_some_and(|owner| Rc::ptr_eq(&owner, class))
    };
    let scope = if let Some(ctor) = &class.constructor {
        Environment::with_parent(ctor.closure.clone())
    } else if let Some(m) = class.methods.values().find(|m| owned(m)) {
        Environment::with_parent(m.closure.clone())
    } else if let Some(m) = class.static_methods.values().find(|m| owned(m)) {
        Environment::with_parent(m.closure.clone())
    } else {
        Environment::new()
    };
    scope
        .borrow_mut()
        .define("self".to_string(), Value::Instance(inst.clone()));

    for field in &class.field_defs {
        let value = match &field.default {
            Some(e) => eval(e, &scope)?,
            None => Value::Nil,
        };
        // The slot always exists: `class.layout` was built from these very
        // `field_defs`, and `init_fields` recurses parent-first so the
        // parent's slots are filled before the child's.
        inst.borrow_mut().set_field(&field.name, value);
    }
    Ok(())
}

pub(super) fn constructor_chain(class: &Rc<ClassObject>) -> Option<Rc<FunctionObject>> {
    if let Some(c) = &class.constructor {
        return Some(c.clone());
    }
    class.parent.as_ref().and_then(constructor_chain)
}
