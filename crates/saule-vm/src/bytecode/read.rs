//! [`decode`]: bytes back to a [`Program`], in the order `write.rs` wrote
//! them.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::Hash;
use std::rc::Rc;

use saule_ast::{GenericType, Type};
use saule_runtime::Value;
use saule_runtime::value::{FieldLayout, SauleStr};

use crate::chunk::{
    CastFast, Chunk, ClassProto, EnumProto, FieldPath, Handler, InterfaceProto, JumpTable,
    LineEntry, LinkKind, PathLink, Proto, StaticSlot, TypeDesc, TypeSlot, UpvalDesc, VariantProto,
};
use crate::op::Instruction;
use crate::program::Program;

use super::origin::{Builtin, Origin, World};
use super::{FORMAT, MAGIC, Source};

/// Read a program written by [`encode`](super::encode).
///
/// Folded constants are looked up again here, so the native packages the
/// program imports must be where `SAULE_HOME` says, and the project context
/// (`saule_project::set`) should be in place first: the prelude built here
/// is the one the program's `Project` reads.
pub fn decode(bytes: &[u8]) -> Result<Program, String> {
    let mut r = Reader { bytes, at: 0 };

    if r.take(MAGIC.len())? != MAGIC {
        return Err("not a Saule bytecode file".into());
    }
    let format = r.u32_fixed()?;
    let version = r.str()?;
    if format != FORMAT || version != saule_version::FULL {
        return Err(format!(
            "compiled by Saule {version} (bytecode format {format}); this is Saule {} \
             (format {FORMAT}) — rebuild it with this version",
            saule_version::FULL
        ));
    }
    let source = Source::from_tag(r.u8()?).ok_or("unknown source kind")?;

    let classes = Rc::new(r.list(Reader::class)?);
    let enums = Rc::new(r.list(Reader::enum_)?);
    let interfaces = Rc::new(r.list(Reader::interface)?);

    let mut world = World::new();
    let n = r.len()?;
    let mut modules = Vec::with_capacity(n);
    for _ in 0..n {
        let mut chunk = r.chunk(source, &mut world)?;
        chunk.classes = Rc::clone(&classes);
        chunk.enums = Rc::clone(&enums);
        chunk.interfaces = Rc::clone(&interfaces);
        crate::compile::verify::verify(&chunk)
            .map_err(|e| format!("`{}` is malformed: {e}", chunk.source.name()))?;
        modules.push(Rc::new(chunk));
    }

    let entry = r.usize()?;
    let via = r.list(|r| {
        r.option(|r| {
            let start = r.usize()?;
            Ok(start..r.usize()?)
        })
    })?;

    if r.at != bytes.len() {
        return Err("trailing bytes after the program".into());
    }
    if entry >= modules.len() || via.len() != modules.len() {
        return Err("the module table is inconsistent".into());
    }

    Ok(Program {
        modules,
        entry,
        via,
    })
}

