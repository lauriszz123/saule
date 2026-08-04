//! The walker's own type inference, plus the `TypeSource` impl that
//! hands it to the shared rules in [`crate::exprty`].

use crate::hover::util::strip_nullable_type;
use saule_ast::{CallArg, Expr, Param, Spanned, Stmt, Type};
use saule_semantic::{lookup_field_type, lookup_method, with_classes};

use super::*;

impl<'a> Cx<'a> {
    /// Infer the types of a call's positional arguments (in order;
    /// `None` where inference can't produce a type). Named arguments are
    /// skipped — mirrors how the typechecker binds generics from
    /// positional args only.
    pub(crate) fn positional_arg_types(&self, args: &[CallArg]) -> Vec<Option<Type>> {
        args.iter()
            .filter_map(|a| match a {
                CallArg::Positional(e) => Some(self.infer_init_type(&e.value)),
                CallArg::Named { .. } => None,
            })
            .collect()
    }

    /// Expand a value-expression list into the flat list of value types it
    /// produces, using Lua-style multi-assign semantics (mirrors the
    /// interpreter's `eval_expr_list`): every expression contributes exactly
    /// one value *except the last*, whose tuple components (a multi-return)
    /// spread into several. Non-final expressions are in single-value context,
    /// so a tuple there collapses to its first component.
    pub(crate) fn spread_value_types(&self, values: &[Spanned<Expr>]) -> Vec<Type> {
        let mut out = Vec::new();
        let n = values.len();
        for (i, v) in values.iter().enumerate() {
            let ty = self.infer_init_type(&v.value);
            if i + 1 == n {
                match ty {
                    Some(Type::Tuple(parts)) => out.extend(parts),
                    other => out.push(first_value_type(other)),
                }
            } else {
                out.push(first_value_type(ty));
            }
        }
        out
    }

