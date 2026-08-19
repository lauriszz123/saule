//! Class codegen (`VM_DESIGN.md` §8).
//!
//! Pass 1 ([`super::layout`]) already decided every field slot and vtable
//! slot. This module compiles the bodies into those slots and emits the
//! instructions that use them.
//!
//! ## What a member access costs
//!
//! `p.health` becomes `GETF r3 r1 4` — one indexed load out of a `Vec<Value>`
//! — whenever the typechecker proved `p`'s class. No hash, no string, no
//! chain walk. When it did not, the access falls back to the dynamic form
//! rather than guessing, because a wrong slot reads a *different field* and
//! nothing would notice.
//!
//! ## Construction
//!
//! `ClassName(args)` is three steps: `NEW` allocates the instance with every
//! slot nil, a synthetic `field_init` proto fills in the declared defaults
//! parent-first (mirroring the tree-walker's `init_fields` recursion), and
//! the constructor runs as an ordinary `CALLM` on the `init` slot.
//!
//! The instance is built in its own register and the constructor called on a
//! *copy*, because `CALLM` writes its result over the receiver — and `init`
//! returns nil, which would otherwise discard the object being built.

use std::ops::Range;

use saule_ast::{ClassMember, Decl, Expr, Spanned};

use super::CompileError;
use super::ctx::Compiler;
use crate::chunk::ClassIdx;
use crate::op::{Instruction, Op};

