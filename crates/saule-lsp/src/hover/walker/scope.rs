//! Scope bookkeeping: entering functions and lambdas, recording
//! locals and loop variables, and resolving an identifier back to
//! the binding it names.

use crate::hover::render::{
    render_class_full, render_enum_from_registry, render_function_sig,
    render_interface_from_registry, render_native_sig_full, render_stdlib_module, render_type,
    with_doc,
};
use crate::hover::util::{
    collect_named_heads, contains, is_primitive, locate_word_in, named_type, resolve_member,
};
use saule_ast::{Expr, Param, Type};
use saule_semantic::{
    lookup_field_type, super_init_target, with_classes, with_enums, with_interfaces,
};
use std::ops::Range;

use super::*;

impl<'a> Cx<'a> {
    /// Record `md` as the hover for `span` when `span` contains the
    /// cursor and is strictly narrower than any prior match.
    pub(crate) fn record(&mut self, span: Range<usize>, md: String) {
        if !contains(&span, self.offset) {
            return;
        }
        let new_w = span.end.saturating_sub(span.start);
        if let Some(b) = &self.best {
            let cur_w = b.span.end.saturating_sub(b.span.start);
            if new_w >= cur_w {
                return;
            }
        }
        self.best = Some(Hit { span, md });
    }

    /// The span of a declaration's *name* token, for anchoring the
    /// declaration's own hover.
    ///
    /// Recording a declaration against its full span — header *and*
    /// body — makes every byte inside that body which no narrower node
    /// claims resolve to the declaration's signature: a comment, a blank
    /// line, the gap between two arguments. The popup then presents a
    /// confident description of a symbol the cursor is nowhere near,
    /// which is the single largest source of hovers that read as random.
    /// Anchored to the name token instead, those positions correctly
    /// produce no hover at all.
    ///
    /// The span runs from the start of the declaration to the end of its
    /// name, so the introducing keyword and any `export` / `local` /
    /// `static` modifier hover as part of the thing they introduce —
    /// pointing at `fn` and getting nothing would be its own small
    /// annoyance. It is the *body* that must be excluded, and is.
    ///
    /// `head_end` bounds the search to the declaration header so a body
    /// that happens to mention the name (recursion, a same-named local)
    /// can't win the match. Falls back to the full span when the name
    /// isn't locatable, which is what keeps the source-less [`hover_at`]
    /// entry point answering as it always has.
    pub(crate) fn decl_name_span(
        &self,
        span: &Range<usize>,
        name: &str,
        head_end: usize,
    ) -> Range<usize> {
        let end = head_end.clamp(span.start, span.end);
        match locate_word_in(self.source, &(span.start..end), name) {
            Some(m) => span.start..m.end,
            None => span.clone(),
        }
    }

    /// What `for … in iter` binds, given how many variables the loop
    /// declares. Returns one type per variable, shortest-first; an empty
    /// vec when the iterable's type says nothing useful.
    ///
    /// A single variable takes the element: `for item in items` over
    /// `table<T>` binds `T`. Two variables take key and value, with the
    /// key defaulting to `integer` for an array-style `table<V>` that
    /// declares no key type.
    pub(crate) fn iteration_types(&self, iter: &Expr, vars: usize) -> Vec<Type> {
        let ty = match self.infer_init_type(iter) {
            Some(Type::Nullable(inner)) => *inner,
            Some(t) => t,
            None => return Vec::new(),
        };
        let Type::Table { key, value } = ty else {
            return Vec::new();
        };
        if vars >= 2 {
            let k = key
                .map(|k| *k)
                .unwrap_or_else(|| Type::Named("integer".into()));
            vec![k, *value]
        } else {
            vec![*value]
        }
    }

