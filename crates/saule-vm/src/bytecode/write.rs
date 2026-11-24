//! [`encode`]: a [`Program`] to bytes.
//!
//! Every struct is taken apart with an exhaustive `let` pattern, so a field
//! added to a chunk type is a compile error here until it is written — and
//! the matching read in `read.rs` follows from the same order.

use std::collections::HashMap;
use std::hash::Hash;
use std::rc::Rc;

use saule_ast::Type;
use saule_runtime::Value;

use crate::chunk::{
    Chunk, ClassProto, EnumProto, FieldPath, Handler, InterfaceProto, JumpTable, LineEntry,
    LinkKind, PathLink, Proto, StaticSlot, TypeDesc, TypeSlot, UpvalDesc, VariantProto,
};
use crate::program::Program;

use super::origin::{self, Origin};
use super::{FORMAT, MAGIC, Source};

/// A program that can't be written as bytecode.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EncodeError {
    /// A constant the compiler folded with no known path back to it — a
    /// value [`origin`] was never told about, or `origin::enable` was not
    /// called before the program was compiled.
    #[error("`{module}` uses {value}, which can't be written as bytecode (it has no known origin)")]
    UnknownConstant { module: String, value: String },
}

/// Write `program`. Compile it with [`origin::enable`] on, or any folded
/// standard-library constant fails with [`EncodeError::UnknownConstant`].
///
/// `label` renames each module for the file — the name errors and stack
/// traces show. The compiler's labels can be absolute paths on the machine
/// that built the program, which mean nothing where it runs.
pub fn encode(
    program: &Program,
    source: Source,
    label: &dyn Fn(&str) -> String,
) -> Result<Vec<u8>, EncodeError> {
    let mut w = Writer::default();
    w.bytes(MAGIC);
    w.u32_fixed(FORMAT);
    w.str(saule_version::FULL);
    w.u8(source.tag());

    // The tables every chunk shares; any module's `Rc` is the program's.
    let first = program.modules.first();
    let classes = first.map(|c| c.classes.as_slice()).unwrap_or(&[]);
    let enums = first.map(|c| c.enums.as_slice()).unwrap_or(&[]);
    let interfaces = first.map(|c| c.interfaces.as_slice()).unwrap_or(&[]);
    w.list(classes, Writer::class);
    w.list(enums, Writer::enum_);
    w.list(interfaces, Writer::interface);

    w.len(program.modules.len());
    for chunk in &program.modules {
        w.chunk(chunk, source, label)?;
    }

    let Program {
        modules: _,
        entry,
        via,
    } = program;
    w.usize(*entry);
    w.list(via, |w, span| {
        w.option(span.as_ref(), |w, s| {
            w.usize(s.start);
            w.usize(s.end);
        })
    });

    Ok(w.out)
}

#[derive(Default)]
pub(super) struct Writer {
    out: Vec<u8>,
}

impl Writer {
    // ─── Primitives ─────────────────────────────────────────────────────

    fn bytes(&mut self, b: &[u8]) {
        self.out.extend_from_slice(b);
    }

    fn u8(&mut self, v: u8) {
        self.out.push(v);
    }

    fn bool(&mut self, v: bool) {
        self.u8(v as u8);
    }

    fn u32_fixed(&mut self, v: u32) {
        self.bytes(&v.to_le_bytes());
    }

