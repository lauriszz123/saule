//! The native signature table, plus the one lookup that needs a
//! `saule_semantic::MethodSig`.
//!
//! The table itself lives in [`saule_sigs`], upstream of `saule-semantic`
//! so that crate's return-type inference can consult it. This module
//! re-exports it wholesale and adds back the single entry point that takes
//! a *semantic* method signature — which cannot live upstream, because
//! `saule-semantic` is downstream of the table.
//!
//! Every existing `saule_typeck::sigs::…` path therefore still resolves,
//! whichever side of the split the item ended up on.

pub use saule_sigs::*;

use saule_ast::Type;
use std::collections::HashMap;

/// Same as [`instantiate_returns`] but for a *semantic* method signature
/// (e.g. a dynamic native package's class method seeded into
/// `saule_semantic`). Returns the substituted return type, or `None` when
/// the method has no declared return.
pub fn instantiate_method_return(
    sig: &saule_semantic::MethodSig,
    arg_types: &[Option<Type>],
) -> Option<Type> {
    let ret = sig.return_ty.clone()?;
    if sig.type_params.is_empty() {
        return Some(ret);
    }
    let mut subst = HashMap::new();
    for (p, found) in sig.params.iter().zip(arg_types.iter()) {
        if let Some(found_ty) = found {
            saule_sigs::generics::unify(&p.ty, found_ty, &sig.type_params, &mut subst);
        }
    }
    Some(saule_sigs::generics::substitute(
        &ret,
        &subst,
        &sig.type_params,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use saule_ast::Param;

    fn param(name: &str, ty: Type) -> Param {
        Param {
            name: name.into(),
            ty,
            default: None,
            variadic: false,
            span: 0..0,
        }
    }


    #[test]
    fn instantiate_method_return_substitutes_nullable() {
        // find<T>(t: table<T>, f: fn(T) -> boolean) -> T?
        let sig = saule_semantic::MethodSig {
            is_static: true,
            is_private: false,
            type_params: vec!["T".into()],
            params: vec![
                param("t", t_table(t_named("T"))),
                param("f", t_function(vec![t_named("T")], t_named("boolean"))),
            ],
            return_ty: Some(t_nullable(t_named("T"))),
        };
        let args = [
            Some(t_table(t_named("integer"))),
            Some(t_function(vec![t_named("integer")], t_named("boolean"))),
        ];
        assert_eq!(
            instantiate_method_return(&sig, &args),
            Some(t_nullable(t_named("integer")))
        );
    }
}
