//! `enum` declaration execution.

use crate::fxhash::FxHashMap as HashMap;
use std::cell::RefCell;
use std::rc::Rc;

use saule_ast::{EnumVariant, Method, Spanned};

use crate::env::Environment;
use crate::error::RuntimeError;
use crate::value::{self, Value};

use super::super::{Flow, expr};
use super::make_function;
use crate::value::SauleStr;

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
        enum_methods.insert(method.name.clone(), value::MethodRef::Tree(func));
    }

    // Create all variants (without enum references initially).
    //
    // Tags are the loop index, so they are dense and follow declaration
    // order — the property the bytecode compiler's jump tables rely on
    // (`VM_DESIGN.md` §9.1). Tuple variants get a tag too, even though they
    // have no singleton to hold it: `by_tag` records `None` for them and
    // `tags` still maps the name, so `Event.Click(1, 2)` can stamp the right
    // tag onto each fresh object it builds.
    let mut by_tag: Vec<Option<Rc<value::EnumVariantObject>>> = Vec::with_capacity(variants.len());
    let mut tags: HashMap<String, u32> = HashMap::default();

    for (tag, variant) in variants.iter().enumerate() {
        let tag = tag as u32;
        match &variant.value {
            EnumVariant::Bare(name) | EnumVariant::Valued(name, _) => {
                let val = match &variant.value {
                    EnumVariant::Valued(_, expr_node) => Some(expr::eval(expr_node, env)?),
                    _ => None,
                };
                let variant_obj = Rc::new(value::EnumVariantObject {
                    enum_name: SauleStr::from(enum_name),
                    variant_name: SauleStr::new(name.clone()),
                    tag,
                    value: val,
                    enum_obj: RefCell::new(None),
                });
                variant_dict.insert(name.clone(), Rc::clone(&variant_obj));
                by_tag.push(Some(variant_obj));
                tags.insert(name.clone(), tag);
            }
            EnumVariant::Tuple { name, fields } => {
                tuple_variants.insert(name.clone(), fields.len());
                by_tag.push(None);
                tags.insert(name.clone(), tag);
            }
        }
    }

    let final_enum = Rc::new(value::EnumObject {
        name: enum_name.to_string(),
        variants: variant_dict.clone(),
        by_tag,
        tags,
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