    /// Best-effort type inference for a `local x = <init>` site when
    /// the user didn't write an annotation. Handles the cases that
    /// account for the bulk of real-world `local`s in Saule code:
    ///
    /// * `Class(args)` — constructor call returns `Class`.
    /// * `obj.method(args)` — uses the registered method's return type.
    /// * `obj.field` — uses the field's declared type.
    /// * Existing local — propagates its known type.
    /// * `self` inside a method — the enclosing class.
    /// * Literal expressions — their primitive type.
    ///
    /// Anything else returns `None`; the caller falls back to `any`.
    pub(crate) fn infer_init_type(&self, init: &Expr) -> Option<Type> {
        match init {
            // `x as T` is always `T?` — the cast can fail, and the
            // nullable result is what forces the caller to handle it.
            Expr::Cast { ty, .. } => Some(Type::Nullable(Box::new(ty.clone()))),
            Expr::Call { callee, args } => {
                if let Expr::Ident(name) = &callee.value {
                    if with_classes(|r| r.contains_key(name)) {
                        return Some(Type::Named(name.clone()));
                    }
                    // Call to a function declared at top level in this
                    // same file. Its type parameters are bound from the
                    // actual argument types, so `map(table<string>, …)`
                    // infers `table<U>`'s `U` rather than leaving the
                    // local at bare `any`.
                    if let Some(f) = self.module_fns.get(name) {
                        let arg_types = self.positional_arg_types(args);
                        return saule_typeck::sigs::instantiate_returns(&f.sig, &arg_types)
                            .into_iter()
                            .next()
                            // A type parameter the arguments didn't pin
                            // down would come back as its own bare name
                            // (`table<U>`). That's "unknown", not a
                            // type — fall through to `any`.
                            .filter(|t| {
                                !saule_typeck::sigs::mentions_unbound_param(t, &f.sig.type_params)
                            });
                    }
                    // Non-constructor free call: consult native-sig
                    // returns or imported function signatures. We
                    // don't have ASTs for those, so return None and
                    // accept `any`.
                    if let Some(sig) = saule_typeck::sigs::lookup(name) {
                        let arg_types = self.positional_arg_types(args);
                        return saule_typeck::sigs::instantiate_returns(&sig, &arg_types)
                            .into_iter()
                            .next();
                    }
                    // Sibling free function inside a class body —
                    // reach through the enclosing-class registry the
                    // same way `resolve_ident` does for hover.
                    if let Some(class) = &self.enclosing_class
                        && let Some(sig) = lookup_method(class, name)
                    {
                        let arg_types = self.positional_arg_types(args);
                        return saule_typeck::sigs::instantiate_method_return(&sig, &arg_types);
                    }
                }
                // `recv.method(args)` — dot-call on an instance or
                // module. Resolve the receiver's class, then chase the
                // method's registered return type.
                if let Expr::Member { obj, name } = &callee.value {
                    let class = self.receiver_class(&obj.value)?;
                    if let Some(sig) = lookup_method(&class, name) {
                        let arg_types = self.positional_arg_types(args);
                        return saule_typeck::sigs::instantiate_method_return(&sig, &arg_types);
                    }
                    let qname = format!("{class}.{name}");
                    if let Some(sig) = saule_typeck::sigs::lookup(&qname) {
                        let arg_types = self.positional_arg_types(args);
                        return saule_typeck::sigs::instantiate_returns(&sig, &arg_types)
                            .into_iter()
                            .next();
                    }
                }
                None
            }
            Expr::Member { obj, name } | Expr::SafeMember { obj, name } => {
                let class = self.receiver_class(&obj.value)?;
                lookup_field_type(&class, name)
            }
            Expr::Index { obj, .. } => {
                // Element type of `table<V>` / `table<K, V>`. Anything
                // else (e.g. indexing a class instance) we don't try
                // to resolve here — typeck handles that statically.
                let obj_ty = self.infer_init_type(&obj.value)?;
                match obj_ty {
                    Type::Table { value, .. } => Some(*value),
                    Type::Nullable(inner) => match *inner {
                        Type::Table { value, .. } => Some(*value),
                        _ => None,
                    },
                    _ => None,
                }
            }
            Expr::ForceUnwrap(inner) => {
                let ty = self.infer_init_type(&inner.value)?;
                Some(strip_nullable_type(ty))
            }
            // Operators are typed by the rules in [`crate::exprty`], shared
            // with the inlay walker and mirrored from the checker.
            Expr::Unary { op, rhs } => crate::exprty::unary_type(self, *op, &rhs.value),
            Expr::Binary { op, lhs, rhs } => {
                crate::exprty::binary_type(self, *op, &lhs.value, &rhs.value)
            }
            Expr::Lambda {
                params,
                return_ty,
                body,
            } => Some(Type::Function {
                params: params.iter().map(|p| p.ty.clone()).collect(),
                ret: Box::new(
                    return_ty
                        .clone()
                        .or_else(|| self.infer_lambda_return(params, body))
                        .unwrap_or_else(|| Type::Named("any".into())),
                ),
            }),
            Expr::Table(entries) => Some(self.infer_table_literal(entries)),
            Expr::Match { arms, .. } => {
                // First arm's body type is a decent approximation;
                // typeck enforces that all arms agree.
                arms.first().and_then(|arm| match &arm.body {
                    saule_ast::MatchBody::Expr(e) => self.infer_init_type(&e.value),
                    saule_ast::MatchBody::Block(b) => match b.last().map(|s| &s.value) {
                        Some(Stmt::Expr(e)) => self.infer_init_type(&e.value),
                        Some(Stmt::Return(rs)) => {
                            rs.first().and_then(|e| self.infer_init_type(&e.value))
                        }
                        _ => None,
                    },
                })
            }
            Expr::Pipe { source, stages } => crate::exprty::pipe_type(self, &source.value, stages),
            Expr::Ident(name) => {
                if let Some(local) = self.lookup_local(name) {
                    return Some(local.ty.clone());
                }
                // Bare class / module / value-type name — its "value"
                // type is the named class itself. Useful for `let s =
                // Storage` (no call) and indirectly for method-chain
                // inference upstream.
                if with_classes(|r| r.contains_key(name))
                    || ((saule_typeck::sigs::is_module(name)
                        || saule_typeck::sigs::is_value_type(name))
                        && saule_semantic::prelude::contains(name))
                {
                    return Some(Type::Named(name.clone()));
                }
                None
            }
            Expr::Self_ => self
                .enclosing_class
                .as_ref()
                .map(|c| Type::Named(c.clone())),
            Expr::Str(_) => Some(Type::Named("string".into())),
            Expr::Int(_) => Some(Type::Named("integer".into())),
            Expr::Float(_) => Some(Type::Named("float".into())),
            Expr::Bool(_) => Some(Type::Named("boolean".into())),
            Expr::Nil => Some(Type::Nullable(Box::new(Type::Named("any".into())))),
        }
    }

    /// Best-effort return type of an unannotated expression-bodied
    /// lambda (`s => #s` is `fn(any) -> integer`). This is what lets a
    /// generic call like `map(items, s => #s)` bind the callback's
    /// result type instead of settling for `table<any>`.
    ///
    /// Two deliberate limits. Block-bodied lambdas are skipped: their
    /// `return` statements would need the full statement walk, and a
    /// wrong answer here is worse than `any`. And a lambda whose
    /// parameters shadow an in-scope binding is skipped too — the body
    /// is inferred against the *enclosing* scope (the parameters aren't
    /// pushed, since inference runs behind `&self`), so a shadowed name
    /// would silently resolve to the outer variable's type.
    pub(crate) fn infer_lambda_return(
        &self,
        params: &[Param],
        body: &saule_ast::LambdaBody,
    ) -> Option<Type> {
        let saule_ast::LambdaBody::Expr(e) = body else {
            return None;
        };
        if params.iter().any(|p| self.lookup_local(&p.name).is_some()) {
            return None;
        }
        Some(first_value_type(self.infer_init_type(&e.value)))
    }

