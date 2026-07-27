//! `enum` declaration execution.

use std::cell::RefCell;
use crate::fxhash::FxHashMap as HashMap;
use std::rc::Rc;

use saule_ast::{EnumVariant, Method, Spanned};

use crate::env::Environment;
use crate::error::RuntimeError;
use crate::value::{self, Value};

use super::super::{Flow, expr};
use super::make_function;

pub(super) fn exec_enum_decl(
    enum_name: &str,
    variants: &[Spanned<EnumVariant>],
    methods: &[Method],
    env: &Rc<RefCell<Environment>>,
    _span: std::ops::Range<usize>,
) -> Result<Flow, RuntimeError> {
    let mut variant_dict = HashMap::default();
    let mut tuple_variants: HashMap<String, usize> = HashMap::default();
    let mut enum_methods = HashMap::default();

    for method in methods {
        let func = Rc::new(make_function(
            Some(format!("{enum_name}.{}", method.name)),
            method.params.clone(),
            method.body.clone(),
            env,
        ));
        enum_methods.insert(method.name.clone(), func);
    }

    // Create all variants (without enum references initially).
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
            EnumVariant::Valued(name, expr_node) => {
                let val = expr::eval(expr_node, env)?;
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

    let final_enum = Rc::new(value::EnumObject {
        name: enum_name.to_string(),
        variants: variant_dict.clone(),
        tuple_variants,
        methods: enum_methods,
    });

    // Back-link each variant to its parent enum.
    for variant in variant_dict.values() {
        *variant.enum_obj.borrow_mut() = Some(final_enum.clone());
    }

    env.borrow_mut()
        .define(enum_name.to_string(), Value::Enum(final_enum));
    Ok(Flow::nil())
}