    /// Record the hover for a loop variable at its binding site, found
    /// by locating the name inside the loop header `search`. Renders
    /// identically to a use of the same variable inside the body, so
    /// `for i` and the `i` two lines down agree.
    ///
    /// Also makes the type ascription's own head (`for x: Color in ...`)
    /// hoverable, matching what parameters and locals already do.
    pub(crate) fn record_loop_var(&mut self, search: &Range<usize>, name: &str, ty: &Type) {
        let Some(span) = locate_word_in(self.source, search, name) else {
            return;
        };
        self.record(
            span,
            format!(
                "```saule\n(loop var) {name}: {ty}\n```",
                ty = render_type(ty)
            ),
        );
        self.record_type_idents_in(ty, search);
    }

    /// The `---` doc comment attached to the declaration starting at
    /// `anchor`, or `None` when it has none (or an empty one).
    ///
    /// Callers reach for this once per declaration and thread the
    /// result through to the parameter loop, so a hover on a parameter
    /// can surface its `@param` line without re-scanning the source.
    pub(crate) fn doc_at(&self, anchor: usize) -> Option<saule_docs::DocBlock> {
        saule_docs::extract(self.source, anchor).filter(|d| !d.is_empty())
    }

    /// Walk into a lambda body, *keeping* the enclosing scope and
    /// stacking the lambda's own parameters on top of it.
    ///
    /// A lambda is a closure: the names around it are exactly the names
    /// its body can use. Starting it with a fresh scope — as this used
    /// to — meant every captured name went unresolved inside the body,
    /// and hover fell through to the next node out. So a cursor on
    /// `rebuild()` or `scratch` inside `onChanged: fn(next: boolean)`
    /// answered with the *lambda's own type*, `(expr): fn(boolean) ->
    /// any`, which describes the callback rather than the token the
    /// cursor is on.
    ///
    /// Function and method declarations still reset — see
    /// [`Cx::enter_function_with_return`]. Their bodies genuinely do not
    /// see a sibling method's locals.
    ///
    /// The *return* type does not carry in, though: a `return` inside a
    /// lambda returns from the lambda. Inheriting the enclosing
    /// function's annotation made the keyword report a type the
    /// statement has nothing to do with, so an unannotated lambda
    /// reports none rather than someone else's.
    pub(crate) fn enter_lambda(
        &mut self,
        params: &[Param],
        return_ty: Option<&Type>,
        body: impl FnOnce(&mut Self),
    ) {
        let mark = self.locals.len();
        let saved_ret = self.enclosing_return_ty.take();
        self.enclosing_return_ty = return_ty.cloned();
        for p in params {
            self.locals.push(LocalVar {
                name: p.name.clone(),
                ty: p.ty.clone(),
                kind: LocalKind::Param,
            });
        }
        body(self);
        self.locals.truncate(mark);
        self.enclosing_return_ty = saved_ret;
    }

    /// Like [`Cx::enter_lambda`] but for a `fn` / method declaration:
    /// the outer scope is replaced rather than extended, since a method
    /// body cannot see a sibling's locals. Also tracks the declared
    /// return type so a hover on the `return` keyword inside the body
    /// can surface it.
    pub(crate) fn enter_function_with_return(
        &mut self,
        params: &[Param],
        return_ty: Option<&Type>,
        body: impl FnOnce(&mut Self),
    ) {
        let saved_locals = std::mem::take(&mut self.locals);
        let saved_ret = self.enclosing_return_ty.take();
        self.enclosing_return_ty = return_ty.cloned();
        for p in params {
            self.locals.push(LocalVar {
                name: p.name.clone(),
                ty: p.ty.clone(),
                kind: LocalKind::Param,
            });
        }
        body(self);
        self.locals = saved_locals;
        self.enclosing_return_ty = saved_ret;
    }

