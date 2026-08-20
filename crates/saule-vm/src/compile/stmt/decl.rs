//! Declarations: `fn`, `class`, `enum`, and exported module variables.
//!
//! Layout (pass 1) has already assigned every class its vtable and every
//! module variable its slot, so this is where those decisions become code:
//! a proto per function, a `CLASS` per class, a store per exported variable.

use saule_ast::Spanned;

use super::super::CompileError;
use super::super::ctx::Compiler;
use crate::op::{Instruction, Op};

impl Compiler<'_> {

    /// A declaration. Only `fn` for now.
    ///
    /// The proto index was reserved by the pre-pass in `compile_with`, so a
    /// forward call already resolves; what happens here is compiling the body
    /// into that reserved slot and binding the name.
    pub(crate) fn decl(&mut self, d: &Spanned<saule_ast::Decl>) -> Result<(), CompileError> {
        let span = &d.span;
        // Classes are compiled in their own pass, before the module body,
        // so the declaration statement itself emits nothing.
        // Classes and enums are laid out in their own passes before the
        // module body, so the declaration statement itself emits nothing.
        if matches!(
            &d.value,
            saule_ast::Decl::Class { .. }
                | saule_ast::Decl::Enum { .. }
                | saule_ast::Decl::Interface { .. }
        ) {
            return Ok(());
        }
        // An `import` a program driver already resolved emits nothing: the
        // names it binds are types, and a type is a compile-time index
        // (§14). Without a driver the name has a module slot nothing writes,
        // so compiling on would read `nil`.
        if matches!(&d.value, saule_ast::Decl::Import { .. }) {
            return if self.imports_bound {
                Ok(())
            } else {
                Err(CompileError::unsupported(
                    "an import declaration",
                    span.clone(),
                ))
            };
        }

        // `export name: T = value` — a module-level variable. The resolver
        // already gave it a module slot (`collect_module_scope` pushes
        // `Decl::Variable` alongside `Stmt::Local`), so this is the same
        // store a module-top `local` compiles to.
        //
        // **No `coerce_to_declared` here, deliberately.** `exec_decl`'s
        // `Decl::Variable` arm evaluates the initializer and defines the
        // name — it never calls `coerce::to_declared`, which the `Stmt::Local`
        // arm right above it does. Coercing here would make
        // `export x: Str = "…"` build a `Str` under the VM and leave a
        // string under the tree-walker: a silent divergence, and the oracle
        // is the tree-walker.
        if let saule_ast::Decl::Variable { name, value, .. } = &d.value {
            if !self.at_module_top() {
                return Err(CompileError::unsupported(
                    "an `export` variable outside the module body",
                    span.clone(),
                ));
            }
            let Some(slot) = self
                .bindings
                .module_slots
                .iter()
                .position(|n| n.as_ref() == name.as_str())
            else {
                return Err(CompileError::unsupported(
                    "a top-level binding the resolver did not record",
                    span.clone(),
                ));
            };
            let m = self.mark();
            let r = match value {
                Some(v) => self.expr_tmp(v)?,
                None => {
                    let r = self.alloc(span)?;
                    let a = self.reg8(r, span)?;
                    self.emit(Instruction::abc(Op::LOADNIL, a, 0, 0), span);
                    r
                }
            };
            let a = self.reg8(r, span)?;
            let g = self.mod_slot(slot as u16, span)?;
            self.emit(Instruction::abx(Op::SETMOD, a, g), span);
            self.free_to(m);
            return Ok(());
        }

        let saule_ast::Decl::Function {
            name, params, body, ..
        } = &d.value
        else {
            return Err(CompileError::unsupported(
                "a declaration the compiler does not handle",
                span.clone(),
            ));
        };

        let Some(&idx) = self.fn_protos.get(name.as_str()) else {
            return Err(CompileError::unsupported(
                "a nested function declaration",
                span.clone(),
            ));
        };

        // Compile-time argument binding is §19's own slice of work; until it
        // lands, refuse the shapes that need it rather than mis-bind them.
        if params.len() > u8::MAX as usize {
            return Err(CompileError::unsupported("a function with over 255 parameters", span.clone()));
        }

        self.push_function(Some(name));
        // Parameters occupy registers `0..n`: the calling convention leaves
        // them there, because the callee's frame *is* the argument window
        // (§6.2). Nothing is copied on entry.
        let outcome = (|| -> Result<(), CompileError> {
            let n = params.len() as u16;
            let label = self.func_label();
            self.f
                .regs
                .reserve_params(n)
                .map_err(|o| o.at(&label, span.clone()))?;
            self.f.n_params = params.len() as u8;
            for (i, p) in params.iter().enumerate() {
                self.f.declare(&p.name, i as u16);
            }
            self.f.entries = self.param_entries(params, 0, span)?;
            self.coerce_params(params, 0, span)?;
            for st in body {
                self.stmt(st)?;
            }
            Ok(())
        })();

        // Pop unconditionally: leaving the compiler inside a half-built
        // function after an error would corrupt every later diagnostic.
        let proto = self.pop_function(span);
        outcome?;
        self.chunk.protos[idx as usize] = std::rc::Rc::new(proto);

        // Bind the name too, so the function is a first-class value and not
        // only a `CALLK` target.
        if let Some(slot) = self
            .bindings
            .module_slots
            .iter()
            .position(|n| n.as_ref() == name.as_str())
        {
            let m = self.mark();
            let r = self.alloc(span)?;
            let a = self.reg8(r, span)?;
            let nested = self.f.nested_index(idx);
            self.emit(Instruction::abx(Op::CLOSURE, a, nested), span);
            let g = self.mod_slot(slot as u16, span)?;
            self.emit(Instruction::abx(Op::SETMOD, a, g), span);
            self.free_to(m);
        }

        Ok(())
    }

}