type R<T> = Result<T, String>;

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    // ─── Primitives ─────────────────────────────────────────────────────

    fn take(&mut self, n: usize) -> R<&'a [u8]> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.bytes.len());
        let Some(end) = end else {
            return Err("the bytecode ends early".into());
        };
        let out = &self.bytes[self.at..end];
        self.at = end;
        Ok(out)
    }

    fn u8(&mut self) -> R<u8> {
        Ok(self.take(1)?[0])
    }

    fn bool(&mut self) -> R<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            b => Err(format!("{b} is not a boolean")),
        }
    }

    fn fixed<const N: usize>(&mut self) -> R<[u8; N]> {
        Ok(self.take(N)?.try_into().expect("took N bytes"))
    }

    fn u32_fixed(&mut self) -> R<u32> {
        Ok(u32::from_le_bytes(self.fixed()?))
    }

    fn u64(&mut self) -> R<u64> {
        let mut out = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = self.u8()?;
            out |= ((byte & 0x7f) as u64) << shift;
            if byte & 0x80 == 0 {
                return Ok(out);
            }
        }
        Err("a number in the bytecode is too long".into())
    }

    fn u32(&mut self) -> R<u32> {
        u32::try_from(self.u64()?).map_err(|_| "a number in the bytecode is too large".into())
    }

    fn u16(&mut self) -> R<u16> {
        u16::try_from(self.u64()?).map_err(|_| "a number in the bytecode is too large".into())
    }

    fn usize(&mut self) -> R<usize> {
        usize::try_from(self.u64()?).map_err(|_| "a number in the bytecode is too large".into())
    }

    /// A count, sanity-checked against what is left: every item takes at
    /// least one byte, so a larger count is corruption, not a reason to
    /// reserve gigabytes.
    fn len(&mut self) -> R<usize> {
        let n = self.usize()?;
        if n > self.bytes.len() - self.at {
            return Err("a list in the bytecode is longer than the file".into());
        }
        Ok(n)
    }

    fn str(&mut self) -> R<String> {
        let n = self.len()?;
        String::from_utf8(self.take(n)?.to_vec()).map_err(|_| "a string is not UTF-8".into())
    }

    fn rc_str(&mut self) -> R<Rc<str>> {
        Ok(Rc::from(self.str()?))
    }

    fn option<T>(&mut self, f: impl FnOnce(&mut Self) -> R<T>) -> R<Option<T>> {
        match self.u8()? {
            0 => Ok(None),
            1 => f(self).map(Some),
            b => Err(format!("{b} is not an option tag")),
        }
    }

    fn list<T>(&mut self, mut f: impl FnMut(&mut Self) -> R<T>) -> R<Vec<T>> {
        let n = self.len()?;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            out.push(f(self)?);
        }
        Ok(out)
    }

    fn map<K: Eq + Hash, V>(
        &mut self,
        mut key: impl FnMut(&mut Self) -> R<K>,
        mut value: impl FnMut(&mut Self) -> R<V>,
    ) -> R<HashMap<K, V>> {
        let n = self.len()?;
        let mut out = HashMap::with_capacity(n);
        for _ in 0..n {
            let k = key(self)?;
            out.insert(k, value(self)?);
        }
        Ok(out)
    }

    fn u32s(&mut self) -> R<Vec<u32>> {
        self.list(Self::u32)
    }

    // ─── Shared tables ──────────────────────────────────────────────────

    fn class(&mut self) -> R<ClassProto> {
        let name = self.rc_str()?;
        let module = self.usize()?;
        let parent = self.option(Self::u32)?;
        let names = self.list(Self::rc_str)?;
        let index = names
            .iter()
            .enumerate()
            .map(|(slot, name)| (Rc::clone(name), slot as u16))
            .collect();
        let layout = Rc::new(FieldLayout::from_parts(index, names));
        Ok(ClassProto {
            name,
            module,
            parent,
            layout,
            field_template: self.option(Self::u32s)?,
            field_init: self.option(Self::u32)?,
            vtable: self.u32s()?,
            vowner: self.u32s()?,
            vindex: self.map(Self::rc_str, Self::u16)?,
            assignable: self.bool()?,
            n_statics: self.u16()?,
            sindex: self.map(Self::rc_str, Self::static_slot)?,
            statics_init: self.option(Self::u32)?,
            static_methods: self.u32s()?,
            smindex: self.map(Self::rc_str, Self::static_slot)?,
            itables: self.map(Self::u32, |r| r.list(Self::u16))?,
            init: self.option(Self::u16)?,
            member_of_vslot: self.map(Self::u16, Self::usize)?,
            member_of_sslot: self.list(Self::usize)?,
        })
    }

    fn static_slot(&mut self) -> R<StaticSlot> {
        Ok(StaticSlot {
            class: self.u32()?,
            slot: self.u16()?,
        })
    }

    fn enum_(&mut self) -> R<EnumProto> {
        Ok(EnumProto {
            name: self.rc_str()?,
            module: self.usize()?,
            variants: self.list(|r| {
                Ok(VariantProto {
                    name: r.rc_str()?,
                    arity: r.u8()?,
                    value: r.option(Self::u32)?,
                })
            })?,
            by_name: self.map(Self::rc_str, Self::u32)?,
            methods: self.map(Self::rc_str, Self::u32)?,
        })
    }

    fn interface(&mut self) -> R<InterfaceProto> {
        Ok(InterfaceProto {
            name: self.rc_str()?,
            methods: self.list(Self::rc_str)?,
            index: self.map(Self::rc_str, Self::u16)?,
        })
    }

    // ─── Modules ────────────────────────────────────────────────────────

    fn chunk(&mut self, source: Source, world: &mut World) -> R<Chunk> {
        let label = self.str()?;
        let text = match source {
            Source::Full => self.str()?,
            // Blanks the shape of the original: the same lines, each as
            // long as it was, so every span still lands on its line and
            // column.
            Source::Lines => self
                .list(Self::usize)?
                .into_iter()
                .map(|n| " ".repeat(n))
                .collect::<Vec<_>>()
                .join("\n"),
        };
        let text = Rc::new(miette::NamedSource::new(label.clone(), text));

        let mut chunk = Chunk::empty(&label);
        chunk.source = Rc::clone(&text);
        chunk.protos = self.list(|r| r.proto(&text).map(Rc::new))?;
        chunk.constants = self.list(|r| r.constant(world, &label))?;
        chunk.type_descs = self.list(Self::type_desc)?;
        for ty in self.list(Self::ty)? {
            chunk.cast_fast.push(CastFast::of(&ty));
            chunk.cast_types.push(Rc::new(ty));
        }
        chunk.jump_tables = self.list(|r| {
            Ok(JumpTable {
                targets: r.u32s()?,
                default: r.u32()?,
            })
        })?;
        chunk.variant_refs = self.list(|r| Ok((r.u32()?, r.u32()?)))?;
        chunk.module_slots = self.usize()?;
        chunk.module_slot_base = self.usize()?;
        chunk.dynamic_imports = self.list(|r| {
            let name = r.str()?;
            let start = r.usize()?;
            Ok((name, start..r.usize()?))
        })?;
        chunk.type_slots = self.list(|r| {
            let slot = r.u16()?;
            let ty = match r.u8()? {
                0 => TypeSlot::Class(r.u32()?),
                1 => TypeSlot::Enum(r.u32()?),
                b => return Err(format!("{b} is not a type slot")),
            };
            Ok((slot, ty))
        })?;
        chunk.module_index = self.usize()?;
        chunk.main = self.u32()?;
        Ok(chunk)
    }

    fn proto(&mut self, source: &Rc<miette::NamedSource<String>>) -> R<Proto> {
        Ok(Proto {
            name: self.option(Self::rc_str)?,
            n_params: self.u8()?,
            is_variadic: self.bool()?,
            max_regs: self.u8()?,
            code: self.list(|r| r.u32_fixed().map(Instruction))?,
            upvals: self.list(|r| {
                Ok(UpvalDesc {
                    from_parent_stack: r.bool()?,
                    index: r.u8()?,
                    name: r.rc_str()?,
                })
            })?,
            protos: self.u32s()?,
            handlers: self.list(|r| {
                Ok(Handler {
                    pc_start: r.u32()?,
                    pc_end: r.u32()?,
                    target: r.u32()?,
                    err_reg: r.u8()?,
                    catch_ty: r.u32()?,
                })
            })?,
            lines: self.list(|r| {
                Ok(LineEntry {
                    pc: r.u32()?,
                    span_start: r.u32()?,
                    span_end: r.u32()?,
                })
            })?,
            caches: RefCell::new(Vec::new()),
            paths: self.list(|r| {
                let links = r.list(|r| {
                    Ok(PathLink {
                        slot: r.u16()?,
                        kind: match r.u8()? {
                            0 => LinkKind::Plain,
                            1 => LinkKind::Safe,
                            2 => LinkKind::Unwrap,
                            b => return Err(format!("{b} is not a field link")),
                        },
                        span_start: r.u32()?,
                        span_end: r.u32()?,
                    })
                })?;
                Ok(FieldPath {
                    links: links.into_boxed_slice(),
                })
            })?,
            owner_class: self.option(Self::u32)?,
            entries: self.u32s()?,
            source: self.bool()?.then(|| Rc::clone(source)),
        })
    }

    fn constant(&mut self, world: &mut World, module: &str) -> R<Value> {
        Ok(match self.u8()? {
            0 => Value::Nil,
            1 => Value::Bool(self.bool()?),
            2 => Value::Int(i64::from_le_bytes(self.fixed()?)),
            3 => Value::Float(f64::from_bits(u64::from_le_bytes(self.fixed()?))),
            4 => Value::Str(SauleStr::new(self.str()?)),
            5 => {
                let origin = self.origin()?;
                world
                    .resolve(&origin)
                    .map_err(|e| format!("`{module}` can't be loaded: {e}"))?
            }
            b => return Err(format!("{b} is not a constant tag")),
        })
    }

    fn origin(&mut self) -> R<Origin> {
        Ok(match self.u8()? {
            0 => Origin::Prelude(self.rc_str()?),
            1 => Origin::Package {
                package: self.rc_str()?,
                name: self.rc_str()?,
            },
            2 => Origin::Member {
                of: Box::new(self.origin()?),
                name: self.rc_str()?,
            },
            3 => Origin::Variant {
                of: Box::new(self.origin()?),
                name: self.rc_str()?,
            },
            4 => {
                let tag = self.u8()?;
                Origin::Builtin(Builtin::from_tag(tag).ok_or("unknown builtin")?)
            }
            b => return Err(format!("{b} is not an origin tag")),
        })
    }

    fn type_desc(&mut self) -> R<TypeDesc> {
        Ok(match self.u8()? {
            0 => TypeDesc::Any,
            1 => TypeDesc::Nil,
            2 => TypeDesc::Bool,
            3 => TypeDesc::Int,
            4 => TypeDesc::Float,
            5 => TypeDesc::Str,
            6 => TypeDesc::Table,
            7 => TypeDesc::Function,
            8 => TypeDesc::Class(self.u32()?),
            9 => TypeDesc::Enum(self.u32()?),
            10 => TypeDesc::Named(self.rc_str()?),
            11 => TypeDesc::Nullable(self.u32()?),
            b => return Err(format!("{b} is not a type test")),
        })
    }

    fn ty(&mut self) -> R<Type> {
        Ok(match self.u8()? {
            0 => Type::Named(self.str()?),
            1 => Type::Nullable(Box::new(self.ty()?)),
            2 => Type::Table {
                key: self.option(|r| r.ty().map(Box::new))?,
                value: Box::new(self.ty()?),
            },
            3 => Type::Tuple(self.list(Self::ty)?),
            4 => Type::Function {
                params: self.list(Self::ty)?,
                ret: Box::new(self.ty()?),
            },
            5 => Type::Generic(Box::new(GenericType {
                name: self.str()?,
                args: self.list(Self::ty)?,
            })),
            b => return Err(format!("{b} is not a type")),
        })
    }
}