    /// Surface hover info for every named-type head reachable from
    /// `ty` (including nullable / table / tuple / function components)
    /// that appears as an identifier inside `search_span` of the
    /// source. Used to make type ascriptions on params, class fields,
    /// return types, etc. resolve the same way bare identifier hovers
    /// do — without baking individual `ty_span` fields into every AST
    /// node that carries a `Type`.
    pub(crate) fn record_type_idents_in(&mut self, ty: &Type, search_span: &Range<usize>) {
        let mut names: Vec<String> = Vec::new();
        collect_named_heads(ty, &mut names);
        for name in names {
            // Skip primitives — they have no useful hover and would
            // mask the parent expression's blurb otherwise.
            if is_primitive(&name) {
                continue;
            }
            if let Some(span) = locate_word_in(self.source, search_span, &name) {
                if !contains(&span, self.offset) {
                    continue;
                }
                if let Some(md) = self.resolve_ident(&name) {
                    self.record(span, md);
                }
            }
        }
    }

    /// Like [`record_type_idents_in`] but takes a list of bare names
    /// (e.g. the parents listed after `extends` / `implements`) and
    /// finds each one inside `search_span`. Names may legitimately
    /// repeat, so we use the first occurrence — that's good enough
    /// for the cases we care about (class headers don't reference the
    /// same parent twice).
    pub(crate) fn record_named_idents_in(&mut self, names: &[String], search_span: &Range<usize>) {
        for name in names {
            if let Some(span) = locate_word_in(self.source, search_span, name) {
                if !contains(&span, self.offset) {
                    continue;
                }
                if let Some(md) = self.resolve_ident(name) {
                    self.record(span, md);
                }
            }
        }
    }

    /// Look up `name` in the current local scope (innermost first).
    /// Returns `None` for free identifiers — the caller falls through
    /// to the registry / native-sig path.
    pub(crate) fn lookup_local(&self, name: &str) -> Option<&LocalVar> {
        self.locals.iter().rev().find(|l| l.name == name)
    }

    /// Resolve a bare identifier to a hover blurb. Tries class /
    /// interface / enum registries (which include builtins and
    /// seed-imported classes), then falls back to the native-signature
    /// registry for stdlib free functions and modules, then finally to
    /// the per-request import context for top-level functions imported
    /// from another `.sau` file. Returns `None` for names we can't tie
    /// to anything (locals, parameters, unknown idents).
    pub(crate) fn resolve_ident(&self, name: &str) -> Option<String> {
        // Type declarations resolve through the registries, which hold
        // no source text — pull any `---` block from the doc index so a
        // hover on a *usage* reads the same as one on the declaration.
        let doc = self.imports.docs.get(name);
        if let Some(info) = with_classes(|r| r.get(name).cloned()) {
            return Some(with_doc(render_class_full(name, &info), doc));
        }
        if with_interfaces(|r| r.contains_key(name)) {
            let extends = with_interfaces(|r| r.get(name).cloned()).unwrap_or_default();
            return Some(with_doc(
                render_interface_from_registry(name, &extends),
                doc,
            ));
        }
        if with_enums(|r| r.contains_key(name)) {
            let info = with_enums(|r| r.get(name).cloned())?;
            let variants: Vec<(String, usize)> = info
                .variants
                .iter()
                .map(|(n, v)| (n.clone(), v.arity()))
                .collect();
            return Some(with_doc(render_enum_from_registry(name, &variants), doc));
        }
        // Free function declared at top level in *this* module. Checked
        // ahead of the native signatures because a local declaration
        // shadows a stdlib name of the same spelling for the rest of
        // the file, and the hover should follow the same rule.
        if let Some(f) = self.module_fns.get(name) {
            return Some(with_doc(f.md.clone(), doc));
        }
        if let Some(sig) = saule_typeck::sigs::lookup(name) {
            return Some(format!(
                "```saule\nfn {name}{}\n```",
                render_native_sig_full(&sig)
            ));
        }
        if saule_typeck::sigs::is_value_type(name) && saule_semantic::prelude::contains(name) {
            return Some(render_stdlib_module(name, "type"));
        }
        if saule_typeck::sigs::is_module(name) && saule_semantic::prelude::contains(name) {
            return Some(render_stdlib_module(name, "module"));
        }
        // Bare identifier inside a class body — try resolving as a
        // method or field of the enclosing class. Covers calling a
        // sibling static (`help()` from inside `Main.main()`) and
        // referencing a sibling instance member without `self.`.
        if let Some(class) = &self.enclosing_class
            && let Some(md) = resolve_member(class, name, false, &self.imports.docs)
        {
            return Some(md);
        }
        // A top-level function imported from another file — final
        // fallback. `analyze_with_seed` registers these under the local
        // alias, following re-export barrels, so a name reached through
        // `import * from UIKit` resolves the same as a direct one.
        if let Some(sig) = saule_semantic::lookup_function(name) {
            return Some(with_doc(
                render_function_sig(name, &sig.type_params, &sig.params, sig.return_ty.as_ref()),
                doc,
            ));
        }
        None
    }

