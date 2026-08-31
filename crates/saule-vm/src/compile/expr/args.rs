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
    /// * a **literal** default → materialized here;
    /// * any other default → left to the callee, which is told about it by a
    ///   bitmask passed as one extra argument. A default must be evaluated
    ///   in the *callee's* frame (§19's stated trap), and the per-arity entry
    ///   stubs fill a *suffix* — there is no arity meaning "fill slot 1 but
    ///   not slot 2". The mask is what says so; `Compiler::param_entries`
    ///   emits the entry that reads it.
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

        // A parameter left unfilled below the last one supplied is a gap.
        // Two kinds, and only the second needs the callee's help:
        //
        //  * a **scalar literal** default is materialized right here. That
        //    is sound for exactly the reason §19 says a general default is
        //    not — a literal reads nothing from the callee's frame and
        //    nothing from its module scope, and has no side effect to
        //    happen in the wrong place or at the wrong time, so evaluating
        //    it at the call site is observationally identical. The same
        //    argument, and the same restriction, is why a valued enum
        //    variant's value must be a literal.
        //
        //  * anything else — a call, a name, `Distribution.Start` — has to
        //    run in the callee. `Distribution` may not even be in scope
        //    here, and its `NodeId` belongs to the callee's module and would
        //    answer the wrong module's binding and type tables.
        //
        // The second kind used to refuse. It is passed to the callee as a
        // bitmask instead: one bit per slot the caller is *not* supplying,
        // read by the gap entry `param_entries` emits. See there for why an
        // absent slot cannot simply be detected as `nil`.
        let truncated = filled.iter().rposition(Option::is_some).map_or(0, |i| i + 1);
        let needs_callee = |i: usize| {
            filled[i].is_none()
                && params[i]
                    .default
                    .as_ref()
                    .is_some_and(|d| literal_default(&d.value, span).is_none())
        };
        // Only a gap in the *middle* forces this: a trailing one is not a
        // gap at all — the call simply reports a shorter arity and the
        // callee's own per-arity stub fills the tail, which is both correct
        // and cheaper.
        let masked = (0..truncated).any(needs_callee);
        // Once a mask is needed the call passes the **whole** parameter list
        // plus the mask, so the callee's gap entry sits at one fixed arity
        // (`n_params + 1`) instead of colliding with an ordinary one. That
        // pulls the trailing slots in too, so they need bits of their own —
        // computing the mask over the truncated range only would leave a
        // trailing default silently `nil`.
        let n = if masked { params.len() } else { truncated };
        let mut mask: i64 = 0;
        if masked {
            for i in 0..n {
                if needs_callee(i) {
                    mask |= 1 << i;
                }
            }
        }
        if masked && params.len() > 63 {
            return Err(CompileError::unsupported(
                "a signature with more than 63 parameters and a default",
                span.clone(),
            ));
        }
        // `B` in the call instruction is a `u8` holding `n_args + 1`, and the
        // mask is one more argument on top of the parameters.
        if masked && params.len() + 2 > u8::MAX as usize {
            return Err(CompileError::unsupported(
                "a call that skips a defaulted parameter of a 254-parameter signature",
                span.clone(),
            ));
        }

        let mut order = Vec::with_capacity(n + usize::from(masked));
        let mut gaps = Vec::new();
        for (i, slot) in filled.iter().take(n).enumerate() {
            match slot {
                Some(a) => order.push(ArgSlot::Given(*a)),
                None if params[i].default.is_some() => {
                    let d = params[i].default.as_ref().expect("matched `is_some`");
                    // The literal is rebuilt from the call site's span rather
                    // than cloned from the declaration, because the
                    // declaration's `NodeId` belongs to the *callee's* module
                    // and would answer the wrong module's tables when the
                    // callee is imported.
                    let fill = match literal_default(&d.value, span) {
                        Some(lit) => lit,
                        // Left to the callee: its gap entry sees this slot's
                        // bit set and runs the real default there. The
                        // placeholder is only what occupies the argument
                        // register until it does.
                        None => {
                            debug_assert!(mask & (1 << i) != 0);
                            Expr::Nil
                        }
                    };
                    order.push(ArgSlot::Nil(gaps.len()));
                    gaps.push(Spanned::new(fill, span.clone()));
                }
                None => {
                    order.push(ArgSlot::Nil(gaps.len()));
                    gaps.push(Spanned::new(Expr::Nil, span.clone()));
                }
            }
        }
        if masked {
            // The mask rides as one more ordinary argument, so every call
            // emitter below — `CALLK`, `CALLSTAT`, `CALLM`, the constructor —
            // needs to know nothing about any of this. The arity it produces
            // is what selects the callee's gap entry.
            order.push(ArgSlot::Nil(gaps.len()));
            gaps.push(Spanned::new(Expr::Int(mask), span.clone()));
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
                    return self.method_param_list(c, "init");
                }
                if let Some(Binding::ClassStatic { class, .. }) = self.binding(callee.id)
                    && let Some(c) = self.layouts.get(class).or(self.f.current_class)
                {
                    return self.method_param_list(c, n);
                }
                self.callee_params.get(&CalleeKey::Function(n.clone()))
            }
            Expr::Member { obj, name } => {
                let c = self.class_named_by(obj).or_else(|| self.class_of_expr(obj))?;
                self.method_param_list(c, name)
            }
            // `obj?.m(...)`. The receiver's proved type is *nullable*, so
            // `class_of_expr` — which only reads a `Type::Named` — answers
            // nothing for it, and every named argument or trailing block on
            // a safe call refused. `context.navigator()?.push() do … end` is
            // the shape.
            //
            // `class_of_nullable_expr` is the same lookup the safe-call
            // emitter already uses to pick its vtable slot, so the two agree
            // about which class this is by sharing the answer rather than by
            // care. Binding against it is sound for the same reason that
            // vtable slot is: `safe_method_call_to` guards the whole call on
            // nil, so the arguments are bound only on the branch where the
            // receiver really is an instance of that class.
            Expr::SafeMember { obj, name } => {
                let c = self
                    .class_named_by(obj)
                    .or_else(|| self.class_of_nullable_expr(obj))?;
                self.method_param_list(c, name)
            }
            _ => None,
        }
    }

    /// `callee_params` for a method of `class`, **or of an ancestor**.
    ///
    /// `callee_params` is keyed by the class that *declares* a method, so a
    /// lookup against the receiver's own class misses everything it
    /// inherits. `Text(…).padding(insets: …)` is the shape: `padding` is
    /// declared on `View`, and the miss surfaced as `a named argument to a
    /// callee the compiler cannot identify` — a refusal that had nothing to
    /// do with the argument.
    ///
    /// Walking upward is the same rule the vtable already follows, and an
    /// override is found before its parent because the subclass's own entry
    /// is tried first.
    fn method_param_list(
        &self,
        class: crate::chunk::ClassIdx,
        name: &str,
    ) -> Option<&Vec<saule_ast::Param>> {
        use crate::compile::ctx::CalleeKey;
        let mut c = Some(class);
        while let Some(idx) = c {
            if let Some(params) = self.callee_params.get(&CalleeKey::Method(idx, name.into())) {
                return Some(params);
            }
            c = self.chunk.classes.get(idx as usize).and_then(|p| p.parent);
        }
        None
    }
}

/// The call-site-safe form of a default expression, if it has one.
///
/// A scalar literal reads nothing from the callee's frame or module scope
/// and has no side effect, so evaluating it at the call site is
/// observationally identical to evaluating it in the callee. Everything else
/// answers `None` and is left for the callee's gap entry to run.
///
/// Rebuilt rather than cloned: a cloned node keeps the *callee* module's
/// `NodeId`, which would answer the wrong module's binding and type tables.
fn literal_default(d: &Expr, span: &Range<usize>) -> Option<Expr> {
    Some(match d {
        Expr::Int(n) => Expr::Int(*n),
        Expr::Float(f) => Expr::Float(*f),
        Expr::Bool(b) => Expr::Bool(*b),
        Expr::Str(t) => Expr::Str(t.clone()),
        Expr::Nil => Expr::Nil,
        // `-1` and `-2.5`: a negated numeric literal is still a literal by
        // the argument above, and a plausible enough default that excluding
        // it would be an arbitrary wart. Rebuilt rather than folded, so the
        // operand keeps whatever overflow behaviour the ordinary unary path
        // already has for `i64::MIN`.
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
        _ => return None,
    })
}
