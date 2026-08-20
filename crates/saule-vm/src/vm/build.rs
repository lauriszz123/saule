//! Turning a chunk's class and enum protos into runtime objects.
//!
//! This runs once per module, before its top level does. Everything it
//! builds is shared with the compiler's own view — the layout `Rc` in
//! particular — so the two cannot disagree about which slot holds what.

use std::cell::RefCell;
use std::rc::Rc;

use saule_interpreter::value::VmFunctionRef;
use saule_interpreter::Value;

use crate::chunk::Chunk;

use super::{Closure, VmShared};

/// Materialise one runtime `ClassObject` per `ClassProto`.
///
/// The layout `Rc` is **shared**, not rebuilt: the class the VM hands to an
/// instance and the layout the compiler resolved `GETF` against are the same
/// allocation, so they cannot disagree about which slot a field occupies
/// (§24.2).
///
/// The method maps are filled here too, from the chunk's flattened `vindex`
/// and `vtable`. They used to be left empty, on the reasoning that dispatch
/// goes through the vtable rather than the map — true for `CALLM`, and false
/// for everything that asks a *class* a question from outside the VM.
/// `display_value` asking "do you have a `toString`?" got `no` and printed
/// `<instance of Money>` with no error at all.
///
/// Protos are ordered parents-first by Pass 1, so a parent is always
/// available when its child is built.
#[allow(clippy::type_complexity)]
pub(crate) fn build_classes(
    chunks: &[Rc<Chunk>],
    weak: &std::rc::Weak<VmShared>,
) -> (
    Vec<Rc<saule_interpreter::value::ClassObject>>,
    std::collections::HashMap<usize, u32>,
    Vec<RefCell<Vec<Value>>>,
) {
    use saule_interpreter::value::{ClassObject, MethodRef};

    // Every chunk of a program shares one class table, so reading it through
    // the first is not a choice of module.
    let table = Rc::clone(&chunks[0].classes);
    let mut classes: Vec<Rc<ClassObject>> = Vec::with_capacity(table.len());
    let mut class_of = std::collections::HashMap::new();
    let mut statics = Vec::with_capacity(table.len());

    // A method proto captures nothing — it reaches module slots through
    // `GETMOD` and statics through `GETSTAT` — so an upvalue-free closure is
    // the whole of it, and one per method is enough.
    //
    // Loaded from the chunk of the module that **declared** the class: a
    // proto index only means something within its own chunk.
    let bind = |module: usize, target: u32| -> Option<Rc<VmFunctionRef>> {
        let chunk = chunks.get(module)?;
        (target != u32::MAX).then(|| {
            VmFunctionRef::new(Closure {
                proto: Rc::clone(chunk.proto(target)),
                chunk: Rc::clone(chunk),
                upvals: Vec::new(),
                shared: weak.clone(),
            })
        })
    };

    for proto in table.iter() {
        let parent = proto.parent.map(|p| Rc::clone(&classes[p as usize]));

        // `vindex` and `vtable` are both prefix-extensions of the parent's,
        // so this map is already flattened in the sense `lookup_method`
        // requires: one probe finds an inherited method too.
        let methods = proto
            .vindex
            .iter()
            .filter_map(|(name, &slot)| {
                let target = proto.vtable.get(slot as usize).copied()?;
                Some((name.to_string(), MethodRef::Vm(bind(proto.module, target)?)))
            })
            .collect();
        let static_methods = proto
            .smindex
            .iter()
            // `smindex` is flattened, so an entry may name a parent — and
            // both the proto vector and the module it belongs to are the
            // *declaring* class's, not this one's.
            .filter_map(|(name, &s)| {
                let owner = &table[s.class as usize];
                let target = owner.static_methods.get(s.slot as usize).copied()?;
                Some((name.to_string(), MethodRef::Vm(bind(owner.module, target)?)))
            })
            .collect();

        let class = Rc::new(ClassObject {
            name: proto.name.to_string(),
            parent,
            field_defs: Vec::new(),
            layout: Rc::clone(&proto.layout),
            methods,
            static_fields: RefCell::new(Default::default()),
            static_methods,
            constructor: None,
        });
        class_of.insert(Rc::as_ptr(&class) as usize, classes.len() as u32);
        classes.push(class);
        statics.push(RefCell::new(vec![Value::Nil; proto.n_statics as usize]));
    }

    (classes, class_of, statics)
}

/// Materialise one runtime enum per `EnumProto`.
///
/// Bare and valued variants are **singletons**, so `Status.Alive` is the
/// same `Rc` every time it is mentioned and identity comparison works
/// (§9.1). A tuple variant has no singleton: each call constructs a
/// fresh object carrying its own payload, stamped with the same tag.
pub(crate) fn build_enums(
    chunks: &[Rc<Chunk>],
    weak: &std::rc::Weak<VmShared>,
) -> Vec<Rc<saule_interpreter::value::EnumObject>> {
    use saule_interpreter::value::{EnumObject, EnumVariantObject, MethodRef};
    let table = Rc::clone(&chunks[0].enums);
    let mut enums = Vec::with_capacity(table.len());
    {
        for proto in table.iter() {
            // A variant's value indexes the *declaring* module's pool.
            let chunk = &chunks[proto.module];
            let mut variants = saule_interpreter::fxhash::FxHashMap::default();
            let mut by_tag = Vec::with_capacity(proto.variants.len());
            let mut tags = saule_interpreter::fxhash::FxHashMap::default();
            let mut tuple_variants = saule_interpreter::fxhash::FxHashMap::default();

            for (tag, v) in proto.variants.iter().enumerate() {
                tags.insert(v.name.to_string(), tag as u32);
                if v.arity > 0 {
                    tuple_variants.insert(v.name.to_string(), v.arity as usize);
                    by_tag.push(None);
                    continue;
                }
                let obj = Rc::new(EnumVariantObject {
                    enum_name: proto.name.to_string(),
                    variant_name: v.name.to_string(),
                    tag: tag as u32,
                    value: v.value.map(|k| chunk.constants[k as usize].clone()),
                    enum_obj: RefCell::new(None),
                });
                variants.insert(v.name.to_string(), Rc::clone(&obj));
                by_tag.push(Some(obj));
            }

            // An enum method captures nothing — module slots through
            // `GETMOD`, statics through `GETSTAT` — so an upvalue-free
            // closure over the *declaring* module's chunk is the whole of
            // it, exactly as a class method's is. A proto index only means
            // something within its own chunk, which is why this reads
            // `proto.module` rather than the running one.
            let methods = proto
                .methods
                .iter()
                .filter(|&(_, &target)| target != u32::MAX)
                .map(|(name, &target)| {
                    (
                        name.to_string(),
                        MethodRef::Vm(VmFunctionRef::new(Closure {
                            proto: Rc::clone(chunk.proto(target)),
                            chunk: Rc::clone(chunk),
                            upvals: Vec::new(),
                            shared: weak.clone(),
                        })),
                    )
                })
                .collect();

            let e = Rc::new(EnumObject {
                name: proto.name.to_string(),
                variants: variants.clone(),
                by_tag,
                tags,
                tuple_variants,
                methods,
            });
            for v in variants.values() {
                *v.enum_obj.borrow_mut() = Some(Rc::clone(&e));
            }
            enums.push(e);
        }
    }
    enums
}

