//! Compile-time argument binding at the call site (§19).
//!
//! Named and defaulted arguments are resolved here, so the call itself is
//! always plain and positional. The rule that makes it safe is evaluation
//! order: arguments are *evaluated* left to right as written, then moved
//! into parameter order.

use std::ops::Range;

use saule_ast::{Expr, Spanned};
use saule_semantic::Binding;

use super::CompileError;
use super::super::ctx::Compiler;

/// Where one argument slot's value comes from, after §19's reordering.
pub(crate) enum ArgSlot {
    /// Index into the call's own `args`.
    Given(usize),
    /// Index into the synthesized-`nil` list, for a parameter a named call
    /// skipped over.
    Nil(usize),
}

impl Compiler<'_> {

    /// Put a call's arguments into **parameter order** (§19).
    ///
    /// The slot assignment is `saule_ast::resolve_arg_slots` — the very
    /// function the typechecker uses — so the two cannot disagree about
    /// which parameter an argument fills, including the trailing-block rule.
    ///
    /// A parameter left unfilled *below* the last one supplied is a gap:
    ///
    /// * nullable, no default → a synthesized `nil`, which is exactly what
    ///   the callee would have left there;
    /// * has a **default** → refused. A default must be evaluated in the
    ///   *callee's* frame (§19's stated trap), and the entry stubs can only
    ///   fill a suffix — there is no entry point meaning "fill slot 1 but
    ///   not slot 2".
    ///
    /// A trailing parameter that is simply not passed is not a gap: the call
    /// reports a shorter arity and the callee's own stub fills it.
    pub(crate) fn reorder_args(
        &self,
        args: &[saule_ast::CallArg],
        params: &[saule_ast::Param],
        span: &Range<usize>,
    ) -> Result<(Vec<ArgSlot>, Vec<Spanned<Expr>>), CompileError> {
        let slots: Vec<saule_ast::ParamSlot<'_>> = params
            .iter()
            .map(|p| saule_ast::ParamSlot::new(&p.name, &p.ty))
            .collect();
        let assigned = saule_ast::resolve_arg_slots(args, &slots);

        let mut filled: Vec<Option<usize>> = vec![None; params.len()];
        for (arg_i, slot) in assigned.iter().enumerate() {
            // An argument the resolver could not place, or one that fills a
            // slot twice. The typechecker reports both; refusing keeps this
            // from inventing a position.
            let Some(slot) = slot.filter(|s| filled[*s].is_none()) else {
                return Err(CompileError::unsupported(
                    "an argument that fills no parameter, or fills one twice",
                    span.clone(),
                ));
            };
            filled[slot] = Some(arg_i);
        }

        let n = filled.iter().rposition(Option::is_some).map_or(0, |i| i + 1);
        let mut order = Vec::with_capacity(n);
        let mut gaps = Vec::new();
        for (i, slot) in filled.iter().take(n).enumerate() {
            match slot {
                Some(a) => order.push(ArgSlot::Given(*a)),
                // A parameter skipped in the *middle* of the list, which
                // has a default. The entry stubs cannot help: they fill a
                // *suffix*, and there is no entry point meaning "fill slot 1
                // but not slot 2".
                //
                // A **scalar literal** default is materialized here instead.
                // That is sound for exactly the reason §19 says a general
                // default is not: a literal reads nothing from the callee's
                // frame and nothing from the callee's module scope, and it
                // has no side effect to happen in the wrong place or at the
                // wrong time — so evaluating it at the call site is
                // observationally identical to evaluating it in the callee.
                // The same argument, and the same restriction, is why a
                // valued enum variant's value must be a literal.
                //
                // The node is rebuilt from the call site's span rather than
                // cloned from the declaration, because the declaration's
                // `NodeId` belongs to the *callee's* module and would answer
                // the wrong module's binding and type tables when the callee
                // is imported.
                None if params[i].default.is_some() => {
                    let d = params[i].default.as_ref().expect("matched `is_some`");
                    let lit = match &d.value {
                        Expr::Int(n) => Expr::Int(*n),
                        Expr::Float(f) => Expr::Float(*f),
                        Expr::Bool(b) => Expr::Bool(*b),
                        Expr::Str(t) => Expr::Str(t.clone()),
                        Expr::Nil => Expr::Nil,
                        // `-1` and `-2.5`: a negated numeric literal is
                        // still a literal by the argument above, and a
                        // plausible enough default that excluding it would be
                        // an arbitrary wart. Rebuilt rather than folded, so
                        // the operand keeps whatever overflow behaviour the
                        // ordinary unary path already has for `i64::MIN`.
                        Expr::Unary { op: saule_ast::UnaryOp::Neg, rhs }
                            if matches!(rhs.value, Expr::Int(_) | Expr::Float(_)) =>
                        {
                            let inner = match &rhs.value {
                                Expr::Int(n) => Expr::Int(*n),
                                Expr::Float(f) => Expr::Float(*f),
                                _ => unreachable!("guarded by the `matches!` above"),
                            };
                            Expr::Unary {
                                op: saule_ast::UnaryOp::Neg,
                                rhs: Box::new(Spanned::new(inner, span.clone())),
                            }
                        }
                        // Anything else — a call, a name, a table literal —
                        // may read the callee's scope or have a side effect,
                        // so it keeps refusing rather than being guessed at
                        // from here.
                        _ => {
                            return Err(CompileError::unsupported(
                                "a skipped parameter whose non-literal default must run in the callee",
                                span.clone(),
                            ));
                        }
                    };
                    order.push(ArgSlot::Nil(gaps.len()));
                    gaps.push(Spanned::new(lit, span.clone()));
                }
                None => {
                    order.push(ArgSlot::Nil(gaps.len()));
                    gaps.push(Spanned::new(Expr::Nil, span.clone()));
                }
            }
        }
        Ok((order, gaps))
    }

    /// The declared parameters of whatever `callee` names, when the compiler
    /// can identify it — a top-level `fn`, a class's constructor, a static
    /// or instance method. `None` for a callee that is only a value, where
    /// there is no declaration to read names from.
    pub(crate) fn callee_param_list(&self, callee: &Spanned<Expr>) -> Option<&Vec<saule_ast::Param>> {
        use crate::compile::ctx::CalleeKey;
        match &callee.value {
            Expr::Ident(n) => {
                // `ClassName(args)` is a constructor, so its parameters are
                // `init`'s.
                if let Some(c) = self.layouts.get(n).filter(|_| self.not_shadowed(n)) {
                    return self.callee_params.get(&CalleeKey::Method(c, "init".into()));
                }
                if let Some(Binding::ClassStatic { class, .. }) = self.binding(callee.id)
                    && let Some(c) = self.layouts.get(class).or(self.f.current_class)
                {
                    return self.callee_params.get(&CalleeKey::Method(c, n.clone()));
                }
                self.callee_params.get(&CalleeKey::Function(n.clone()))
            }
            Expr::Member { obj, name } => {
                let c = self.class_named_by(obj).or_else(|| self.class_of_expr(obj))?;
                self.callee_params.get(&CalleeKey::Method(c, name.clone()))
            }
            _ => None,
        }
    }
}
