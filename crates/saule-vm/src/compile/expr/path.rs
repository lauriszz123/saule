//! Chains of field reads, compiled to one `GETPATH`.
//!
//! `row.mid?.inner?.v` was eleven instructions: a `GETF` per link, and a
//! `JNOTNIL`/`JMP`/`JMP`/`LOADNIL` diamond around every `?.`. Each `GETF`
//! also copied the object in the middle of the chain into a register — a
//! reference-count increment on the way in and a decrement on the way out —
//! for an object the program only passes through. The compiler has already
//! proved the class at every link and resolved every name to a slot, so the
//! whole chain is data: a [`FieldPath`] in the proto, walked by the VM in one
//! instruction that copies only the value at the end.

use saule_ast::{Expr, Spanned};

use super::super::ctx::Compiler;
use super::CompileError;
use super::member::MemberAccess;
use crate::chunk::{FieldPath, LinkKind, PathLink};
use crate::op::{Instruction, Op};

impl Compiler<'_> {
    /// Compile `e` as one `GETPATH` if it is a chain of field reads worth
    /// one, and say whether it was.
    ///
    /// Worth one means two links or more, or a single `?.` — whose nil test
    /// is the other half of what the instruction saves. A lone `.name` or
    /// `.name!` is already one `GETF` or `GETFU`.
    pub(crate) fn field_path_to(
        &mut self,
        e: &Spanned<Expr>,
        dst: u16,
    ) -> Result<bool, CompileError> {
        let Some((root, links)) = self.field_path(e) else {
            return Ok(false);
        };
        if links.len() < 2 && links.iter().all(|l| l.kind != LinkKind::Safe) {
            return Ok(false);
        }
        let path = FieldPath {
            links: links.into_boxed_slice(),
        };
        let idx = match self.f.paths.iter().position(|p| *p == path) {
            Some(i) => i,
            None => {
                self.f.paths.push(path);
                self.f.paths.len() - 1
            }
        };
        let Ok(c) = u8::try_from(idx) else {
            // Past 256 distinct paths in one function: the rest compile a
            // link at a time, which is what they always did.
            self.f.paths.pop();
            return Ok(false);
        };

        let span = &e.span;
        // The root is read where it already is when that is safe — the
        // same rule `member_read` applies to its one receiver.
        let in_place = self.operand_is_pure(root);
        let m = self.mark();
        let r = self.operand_to_reg(root, in_place)?;
        let (a, b) = (self.reg8(dst, span)?, self.reg8(r, span)?);
        self.emit(Instruction::abc(Op::GETPATH, a, b, c), span);
        self.free_to(m);
        Ok(true)
    }

    /// The chain of field links `e` ends in, outermost last, and the
    /// expression it starts from. `None` when `e` is not a field link at all.
    fn field_path<'e>(&self, e: &'e Spanned<Expr>) -> Option<(&'e Spanned<Expr>, Vec<PathLink>)> {
        let mut links = Vec::new();
        let mut cur = e;
        loop {
            let (obj, slot, kind) = match &cur.value {
                Expr::ForceUnwrap(inner) => match &inner.value {
                    Expr::Member { obj, name } => match self.plain_field_slot(obj, name) {
                        Some(slot) => (obj, slot, LinkKind::Unwrap),
                        None => break,
                    },
                    _ => break,
                },
                Expr::Member { obj, name } => match self.plain_field_slot(obj, name) {
                    Some(slot) => (obj, slot, LinkKind::Plain),
                    None => break,
                },
                Expr::SafeMember { obj, name } => {
                    match self
                        .class_of_nullable_expr(obj)
                        .and_then(|class| self.member_access(class, name))
                    {
                        Some(MemberAccess::Field(slot)) => (obj, slot, LinkKind::Safe),
                        _ => break,
                    }
                }
                _ => break,
            };
            links.push(PathLink {
                slot,
                kind,
                span_start: cur.span.start as u32,
                span_end: cur.span.end as u32,
            });
            cur = obj;
        }
        if links.is_empty() {
            return None;
        }
        links.reverse();
        Some((cur, links))
    }

    /// The slot `obj.name` reads, when [`member_read`](Self::member_read)
    /// would compile it to a `GETF` — and `None` for every shape it sends
    /// somewhere else, tested in the order it tests them.
    fn plain_field_slot(&self, obj: &Spanned<Expr>, name: &str) -> Option<u16> {
        // `self.count` in a static fn: a static read.
        if matches!(obj.value, Expr::Self_)
            && !self.f.in_method
            && let Some(class) = self.f.current_class
            && self.chunk.classes[class as usize].sindex.contains_key(name)
        {
            return None;
        }
        if let Expr::Ident(n) = &obj.value {
            // `Status.Alive`: a variant reference.
            if self.not_shadowed(n)
                && let Some(e_idx) = self.layouts.enum_of(n)
                && self.chunk.enums[e_idx as usize].by_name.contains_key(name)
            {
                return None;
            }
            // `Counter.total`: a static named through its class.
            if let Some(class) = self.class_named_by(obj)
                && self.chunk.classes[class as usize].sindex.contains_key(name)
            {
                return None;
            }
            // `Math.pi`: a prelude member folded to a constant.
            if !self.mutated_receivers.contains(n) && self.prelude_member(obj.id, n, name).is_some()
            {
                return None;
            }
        }
        // `variant.value`, and `t.key` on a table, are not fields.
        if name == "value"
            && self
                .ty_name(obj.id)
                .is_some_and(|t| self.layouts.enum_of(t).is_some())
        {
            return None;
        }
        if matches!(self.types.get(&obj.id), Some(saule_ast::Type::Table { .. })) {
            return None;
        }
        match self.member_access(self.class_of_expr(obj)?, name)? {
            MemberAccess::Field(slot) => Some(slot),
            MemberAccess::Static(_) => None,
        }
    }
}