impl Compiler<'_> {
    /// Compile every method of a `class` into the slots Pass 1 assigned.
    pub fn class_decl(&mut self, d: &Spanned<Decl>) -> Result<(), CompileError> {
        let span = &d.span;
        let Decl::Class { name, members, .. } = &d.value else {
            unreachable!("class_decl called with a non-class");
        };
        let Some(idx) = self.layouts.get(name) else {
            return Err(CompileError::unsupported("this class", span.clone()));
        };

        // Instance methods, into their vtable slots. The slot map is read
        // out first: compiling a body borrows the compiler mutably.
        let vslots: Vec<(u16, usize)> = self.chunk.classes[idx as usize]
            .member_of_vslot
            .iter()
            .map(|(a, b)| (*a, *b))
            .collect();
        for (slot, member_i) in vslots {
            let ClassMember::Method(m) = &members[member_i].value else {
                continue;
            };
            let proto = self.method_proto(name, idx, m, /* has_self */ true, span)?;
            self.chunk.classes_mut()[idx as usize].vtable[slot as usize] = proto;
        }

        // Static methods have no receiver, so parameter 0 is the user's.
        let sslots: Vec<usize> = self.chunk.classes[idx as usize].member_of_sslot.clone();
        for (slot, member_i) in sslots.into_iter().enumerate() {
            let ClassMember::Method(m) = &members[member_i].value else {
                continue;
            };
            let proto = self.method_proto(name, idx, m, /* has_self */ false, span)?;
            self.chunk.classes_mut()[idx as usize].static_methods[slot] = proto;
        }

        self.field_init_proto(idx, members, span)?;
        self.static_defaults(idx, members, span)?;
        Ok(())
    }

    /// Compile every method of an `enum` into a proto, and record its index.
    ///
    /// An enum method is an instance method with no layout behind it: `self`
    /// is the *variant object*, which has no field slots, so every access
    /// through it takes the dynamic path (`GETFX` / `CALLMX`) and defers to
    /// the tree-walker's own `read_member` — which is what makes `.value`
    /// and `.name` mean the same thing under both engines by construction.
    ///
    /// Dispatch is dynamic too: `CALLMX` on a variant receiver reaches
    /// `dispatch_member_call_multi`, which finds the `MethodRef` the VM's
    /// start-up pass put in the runtime `EnumObject`. There is no vtable to
    /// build because an enum cannot be extended, so a name probe is the
    /// whole of it.
    pub fn enum_decl(&mut self, d: &Spanned<Decl>) -> Result<(), CompileError> {
        let span = &d.span;
        let Decl::Enum { name, methods, .. } = &d.value else {
            unreachable!("enum_decl called with a non-enum");
        };
        if methods.is_empty() {
            return Ok(());
        }
        let Some(idx) = self.layouts.enum_of(name) else {
            return Err(CompileError::unsupported(
                "an enum the compiler did not lay out",
                span.clone(),
            ));
        };
        for m in methods {
            let proto = self.enum_method_proto(name, m, span)?;
            self.chunk.enums_mut()[idx as usize]
                .methods
                .insert(std::rc::Rc::from(m.name.as_str()), proto);
        }
        Ok(())
    }

    /// One enum method body. Shaped like [`Self::method_proto`] with
    /// `has_self` set and no owning class — `self` is a variant, not an
    /// instance, so there is no `current_class` for a static or a field slot
    /// to resolve against.
    fn enum_method_proto(
        &mut self,
        enum_name: &str,
        m: &saule_ast::Method,
        span: &Range<usize>,
    ) -> Result<u32, CompileError> {
        self.push_function(Some(&format!("{enum_name}.{}", m.name)));
        self.f.in_method = true;

        let outcome = (|| -> Result<(), CompileError> {
            let n = m.params.len() as u16 + 1;
            let label = self.func_label();
            self.f
                .regs
                .reserve_params(n)
                .map_err(|o| o.at(&label, span.clone()))?;
            self.f.n_params = n as u8;
            self.f.declare("self", 0);
            for (i, p) in m.params.iter().enumerate() {
                self.f.declare(&p.name, 1 + i as u16);
            }
            self.f.entries = self.param_entries(&m.params, 1, span)?;
            self.coerce_params(&m.params, 1, span)?;
            for st in &m.body {
                self.stmt(st)?;
            }
            Ok(())
        })();

        let proto = self.pop_function(span);
        outcome?;
        Ok(self.chunk.add_proto(proto))
    }

    /// Compile one method body into a proto.
    fn method_proto(
        &mut self,
        class_name: &str,
        class: ClassIdx,
        m: &saule_ast::Method,
        has_self: bool,
        span: &Range<usize>,
    ) -> Result<u32, CompileError> {
        self.push_function(Some(&format!("{class_name}.{}", m.name)));
        self.f.current_class = Some(class);
        self.f.in_method = has_self;

        let outcome = (|| -> Result<(), CompileError> {
            // `self` is parameter 0 — the receiver already sits at the base
            // of the frame because `CALLM` puts it there (§6.2), so nothing
            // is copied on entry.
            let n = m.params.len() as u16 + u16::from(has_self);
            let label = self.func_label();
            self.f
                .regs
                .reserve_params(n)
                .map_err(|o| o.at(&label, span.clone()))?;
            self.f.n_params = n as u8;
            let first = u16::from(has_self);
            if has_self {
                self.f.declare("self", 0);
            }
            for (i, p) in m.params.iter().enumerate() {
                self.f.declare(&p.name, first + i as u16);
            }
            self.f.entries = self.param_entries(&m.params, first, span)?;
            self.coerce_params(&m.params, first, span)?;
            for st in &m.body {
                self.stmt(st)?;
            }
            Ok(())
        })();

        let proto = self.pop_function(span);
        outcome?;
        Ok(self.chunk.add_proto(proto))
    }

    /// Build the synthetic proto that fills in declared field defaults.
    ///
    /// Runs the parent's first, mirroring `init_fields`'s recursion in the
    /// tree-walker, so a subclass's defaults can overwrite an inherited one
    /// and the ordering is observably the same.
    fn field_init_proto(
        &mut self,
        class: ClassIdx,
        members: &[Spanned<ClassMember>],
        span: &Range<usize>,
    ) -> Result<(), CompileError> {
        let defaults: Vec<(u16, &Spanned<Expr>)> = members
            .iter()
            .filter_map(|m| match &m.value {
                ClassMember::Field {
                    name,
                    default: Some(d),
                    ..
                } => self.chunk.classes[class as usize]
                    .layout
                    .slot(name)
                    .map(|slot| (slot, d)),
                _ => None,
            })
            .collect();

        // The parent's field initializer, with the module that declared it —
        // a proto index alone would name the wrong function once the parent
        // lives in another module.
        let parent_init = self.chunk.classes[class as usize].parent.and_then(|p| {
            let pc = &self.chunk.classes[p as usize];
            pc.field_init.map(|f| (pc.module, f))
        });
        if defaults.is_empty() && parent_init.is_none() {
            return Ok(());
        }

        self.push_function(Some("<field-init>"));
        self.f.current_class = Some(class);
        self.f.in_method = true;
        let outcome = (|| -> Result<(), CompileError> {
            let label = self.func_label();
            self.f
                .regs
                .reserve_params(1)
                .map_err(|o| o.at(&label, span.clone()))?;
            self.f.n_params = 1;
            self.f.declare("self", 0);

            if let Some((pmod, parent)) = parent_init {
                let m = self.mark();
                let base = self.alloc(span)?;
                let a = self.reg8(base, span)?;
                self.emit(Instruction::abc(Op::MOVE, a, 0, 0), span);
                self.emit(Instruction::abc(Op::CALLK, a, 2, 1), span);
                let t = self.call_target(pmod, parent, span)?;
                self.emit(Instruction::ax_of(Op::EXTRAARG, t), span);
                self.free_to(m);
            }
            for (slot, d) in defaults {
                let m = self.mark();
                let r = self.expr_tmp(d)?;
                let c = self.reg8(r, span)?;
                self.emit(Instruction::abc(Op::SETF, 0, slot as u8, c), span);
                self.free_to(m);
            }
            Ok(())
        })();
        let proto = self.pop_function(span);
        outcome?;
        let idx = self.chunk.add_proto(proto);
        self.chunk.classes_mut()[class as usize].field_init = Some(idx);
        Ok(())
    }

    /// Evaluate static-field initializers into their slots, in the module
    /// body, where the tree-walker also evaluates them.
    fn static_defaults(
        &mut self,
        class: ClassIdx,
        members: &[Spanned<ClassMember>],
        span: &Range<usize>,
    ) -> Result<(), CompileError> {
        let inits: Vec<(crate::chunk::StaticSlot, Spanned<Expr>)> = members
            .iter()
            .filter_map(|m| match &m.value {
                ClassMember::Field {
                    name,
                    default: Some(d),
                    ..
                } => self.chunk.classes[class as usize]
                    .sindex
                    .get(name.as_str())
                    // Only this class's *own* statics: an inherited entry
                    // names the parent's cell, which the parent's own
                    // initializer already filled.
                    .filter(|s| s.class == class)
                    .map(|s| (*s, d.clone())),
                _ => None,
            })
            .collect();

        for (s, d) in inits {
            let m = self.mark();
            let r = self.expr_tmp(&d)?;
            let a = self.reg8(r, span)?;
            self.emit(
                Instruction::abc(Op::SETSTAT, a, s.class as u8, s.slot as u8),
                span,
            );
            self.free_to(m);
        }
        Ok(())
    }

    /// `ClassName(args)`.
    pub fn construct_to(
        &mut self,
        class: ClassIdx,
        args: &[&Spanned<Expr>],
        dst: u16,
        span: &Range<usize>,
    ) -> Result<(), CompileError> {
        let m = self.mark();
        let inst = self.alloc(span)?;
        let ia = self.reg8(inst, span)?;
        self.emit(Instruction::abx(Op::NEW, ia, class as u16), span);

        if let Some(fi) = self.chunk.classes[class as usize].field_init {
            let fm = self.mark();
            let base = self.alloc(span)?;
            let a = self.reg8(base, span)?;
            self.emit(Instruction::abc(Op::MOVE, a, ia, 0), span);
            self.emit(Instruction::abc(Op::CALLK, a, 2, 1), span);
            let t = self.own_call_target(fi, span)?;
            self.emit(Instruction::ax_of(Op::EXTRAARG, t), span);
            self.free_to(fm);
        }

        if let Some(slot) = self.chunk.classes[class as usize].init {
            // Called on a *copy* of the instance: `CALLM` writes its result
            // over the receiver, and `init` returns nil.
            let cm = self.mark();
            let base = self.alloc_n(args.len() as u16 + 1, span)?;
            let a = self.reg8(base, span)?;
            self.emit(Instruction::abc(Op::MOVE, a, ia, 0), span);
            for (i, arg) in args.iter().enumerate() {
                self.expr_to(arg, base + 1 + i as u16)?;
            }
            self.emit(
                Instruction::abc(Op::CALLM, a, args.len() as u8 + 1, slot as u8),
                span,
            );
            self.free_to(cm);
        }

        self.move_result(inst, dst, span)?;
        self.free_to(m);
        Ok(())
    }

    /// The class a receiver expression statically denotes, if the front end
    /// proved one. `None` means "fall back", never "guess".
    pub fn class_of_expr(&self, e: &Spanned<Expr>) -> Option<ClassIdx> {
        if matches!(e.value, Expr::Self_) {
            return self.f.current_class;
        }
        self.ty_name(e.id).and_then(|n| self.layouts.get(n))
    }

    /// Refuse a class that does not satisfy an interface it declares.
    ///
    /// The tree-walker validates this when it *declares* the class
    /// (`eval/stmt/classes.rs`), and nothing earlier in the pipeline does —
    /// `tests/ui/implements_missing_method.sau` exists to record that as a
    /// typeck gap. So the compiler has to check too, or a program that
    /// should have been rejected compiles and runs with a hole in its
    /// itable.
    ///
    /// Interfaces *declared in this module* are checked in Pass 1, where the
    /// itable is built and a missing slot is visible directly. This pass
    /// covers the rest: the stdlib contracts (`Iterable`, `OpAdd`, …),
    /// looked up exactly the way the tree-walker looks them up — by name, in
    /// a prelude scope.
    pub fn check_interface_conformance(
        &self,
        module: &saule_ast::Module,
    ) -> Result<(), CompileError> {
        for s in &module.stmts {
            let saule_ast::Stmt::Decl(d) = &s.value else {
                continue;
            };
            let Decl::Class {
                name, implements, ..
            } = &d.value
            else {
                continue;
            };
            let Some(idx) = self.layouts.get(name) else {
                continue;
            };
            for iname in implements {
                // Declared here: Pass 1 already answered.
                if self.layouts.interface_of(iname).is_some() {
                    continue;
                }
                let Some(saule_interpreter::Value::Interface(iface)) = self.prelude_value(iname)
                else {
                    // An interface from another module, or none at all. The
                    // tree-walker errors on the second and this compiler
                    // cannot see the first, so neither is ours to compile.
                    return Err(CompileError::unsupported(
                        "a class implementing an interface this compiler cannot see",
                        d.span.clone(),
                    ));
                };
                let cls = &self.chunk.classes[idx as usize];
                // `vindex` and `smindex` are already flattened over the
                // parent chain, so a method satisfied by inheritance counts —
                // which is what the tree-walker's flattened `methods` map
                // does too.
                let satisfied = iface.methods.keys().all(|m| {
                    cls.vindex.contains_key(m.as_str()) || cls.smindex.contains_key(m.as_str())
                });
                if !satisfied {
                    return Err(CompileError::unsupported(
                        "a class that does not implement every method of its interface",
                        d.span.clone(),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Whether a bare name still means the class / enum / stdlib entity it
    /// looks like, rather than a value binding that shadows it.
    ///
    /// Two lookups, because a `local` lands in two different places
    /// depending on where it was written: inside a function it is a frame
    /// local (`FuncCtx::lookup`), at the top of the module it is a module
    /// slot (`shadowed_names`). Checking only the first read a class's
    /// static where `local Foo = {...}` meant its own table.
    pub fn not_shadowed(&self, name: &str) -> bool {
        self.f.lookup(name).is_none() && !self.shadowed_names.contains(name)
    }

    /// The class a receiver denotes, seeing through one layer of `?`.
    ///
    /// Separate from [`Self::class_of_expr`] on purpose. Stripping the `?`
    /// is only sound where the nil case is handled *outside* the slot-based
    /// access, which is exactly what `?.` does and what a plain `.` does
    /// not — so the two callers must not share one helper.
    pub fn class_of_nullable_expr(&self, e: &Spanned<Expr>) -> Option<ClassIdx> {
        if let Some(c) = self.class_of_expr(e) {
            return Some(c);
        }
        match self.types.get(&e.id)? {
            saule_ast::Type::Nullable(inner) => match inner.as_ref() {
                saule_ast::Type::Named(n) => self.layouts.get(n),
                _ => None,
            },
            _ => None,
        }
    }

    /// A class *named* by an expression — `Counter.total`, where the
    /// receiver is a type name rather than a value.
    pub fn class_named_by(&self, e: &Spanned<Expr>) -> Option<ClassIdx> {
        match &e.value {
            Expr::Ident(n) if self.not_shadowed(n) => self.layouts.get(n),
            _ => None,
        }
    }
}