    /// Infer a `table<V>` (array literal) or `table<K, V>` (map literal)
    /// from a table-constructor's entries, so generic natives like
    /// `Util.map(table<T>, …)` can bind their element type. Falls back to a
    /// bare `table` when the entries are empty or their types disagree.
    pub(crate) fn infer_table_literal(&self, entries: &[saule_ast::TableEntry]) -> Type {
        use saule_ast::TableEntry;
        let mut value_ty: Option<Type> = None;
        let mut key_ty: Option<Type> = None;
        let mut has_field = false;
        let mut consistent = true;
        for entry in entries {
            let (k, v) = match entry {
                TableEntry::Positional(v) => (None, self.infer_init_type(&v.value)),
                TableEntry::Field { key, value } => {
                    has_field = true;
                    (
                        self.infer_init_type(&key.value),
                        self.infer_init_type(&value.value),
                    )
                }
            };
            if let Some(vt) = v {
                match &value_ty {
                    Some(existing) if existing != &vt => consistent = false,
                    _ => value_ty = Some(vt),
                }
            }
            if let Some(kt) = k {
                match &key_ty {
                    Some(existing) if existing != &kt => consistent = false,
                    _ => key_ty = Some(kt),
                }
            }
        }
        match value_ty {
            Some(v) if consistent => Type::Table {
                key: if has_field {
                    key_ty.map(Box::new)
                } else {
                    None
                },
                value: Box::new(v),
            },
            _ => Type::Named("table".into()),
        }
    }

    /// Refine a bare structural annotation (`table` / `function`) against the
    /// initializer's inferred shape: `local nums: table = {1, 2}` becomes
    /// `table<integer>`. A `nil`-side or mismatched-kind value leaves the
    /// declared type untouched. Mirrors typeck's `refine_bare_binding`.
    pub(crate) fn refine_bare_annotation(&self, decl: Type, value: Option<Type>) -> Type {
        let Type::Named(name) = &decl else {
            return decl;
        };
        let Some(value_ty) = value else {
            return decl;
        };
        let inner = match &value_ty {
            Type::Nullable(i) => (**i).clone(),
            other => other.clone(),
        };
        let matches_kind = matches!(
            (name.as_str(), &inner),
            ("table", Type::Table { .. }) | ("function", Type::Function { .. })
        );
        if matches_kind { inner } else { decl }
    }

    /// Best-effort: resolve `callee` to the name and parameter list
    /// named-argument hover needs. Handles the common shapes —
    /// constructor calls, sibling free / static / method calls, and
    /// functions imported from another file — without trying to be
    /// exhaustive.
    pub(crate) fn callee_sig(&self, callee: &Expr) -> Option<CalleeSig> {
        let sig = |display: String, params: Vec<Param>, type_params: Vec<String>, doc_key: &str| {
            CalleeSig {
                params,
                type_params,
                doc: self.imports.docs.get(doc_key).cloned(),
                display,
            }
        };
        match callee {
            Expr::Ident(name) => {
                // Constructor: `init` method, falling through to a
                // bare `Class()` call (which uses no init params).
                if with_classes(|r| r.contains_key(name)) {
                    let init = lookup_method(name, "init")?;
                    // Constructor prose is conventionally written on the
                    // class, so try that before `Class.init`.
                    let key = if self.imports.docs.get(name).is_some() {
                        name.clone()
                    } else {
                        format!("{name}.init")
                    };
                    return Some(sig(name.clone(), init.params, init.type_params, &key));
                }
                if let Some(class) = &self.enclosing_class
                    && let Some(m) = lookup_method(class, name)
                {
                    return Some(sig(
                        format!("{class}.{name}"),
                        m.params,
                        m.type_params,
                        &format!("{class}.{name}"),
                    ));
                }
                // Sibling top-level `fn` — the one free-call shape where
                // we do have the declared parameter *names*.
                if let Some(f) = self.module_fns.get(name) {
                    return Some(sig(
                        name.clone(),
                        f.params.clone(),
                        f.sig.type_params.clone(),
                        name,
                    ));
                }
                // A free function imported from another `.sau` file,
                // including through a re-export barrel — the
                // `showDialog(builder: …)` helpers a UI file is full of.
                // The seed registers these by local alias, so no import
                // walk is needed here.
                if let Some(f) = saule_semantic::lookup_function(name) {
                    return Some(sig(name.clone(), f.params, f.type_params, name));
                }
                // Native sigs (`Math.sqrt`, `print`) know positional
                // types but no parameter names, so they can't drive
                // named-arg hover. Treat them as unresolved.
                None
            }
            Expr::Member { obj, name } => {
                if let Some((owner, m)) = self.super_target(name, &obj.value) {
                    let key = format!("{owner}.init");
                    return Some(sig(format!("{owner}.init"), m.params, m.type_params, &key));
                }
                let class = self.receiver_class(&obj.value)?;
                let m = lookup_method(&class, name)?;
                let key = format!("{class}.{name}");
                Some(sig(key.clone(), m.params, m.type_params, &key))
            }
            _ => None,
        }
    }
}