    /// LEB128.
    fn u64(&mut self, mut v: u64) {
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                self.u8(byte);
                return;
            }
            self.u8(byte | 0x80);
        }
    }

    fn u32(&mut self, v: u32) {
        self.u64(v as u64);
    }

    fn u16(&mut self, v: u16) {
        self.u64(v as u64);
    }

    fn usize(&mut self, v: usize) {
        self.u64(v as u64);
    }

    fn len(&mut self, n: usize) {
        self.usize(n);
    }

    fn str(&mut self, s: &str) {
        self.len(s.len());
        self.bytes(s.as_bytes());
    }

    fn option<T>(&mut self, v: Option<T>, f: impl FnOnce(&mut Self, T)) {
        match v {
            None => self.u8(0),
            Some(v) => {
                self.u8(1);
                f(self, v);
            }
        }
    }

    fn list<T>(&mut self, items: &[T], mut f: impl FnMut(&mut Self, &T)) {
        self.len(items.len());
        for item in items {
            f(self, item);
        }
    }

    fn try_list<T, E>(
        &mut self,
        items: &[T],
        mut f: impl FnMut(&mut Self, &T) -> Result<(), E>,
    ) -> Result<(), E> {
        self.len(items.len());
        for item in items {
            f(self, item)?;
        }
        Ok(())
    }

    /// Sorted by key, so the output doesn't depend on hash order.
    fn map<K: Ord + Hash, V>(
        &mut self,
        map: &HashMap<K, V>,
        mut key: impl FnMut(&mut Self, &K),
        mut value: impl FnMut(&mut Self, &V),
    ) {
        let mut entries: Vec<_> = map.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        self.len(entries.len());
        for (k, v) in entries {
            key(self, k);
            value(self, v);
        }
    }

    fn rc_str(&mut self, s: &Rc<str>) {
        self.str(s);
    }

    fn u32s(&mut self, v: &[u32]) {
        self.list(v, |w, x| w.u32(*x));
    }

    // ─── Shared tables ──────────────────────────────────────────────────

    fn class(&mut self, c: &ClassProto) {
        let ClassProto {
            name,
            module,
            parent,
            layout,
            field_template,
            field_init,
            vtable,
            vowner,
            vindex,
            assignable,
            n_statics,
            sindex,
            statics_init,
            static_methods,
            smindex,
            itables,
            init,
            member_of_vslot,
            member_of_sslot,
        } = c;
        self.rc_str(name);
        self.usize(*module);
        self.option(*parent, Self::u32);
        self.list(layout.names(), Self::rc_str);
        self.option(field_template.as_deref(), |w, t| w.u32s(t));
        self.option(*field_init, Self::u32);
        self.u32s(vtable);
        self.u32s(vowner);
        self.map(vindex, Self::rc_str, |w, v| w.u16(*v));
        self.bool(*assignable);
        self.u16(*n_statics);
        self.map(sindex, Self::rc_str, Self::static_slot);
        self.option(*statics_init, Self::u32);
        self.u32s(static_methods);
        self.map(smindex, Self::rc_str, Self::static_slot);
        self.map(
            itables,
            |w, k| w.u32(*k),
            |w, v| w.list(v, |w, x| w.u16(*x)),
        );
        self.option(*init, Self::u16);
        self.map(member_of_vslot, |w, k| w.u16(*k), |w, v| w.usize(*v));
        self.list(member_of_sslot, |w, v| w.usize(*v));
    }

    fn static_slot(&mut self, s: &StaticSlot) {
        let StaticSlot { class, slot } = s;
        self.u32(*class);
        self.u16(*slot);
    }

    fn enum_(&mut self, e: &EnumProto) {
        let EnumProto {
            name,
            module,
            variants,
            by_name,
            methods,
        } = e;
        self.rc_str(name);
        self.usize(*module);
        self.list(variants, |w, v| {
            let VariantProto { name, arity, value } = v;
            w.rc_str(name);
            w.u8(*arity);
            w.option(*value, Self::u32);
        });
        self.map(by_name, Self::rc_str, |w, v| w.u32(*v));
        self.map(methods, Self::rc_str, |w, v| w.u32(*v));
    }

    fn interface(&mut self, i: &InterfaceProto) {
        let InterfaceProto {
            name,
            methods,
            index,
        } = i;
        self.rc_str(name);
        self.list(methods, Self::rc_str);
        self.map(index, Self::rc_str, |w, v| w.u16(*v));
    }

    // ─── Modules ────────────────────────────────────────────────────────

    fn chunk(
        &mut self,
        c: &Chunk,
        source: Source,
        relabel: &dyn Fn(&str) -> String,
    ) -> Result<(), EncodeError> {
        let Chunk {
            protos,
            classes: _,
            enums: _,
            interfaces: _,
            constants,
            type_descs,
            cast_types,
            // Derived from `cast_types`; rebuilt on load.
            cast_fast: _,
            jump_tables,
            variant_refs,
            module_slots,
            module_slot_base,
            dynamic_imports,
            type_slots,
            module_index,
            main,
            source: text,
        } = c;

        let label = relabel(text.name());
        self.str(&label);
        let text = text.inner();
        match source {
            Source::Full => self.str(text),
            Source::Lines => {
                let lines: Vec<usize> = text.split('\n').map(str::len).collect();
                self.list(&lines, |w, n| w.usize(*n));
            }
        }

        self.list(protos, |w, p| w.proto(p));
        self.try_list(constants, |w, v| w.constant(v, &label))?;
        self.list(type_descs, Self::type_desc);
        self.list(cast_types, |w, t| w.ty(t));
        self.list(jump_tables, |w, t| {
            let JumpTable { targets, default } = t;
            w.u32s(targets);
            w.u32(*default);
        });
        self.list(variant_refs, |w, (e, tag)| {
            w.u32(*e);
            w.u32(*tag);
        });
        self.usize(*module_slots);
        self.usize(*module_slot_base);
        self.list(dynamic_imports, |w, (name, span)| {
            w.str(name);
            w.usize(span.start);
            w.usize(span.end);
        });
        self.list(type_slots, |w, (slot, ty)| {
            w.u16(*slot);
            match ty {
                TypeSlot::Class(i) => {
                    w.u8(0);
                    w.u32(*i);
                }
                TypeSlot::Enum(i) => {
                    w.u8(1);
                    w.u32(*i);
                }
            }
        });
        self.usize(*module_index);
        self.u32(*main);
        Ok(())
    }

    fn proto(&mut self, p: &Proto) {
        let Proto {
            name,
            n_params,
            is_variadic,
            max_regs,
            code,
            upvals,
            protos,
            handlers,
            lines,
            // Runtime scratch, empty on load.
            caches: _,
            paths,
            owner_class,
            entries,
            source,
        } = p;
        self.option(name.as_ref(), Self::rc_str);
        self.u8(*n_params);
        self.bool(*is_variadic);
        self.u8(*max_regs);
        self.list(code, |w, i| w.u32_fixed(i.0));
        self.list(upvals, |w, u| {
            let UpvalDesc {
                from_parent_stack,
                index,
                name,
            } = u;
            w.bool(*from_parent_stack);
            w.u8(*index);
            w.rc_str(name);
        });
        self.u32s(protos);
        self.list(handlers, |w, h| {
            let Handler {
                pc_start,
                pc_end,
                target,
                err_reg,
                catch_ty,
            } = h;
            w.u32(*pc_start);
            w.u32(*pc_end);
            w.u32(*target);
            w.u8(*err_reg);
            w.u32(*catch_ty);
        });
        self.list(lines, |w, l| {
            let LineEntry {
                pc,
                span_start,
                span_end,
            } = l;
            w.u32(*pc);
            w.u32(*span_start);
            w.u32(*span_end);
        });
        self.list(paths, |w, p| {
            let FieldPath { links } = p;
            w.list(links, |w, l| {
                let PathLink {
                    slot,
                    kind,
                    span_start,
                    span_end,
                } = l;
                w.u16(*slot);
                w.u8(match kind {
                    LinkKind::Plain => 0,
                    LinkKind::Safe => 1,
                    LinkKind::Unwrap => 2,
                });
                w.u32(*span_start);
                w.u32(*span_end);
            });
        });
        self.option(*owner_class, Self::u32);
        self.u32s(entries);
        // Always the chunk's own source; only whether there is one varies.
        self.bool(source.is_some());
    }

    fn constant(&mut self, v: &Value, module: &str) -> Result<(), EncodeError> {
        match v {
            Value::Nil => self.u8(0),
            Value::Bool(b) => {
                self.u8(1);
                self.bool(*b);
            }
            Value::Int(n) => {
                self.u8(2);
                self.bytes(&n.to_le_bytes());
            }
            Value::Float(f) => {
                self.u8(3);
                self.bytes(&f.to_bits().to_le_bytes());
            }
            Value::Str(s) => {
                self.u8(4);
                self.str(s.as_str());
            }
            other => {
                let Some(path) = origin::origin_of(other) else {
                    return Err(EncodeError::UnknownConstant {
                        module: module.to_string(),
                        value: origin::describe(other),
                    });
                };
                self.u8(5);
                self.origin(&path);
            }
        }
        Ok(())
    }

    fn origin(&mut self, o: &Origin) {
        match o {
            Origin::Prelude(name) => {
                self.u8(0);
                self.rc_str(name);
            }
            Origin::Package { package, name } => {
                self.u8(1);
                self.rc_str(package);
                self.rc_str(name);
            }
            Origin::Member { of, name } => {
                self.u8(2);
                self.origin(of);
                self.rc_str(name);
            }
            Origin::Variant { of, name } => {
                self.u8(3);
                self.origin(of);
                self.rc_str(name);
            }
            Origin::Builtin(b) => {
                self.u8(4);
                self.u8(b.tag());
            }
        }
    }

    fn type_desc(&mut self, t: &TypeDesc) {
        match t {
            TypeDesc::Any => self.u8(0),
            TypeDesc::Nil => self.u8(1),
            TypeDesc::Bool => self.u8(2),
            TypeDesc::Int => self.u8(3),
            TypeDesc::Float => self.u8(4),
            TypeDesc::Str => self.u8(5),
            TypeDesc::Table => self.u8(6),
            TypeDesc::Function => self.u8(7),
            TypeDesc::Class(i) => {
                self.u8(8);
                self.u32(*i);
            }
            TypeDesc::Enum(i) => {
                self.u8(9);
                self.u32(*i);
            }
            TypeDesc::Named(name) => {
                self.u8(10);
                self.rc_str(name);
            }
            TypeDesc::Nullable(i) => {
                self.u8(11);
                self.u32(*i);
            }
        }
    }

    fn ty(&mut self, t: &Type) {
        match t {
            Type::Named(name) => {
                self.u8(0);
                self.str(name);
            }
            Type::Nullable(inner) => {
                self.u8(1);
                self.ty(inner);
            }
            Type::Table { key, value } => {
                self.u8(2);
                self.option(key.as_deref(), Self::ty);
                self.ty(value);
            }
            Type::Tuple(items) => {
                self.u8(3);
                self.list(items, |w, t| w.ty(t));
            }
            Type::Function { params, ret } => {
                self.u8(4);
                self.list(params, |w, t| w.ty(t));
                self.ty(ret);
            }
            Type::Generic(g) => {
                self.u8(5);
                self.str(&g.name);
                self.list(&g.args, |w, t| w.ty(t));
            }
        }
    }
}