    /// `(owner, sig)` of the constructor a `self.super(...)` written in
    /// the current class delegates to, when `name` / `obj` are exactly
    /// that form. `None` for every other member access.
    pub(crate) fn super_target(
        &self,
        name: &str,
        obj: &Expr,
    ) -> Option<(String, saule_semantic::MethodSig)> {
        if name != "super" || !matches!(obj, Expr::Self_) {
            return None;
        }
        super_init_target(self.enclosing_class.as_deref()?)
    }

    /// Best-effort: figure out which class a member-access receiver
    /// refers to. Handles `self`, bare class-name references (static
    /// access like `Math.sqrt`), and chained `Class.foo.bar` where the
    /// inner field's declared type is a named class.
    pub(crate) fn receiver_class(&self, obj: &Expr) -> Option<String> {
        match obj {
            Expr::Self_ => self.enclosing_class.clone(),
            Expr::Ident(name) => {
                // Locals first — `newEntry.setDone(...)` resolves
                // through the local's declared/inferred type.
                if let Some(local) = self.lookup_local(name) {
                    return named_type(&local.ty);
                }
                if with_classes(|r| r.contains_key(name))
                    || with_enums(|r| r.contains_key(name))
                    || ((saule_typeck::sigs::is_module(name)
                        || saule_typeck::sigs::is_value_type(name))
                        && saule_semantic::prelude::contains(name))
                {
                    Some(name.clone())
                } else {
                    None
                }
            }
            Expr::Member { obj: inner, name } | Expr::SafeMember { obj: inner, name } => {
                let inner_class = self.receiver_class(&inner.value)?;
                let ty = lookup_field_type(&inner_class, name)?;
                named_type(&ty)
            }
            // `x!.foo` — the unwrap doesn't change which class the
            // receiver names, it only drops the nullability, and
            // `named_type` already looks through `Nullable`.
            //
            // This arm matters more than it looks: a nullable has to be
            // unwrapped before anything can be read off it, so every
            // hover through one — which is most tree-walking code —
            // used to fall through to `None` here and get answered with
            // the *enclosing function* instead. That isn't a missing
            // hover, it's a wrong one: it names a symbol that has
            // nothing to do with the token under the cursor.
            Expr::ForceUnwrap(inner) => self.receiver_class(&inner.value),
            // `(x as T).foo` — the cast names the class outright, which
            // is the whole reason to have written it.
            Expr::Cast { ty, .. } => named_type(ty),
            Expr::Call { callee, .. } => {
                // `Class(args).foo` — constructor call returns the class.
                if let Expr::Ident(name) = &callee.value
                    && with_classes(|r| r.contains_key(name))
                {
                    return Some(name.clone());
                }
                None
            }
            Expr::Index { obj: inner, .. } => {
                // `tbl[i].foo` — the receiver's class is the element
                // type of the indexed `table<…>` (or the `value` half
                // of a `table<K, V>`). Walk the inner expression to
                // get its declared type, then peel one level.
                let ty = self.infer_init_type(&inner.value)?;
                match ty {
                    Type::Table { value, .. } => named_type(&value),
                    other => named_type(&other),
                }
            }
            _ => None,
        }
    }
}
