//! The walker's own type inference, and the callee resolution the
//! parameter-name hints need.

use saule_ast::{CallArg, Expr, LambdaBody, MatchBody, Param, Spanned, Stmt, TableEntry, Type};
use saule_semantic::{lookup_field_type, lookup_method, with_classes};

use super::*;

/// A semantic method signature in the shape the generic-instantiation
/// helpers take.
fn method_sig_as_native(sig: saule_semantic::MethodSig) -> saule_typeck::sigs::NativeSig {
    saule_typeck::sigs::NativeSig {
        type_params: sig.type_params,
        bounds: Vec::new(),
        params: sig.params.iter().map(|p| p.ty.clone()).collect(),
        variadic: None,
        returns: sig.return_ty.into_iter().collect(),
    }
}

impl<'a> Cx<'a> {
    /// Expand a value-expression list into the flat list of value types it
    /// yields, using Lua-style multi-assign semantics (mirrors the
    /// interpreter): every expression contributes one value except the last,
    /// whose tuple components (a multi-return) spread into several.
    /// Non-final expressions sit in single-value context, so a tuple there
    /// collapses to its first component.
    pub(crate) fn spread_value_types(&self, values: &[Spanned<Expr>]) -> Vec<Type> {
        let mut out = Vec::new();
        let n = values.len();
        for (i, v) in values.iter().enumerate() {
            let ty = self.infer_type(&v.value);
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

    pub(crate) fn infer_type(&self, e: &Expr) -> Option<Type> {
        match e {
            // A recovery hole gives no type, so no hint is rendered.
            Expr::Error => None,
            Expr::Int(_) => Some(Type::Named("integer".into())),
            Expr::Float(_) => Some(Type::Named("float".into())),
            Expr::Bool(_) => Some(Type::Named("boolean".into())),
            Expr::Str(_) => Some(Type::Named("string".into())),
            Expr::Nil => None,
            Expr::Ident(name) => self
                .locals
                .iter()
                .rev()
                .find(|l| l.name == *name)
                .map(|l| l.ty.clone()),
            Expr::Call { callee, args, .. } => {
                if let Expr::Ident(name) = &callee.value {
                    if with_classes(|r| r.contains_key(name)) {
                        return Some(Type::Named(name.clone()));
                    }
                    // Sibling top-level `fn` — bind its type parameters
                    // from the actual arguments, exactly as for a generic
                    // native.
                    if let Some(f) = self.user_fns.get(name) {
                        let arg_types = self.positional_arg_types(args);
                        return saule_typeck::sigs::instantiate_returns(&f.sig, &arg_types)
                            .into_iter()
                            .next()
                            // A type parameter the arguments didn't pin
                            // down comes back as its own bare name — no
                            // hint beats a hint reading `: table<U>`.
                            .filter(|t| {
                                !saule_typeck::sigs::mentions_unbound_param(t, &f.sig.type_params)
                            });
                    }
                    if let Some(sig) = saule_typeck::sigs::lookup(name) {
                        let arg_types = self.positional_arg_types(args);
                        return saule_typeck::sigs::instantiate_returns(&sig, &arg_types)
                            .into_iter()
                            .next();
                    }
                }
                // `recv.method(args)` — dot-call on a class instance or on
                // a stdlib module. The receiver may not resolve to a class
                // at all (`Os.fsInfo(p)`), so a failed `receiver_class`
                // falls through to the module-qualified native lookup
                // rather than giving up — mirrors `callee_params`.
                if let Expr::Member { obj, name } = &callee.value {
                    if let Some(class) = self.receiver_class(&obj.value) {
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
                    // Stdlib module call: `Os.fsInfo(path)` — receiver is a
                    // bare identifier registered as a module, not a class.
                    if let Expr::Ident(recv) = &obj.value
                        && let Some(sig) = saule_typeck::sigs::lookup(&format!("{recv}.{name}"))
                    {
                        let arg_types = self.positional_arg_types(args);
                        return saule_typeck::sigs::instantiate_returns(&sig, &arg_types)
                            .into_iter()
                            .next();
                    }
                }
                None
            }
            Expr::Self_ => self.enclosing_class.clone().map(Type::Named),
            Expr::Table(entries) => Some(self.infer_table_literal(entries)),
            // Operators are typed by the rules in [`crate::exprty`], shared
            // with the hover walker and mirrored from the checker. Typing
            // them at all is what lets a callback body like `x => x * 2`
            // pin down a generic result.
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
            Expr::Pipe { source, stages } => crate::exprty::pipe_type(self, &source.value, stages),
            // `x as T` — `T?` for the readings that can fail (a type test,
            // a parse), `T` for a conversion that cannot. Worth a hint
            // either way, and especially when the `?` is there: it is the
            // reason the result still has to be unwrapped.
            Expr::Cast { value, ty, .. } => {
                let source = self.infer_type(&value.value);
                Some(saule_typeck::casts::resolve(source.as_ref(), ty).result(ty))
            }
            // `x!` drops the nullability the unwrap just asserted away.
            Expr::ForceUnwrap(inner) => Some(strip_nullable(self.infer_type(&inner.value)?)),
            // `obj.field` — the field's declared type. A bare method
            // reference has no useful label (function types don't
            // render), so only fields answer here.
            Expr::Member { obj, name } => {
                if let Some(class) = self.receiver_class(&obj.value)
                    && let Some(ty) = lookup_field_type(&class, name)
                {
                    return Some(ty);
                }
                // `Os.sep`, `Math.pi` — a typed stdlib constant read off a
                // module identifier, which resolves to no class.
                match &obj.value {
                    Expr::Ident(recv) => {
                        saule_typeck::sigs::lookup_const(&format!("{recv}.{name}"))
                    }
                    _ => None,
                }
            }
            // `obj?.field` yields nil when the receiver does, so the
            // whole chain is nullable whatever the field says.
            Expr::SafeMember { obj, name } => {
                let class = self.receiver_class(&obj.value)?;
                let inner = lookup_field_type(&class, name)?;
                Some(Type::Nullable(Box::new(strip_nullable(inner))))
            }
            // `t[k]` — the declared element type. Indexing anything else
            // says nothing the declaration can pin down.
            Expr::Index { obj, .. } => match strip_nullable(self.infer_type(&obj.value)?) {
                Type::Table { value, .. } => Some(*value),
                _ => None,
            },
            // Every arm of a `match` produces the same type — the
            // checker enforces it — so the first one speaks for all.
            Expr::Match { arms, .. } => arms.first().and_then(|arm| match &arm.body {
                MatchBody::Expr(e) => self.infer_type(&e.value),
                MatchBody::Block(b) => match b.last().map(|s| &s.value) {
                    Some(Stmt::Expr(e)) => self.infer_type(&e.value),
                    Some(Stmt::Return(rs)) => rs.first().and_then(|e| self.infer_type(&e.value)),
                    _ => None,
                },
            }),
        }
    }

    /// Best-effort return type of an unannotated expression-bodied
    /// lambda, so a generic call like `map(items, s => #s)` can bind the
    /// callback's result type. Mirrors the hover walker's helper of the
    /// same name, including its two limits: block bodies are skipped, and
    /// so is any lambda whose parameters shadow an in-scope binding (the
    /// body is inferred against the enclosing scope, where a shadowed
    /// name would resolve to the wrong local).
    pub(crate) fn infer_lambda_return(&self, params: &[Param], body: &LambdaBody) -> Option<Type> {
        let LambdaBody::Expr(e) = body else {
            return None;
        };
        if params
            .iter()
            .any(|p| self.locals.iter().any(|l| l.name == p.name))
        {
            return None;
        }
        Some(first_value_type(self.infer_type(&e.value)))
    }

    /// Infer a `table<V>` (array literal) or `table<K, V>` (map literal)
    /// from a table constructor's entries so generic natives like
    /// `Table.sort(table<V>, …)` bind their element type. Falls back to a bare
    /// `table` when entries are empty or their types disagree.
    pub(crate) fn infer_table_literal(&self, entries: &[TableEntry]) -> Type {
        let mut value_ty: Option<Type> = None;
        let mut key_ty: Option<Type> = None;
        let mut has_field = false;
        let mut consistent = true;
        for entry in entries {
            let (k, v) = match entry {
                TableEntry::Positional(v) => (None, self.infer_type(&v.value)),
                TableEntry::Field { key, value } => {
                    has_field = true;
                    (self.infer_type(&key.value), self.infer_type(&value.value))
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

    /// Refine the bare structural annotation `table` against the
    /// initializer's inferred shape: `local nums: table = {1, 2}` becomes
    /// `table<integer>`. Leaves the declared type untouched on a mismatch.
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
        let matches_kind = matches!((name.as_str(), &inner), ("table", Type::Table { .. }));
        if matches_kind { inner } else { decl }
    }

    /// Infer the types of a call's positional arguments (in order;
    /// `None` where inference can't produce a type). Named arguments are
    /// skipped — mirrors how the typechecker binds generics from
    /// positional args only.
    pub(crate) fn positional_arg_types(&self, args: &[CallArg]) -> Vec<Option<Type>> {
        args.iter()
            .filter_map(|a| match a {
                CallArg::Positional(e) => Some(self.infer_type(&e.value)),
                CallArg::Named { .. } => None,
            })
            .collect()
    }

    /// Best-effort: figure out which class a member-access receiver
    /// refers to. Mirrors the hover walker's `receiver_class` but
    /// limited to the cases inlay hints actually need.
    pub(crate) fn receiver_class(&self, obj: &Expr) -> Option<String> {
        match obj {
            Expr::Self_ => self.enclosing_class.clone(),
            Expr::Ident(name) => {
                if let Some(local) = self.locals.iter().rev().find(|l| l.name == *name)
                    && let Type::Named(n) = &local.ty
                {
                    return Some(n.clone());
                }
                if with_classes(|r| r.contains_key(name)) {
                    return Some(name.clone());
                }
                None
            }
            Expr::Call { callee, .. } => {
                if let Expr::Ident(n) = &callee.value
                    && with_classes(|r| r.contains_key(n))
                {
                    return Some(n.clone());
                }
                // A link past the first in a chain has a member callee,
                // and its class is that method's declared return type.
                // Without this the argument-name hints stopped after the
                // first modifier: `.font( size: 28.0)` was labelled and
                // `.foregroundStyle(theme.text)` right after it was not.
                if let Expr::Member { obj: inner, name } | Expr::SafeMember { obj: inner, name } =
                    &callee.value
                {
                    let inner_class = self.receiver_class(&inner.value)?;
                    return match lookup_method(&inner_class, name)?.return_ty? {
                        Type::Named(n) => Some(n),
                        // `-> Text?` still names `Text`; the nullability
                        // is the caller's problem, not the lookup's.
                        Type::Nullable(inner) => match *inner {
                            Type::Named(n) => Some(n),
                            _ => None,
                        },
                        _ => None,
                    };
                }
                None
            }
            _ => None,
        }
    }

    /// The type each argument slot expects, with the callee's generics
    /// bound from the argument types. Only the shapes carrying a
    /// signature answer; everything else yields `None` per slot.
    pub(crate) fn expected_arg_types(&self, callee: &Expr, args: &[CallArg]) -> Vec<Option<Type>> {
        let sig = match callee {
            Expr::Ident(name) => with_classes(|r| r.contains_key(name))
                .then(|| lookup_method(name, "init"))
                .flatten()
                .map(method_sig_as_native)
                .or_else(|| self.user_fns.get(name).map(|f| f.sig.clone()))
                .or_else(|| saule_typeck::sigs::lookup(name)),
            Expr::Member { obj, name } => self
                .receiver_class(&obj.value)
                .and_then(|class| {
                    lookup_method(&class, name)
                        .map(method_sig_as_native)
                        .or_else(|| saule_typeck::sigs::lookup(&format!("{class}.{name}")))
                })
                // Stdlib module receiver (`Os.fsInfo`) — no class to
                // resolve, so fall back to the qualified native sig.
                .or_else(|| match &obj.value {
                    Expr::Ident(recv) => saule_typeck::sigs::lookup(&format!("{recv}.{name}")),
                    _ => None,
                }),
            _ => None,
        };
        let Some(sig) = sig else {
            return vec![None; args.len()];
        };
        saule_typeck::sigs::instantiate_params(&sig, &self.positional_arg_types(args))
    }

    pub(crate) fn callee_params(&self, callee: &Expr) -> Option<CalleeParams> {
        match callee {
            Expr::Ident(name) => {
                if with_classes(|r| r.contains_key(name)) {
                    return lookup_method(name, "init").map(|sig| CalleeParams::Named(sig.params));
                }
                if let Some(class) = &self.enclosing_class
                    && let Some(sig) = lookup_method(class, name)
                {
                    return Some(CalleeParams::Named(sig.params));
                }
                if let Some(f) = self.user_fns.get(name) {
                    return Some(CalleeParams::Named(f.params.clone()));
                }
                // Bare native (`assert`, `tonumber`, ...): synthesise
                // names from the registered native sig so the user
                // gets `assert(cond: true, message: ...)`.
                if let Some(native) = saule_typeck::sigs::lookup(name) {
                    return Some(CalleeParams::Native {
                        names: crate::server::native_names::param_names(name, &native),
                        has_variadic: native.variadic.is_some(),
                    });
                }
                None
            }
            Expr::Member { obj, name } => {
                if let Some(class) = self.receiver_class(&obj.value) {
                    if let Some(sig) = lookup_method(&class, name) {
                        return Some(CalleeParams::Named(sig.params));
                    }
                    let qname = format!("{class}.{name}");
                    if let Some(native) = saule_typeck::sigs::lookup(&qname) {
                        return Some(CalleeParams::Native {
                            names: crate::server::native_names::param_names(&qname, &native),
                            has_variadic: native.variadic.is_some(),
                        });
                    }
                }
                // Stdlib module call: `Math.floor(2)` — receiver is a
                // bare identifier registered as a module.
                if let Expr::Ident(recv) = &obj.value {
                    let qname = format!("{recv}.{name}");
                    if let Some(native) = saule_typeck::sigs::lookup(&qname) {
                        return Some(CalleeParams::Native {
                            names: crate::server::native_names::param_names(&qname, &native),
                            has_variadic: native.variadic.is_some(),
                        });
                    }
                }
                None
            }
            _ => None,
        }
    }
}
