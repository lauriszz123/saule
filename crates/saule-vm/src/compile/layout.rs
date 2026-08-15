//! Pass 1: class layout (`VM_DESIGN.md` §8, §17).
//!
//! Builds a [`ClassProto`] per `class` declaration: which slot each instance
//! field occupies, which vtable slot each method occupies, and where the
//! statics live. Runs before any body is compiled, so a method can reference
//! a class declared further down the file.
//!
//! ## The prefix invariant
//!
//! **A subclass's field slots and vtable slots both extend its parent's
//! rather than reordering them.** That single property is what makes
//! `GETF r3 r1 4` — compiled against the static type `Player` — still
//! correct when `r1` turns out to hold a `Warrior`, and what makes a `CALLM`
//! on slot 7 reach the override. It is C++-style virtual dispatch, and it is
//! sound here for the same reason: inheritance is single and nominal
//! (`Decl::Class.extends` is an `Option<String>`).
//!
//! Classes are therefore laid out **parents first**, which is why this pass
//! orders them by inheritance depth instead of by source order.
//!
//! ## Single-sourcing (§24.2)
//!
//! §24.2 names the worst bug this project could ship: the compiler computing
//! one field layout and the runtime computing another, so `GETF` reads the
//! wrong field, silently. The mitigation is that there is only ever **one**
//! layout — the runtime `ClassObject` the VM builds shares the very same
//! `Rc<FieldLayout>` this pass produced, so divergence is not merely
//! unlikely, it is unrepresentable.

use std::collections::HashMap;
use std::rc::Rc;

use saule_ast::{ClassMember, Decl, Module, Stmt};
use saule_interpreter::value::FieldLayout;

use crate::chunk::{ClassIdx, ClassProto};
use crate::compile::CompileError;

/// What Pass 1 learned, consumed by codegen.
#[derive(Debug, Default)]
pub struct Layouts {
    /// Class name -> index into `Chunk::classes`.
    pub index: HashMap<String, ClassIdx>,
    /// Enum name -> index into `Chunk::enums`.
    pub enums: HashMap<String, u32>,
    /// Interface name -> index into `Chunk::interfaces`.
    pub interfaces: HashMap<String, u32>,
}

impl Layouts {
    pub fn get(&self, name: &str) -> Option<ClassIdx> {
        self.index.get(name).copied()
    }

    pub fn enum_of(&self, name: &str) -> Option<u32> {
        self.enums.get(name).copied()
    }

    pub fn interface_of(&self, name: &str) -> Option<u32> {
        self.interfaces.get(name).copied()
    }
}

/// Collect every `interface` and number its methods.
///
/// `extends` is flattened here: an interface's slot list includes everything
/// it inherits, so an implementing class needs one itable per interface
/// rather than one per level.
pub fn build_interfaces(module: &Module) -> (Vec<crate::chunk::InterfaceProto>, HashMap<String, u32>) {
    use crate::chunk::InterfaceProto;

    let mut decls: HashMap<&str, (&Vec<String>, Vec<&str>)> = HashMap::new();
    let mut order: Vec<&str> = Vec::new();
    for st in &module.stmts {
        if let Stmt::Decl(d) = &st.value
            && let Decl::Interface {
                name,
                extends,
                methods,
                ..
            } = &d.value
        {
            decls.insert(name, (extends, methods.iter().map(|m| m.name.as_str()).collect()));
            order.push(name);
        }
    }

    /// Method names of `name`, parents first, deduplicated.
    fn gather<'a>(
        name: &'a str,
        decls: &HashMap<&'a str, (&'a Vec<String>, Vec<&'a str>)>,
        seen: &mut Vec<&'a str>,
        out: &mut Vec<String>,
    ) {
        if seen.contains(&name) {
            return;
        }
        seen.push(name);
        let Some((extends, methods)) = decls.get(name) else {
            return;
        };
        for p in extends.iter() {
            gather(p, decls, seen, out);
        }
        for m in methods {
            if !out.iter().any(|o| o == m) {
                out.push((*m).to_string());
            }
        }
    }

    let mut protos = Vec::new();
    let mut index = HashMap::new();
    for name in order {
        let mut methods = Vec::new();
        gather(name, &decls, &mut Vec::new(), &mut methods);
        let names: Vec<Rc<str>> = methods.iter().map(|m| Rc::from(m.as_str())).collect();
        let idx = names
            .iter()
            .enumerate()
            .map(|(i, n)| (Rc::clone(n), i as u16))
            .collect();
        index.insert(name.to_string(), protos.len() as u32);
        protos.push(InterfaceProto {
            name: Rc::from(name),
            methods: names,
            index: idx,
        });
    }
    (protos, index)
}

/// Collect every `enum` and assign **dense tags in declaration order**.
///
/// Dense is the whole point (§9.1): it is what lets a `match` over an enum
/// compile to one indexed jump instead of a chain of string comparisons.
pub fn build_enums(
    module: &Module,
    chunk: &mut crate::chunk::Chunk,
    layouts: &mut Layouts,
) -> Result<(), CompileError> {
    use saule_ast::EnumVariant;
    use crate::chunk::{EnumProto, VariantProto};

    for st in &module.stmts {
        let Stmt::Decl(d) = &st.value else { continue };
        let Decl::Enum { name, variants, .. } = &d.value else {
            continue;
        };

        let mut protos = Vec::with_capacity(variants.len());
        let mut by_name = HashMap::new();
        for (tag, v) in variants.iter().enumerate() {
            let (vname, arity, value) = match &v.value {
                EnumVariant::Bare(n) => (n.clone(), 0u8, None),
                EnumVariant::Valued(n, expr) => {
                    let k = match crate::compile::literal_value(&expr.value) {
                        Some(val) => chunk.add_constant(val),
                        None => {
                            return Err(CompileError::unsupported(
                                "an enum variant whose value is not a literal",
                                v.span.clone(),
                            ));
                        }
                    };
                    (n.clone(), 0, Some(k))
                }
                EnumVariant::Tuple { name, fields } => {
                    (name.clone(), fields.len() as u8, None)
                }
            };
            by_name.insert(Rc::from(vname.as_str()), tag as u32);
            protos.push(VariantProto {
                name: Rc::from(vname.as_str()),
                arity,
                value,
            });
        }

        layouts
            .enums
            .insert(name.clone(), chunk.enums.len() as u32);
        chunk.enums.push(EnumProto {
            name: Rc::from(name.as_str()),
            variants: protos,
            by_name,
        });
    }
    Ok(())
}

/// Collect every `class` in `module` and build its proto.
///
/// Returns the protos in dependency order — a parent always precedes its
/// children — alongside the name index.
pub fn build(module: &Module) -> Result<(Vec<ClassProto>, Layouts), CompileError> {
    let (ifaces, iface_index) = build_interfaces(module);
    // Gather declarations first so a forward `extends` resolves.
    let mut implements: HashMap<&str, &Vec<String>> = HashMap::new();
    let mut decls: Vec<(&str, Option<&str>, &[saule_ast::Spanned<ClassMember>], std::ops::Range<usize>)> =
        Vec::new();
    for s in &module.stmts {
        if let Stmt::Decl(d) = &s.value
            && let Decl::Class {
                name,
                extends,
                members,
                implements: imps,
                ..
            } = &d.value
        {
            // `Assignable<T>` makes `local x: C = <a bare T>` build a `C` at
            // the *binding site*, from the declared type (`eval/coerce.rs`).
            // Nothing in this compiler does that yet, so a chunk would store
            // the bare value and the first method call on it would find a
            // `string` where an instance was promised. Refuse the module
            // rather than compile that.
            if imps.iter().any(|i| i == "Assignable") {
                return Err(CompileError::unsupported(
                    "a class implementing `Assignable`",
                    d.span.clone(),
                ));
            }
            // `OpToString`. `display_value` asks the *runtime* `ClassObject`
            // whether it has a `toString`, and a VM-built class has an empty
            // method map — so it answers no and falls back to
            // `<instance of Money>` **without an error**. Every `println`,
            // `tostring` and `..` on such an instance would be quietly
            // wrong, which is worse than any refusal.
            //
            // Refused per class rather than per use site because an instance
            // reaches a display through natives, tables and anything else
            // holding it; there is no small set of sites to guard. The
            // re-entrancy work removes this whole family — see VM_TASKS.md.
            if members.iter().any(|m| {
                matches!(&m.value, ClassMember::Method(me) if me.name == "toString" && !me.is_static)
            }) {
                return Err(CompileError::unsupported(
                    "a class with a `toString` overload",
                    d.span.clone(),
                ));
            }
            implements.insert(name, imps);
            decls.push((name, extends.as_deref(), members, d.span.clone()));
        }
    }

    let ordered = order_by_depth(&decls)?;

    let mut protos: Vec<ClassProto> = Vec::new();
    let mut index: HashMap<String, ClassIdx> = HashMap::new();

    for &i in &ordered {
        let (name, extends, members, span) = &decls[i];
        let parent_idx = match extends {
            Some(p) => match index.get(*p) {
                Some(idx) => Some(*idx),
                // A parent from another module. Cross-module layouts are the
                // import slice of Phase 3; refusing is correct until then,
                // because guessing a layout is exactly §24.2's failure.
                None => {
                    return Err(CompileError::unsupported(
                        "a class extending one from another module",
                        span.clone(),
                    ));
                }
            },
            None => None,
        };

        // Classes are appended in `ordered`, so this is the index this
        // class will have — which its own statics must be recorded against.
        let self_idx = protos.len() as ClassIdx;
        let mut proto = build_one(name, self_idx, parent_idx, members, &protos, span)?;
        // An itable per implemented interface: interface slot -> this
        // class's vtable slot. Built once here, so a `CALLIF` is a small-map
        // probe and an indexed load rather than a name lookup (§8.4).
        for iname in implements.get(*name).into_iter().flat_map(|v| v.iter()) {
            let Some(&ii) = iface_index.get(iname.as_str()) else {
                continue;
            };
            let slots: Option<Vec<u16>> = ifaces[ii as usize]
                .methods
                .iter()
                .map(|m| proto.vindex.get(m).copied())
                .collect();
            match slots {
                Some(s) => {
                    proto.itables.insert(ii, s);
                }
                // The class is missing a method its interface requires.
                // Nothing before this point rejects that — `saule-semantic`
                // does not, and the typechecker does not either
                // (`tests/ui/implements_missing_method.sau` records the gap)
                // — the *tree-walker* catches it, when it declares the
                // class. So refuse the module and let the oracle produce
                // the diagnostic, rather than compiling a class whose
                // itable silently has a hole in it.
                None => {
                    return Err(CompileError::unsupported(
                        "a class that does not implement every method of its interface",
                        span.clone(),
                    ));
                }
            }
        }
        let proto = proto;
        index.insert((*name).to_string(), protos.len() as ClassIdx);
        protos.push(proto);
    }

    Ok((
        protos,
        Layouts {
            index,
            enums: HashMap::new(),
            interfaces: iface_index,
        },
    ))
}

/// Order classes so a parent is always built before its children.
fn order_by_depth(
    decls: &[(&str, Option<&str>, &[saule_ast::Spanned<ClassMember>], std::ops::Range<usize>)],
) -> Result<Vec<usize>, CompileError> {
    let position: HashMap<&str, usize> = decls.iter().enumerate().map(|(i, d)| (d.0, i)).collect();
    let mut done = vec![false; decls.len()];
    let mut out = Vec::with_capacity(decls.len());

    for start in 0..decls.len() {
        // Walk up to the root, then emit downwards.
        let mut chain = Vec::new();
        let mut cur = Some(start);
        while let Some(i) = cur {
            if done[i] {
                break;
            }
            if chain.contains(&i) {
                // `saule-semantic` rejects inheritance cycles, so this only
                // fires on an unanalysed module — but it must terminate
                // rather than loop.
                return Err(CompileError::unsupported(
                    "a cyclic class hierarchy",
                    decls[i].3.clone(),
                ));
            }
            chain.push(i);
            cur = decls[i].1.and_then(|p| position.get(p).copied());
        }
        for i in chain.into_iter().rev() {
            if !done[i] {
                done[i] = true;
                out.push(i);
            }
        }
    }
    Ok(out)
}

fn build_one(
    name: &str,
    self_idx: ClassIdx,
    parent: Option<ClassIdx>,
    members: &[saule_ast::Spanned<ClassMember>],
    built: &[ClassProto],
    span: &std::ops::Range<usize>,
) -> Result<ClassProto, CompileError> {
    // The tree-walker promotes a defaulted field to a static when the class
    // has no `init` (`eval/stmt/classes.rs`). The compiler has to make the
    // same call, or the two engines disagree about what `C.field` means.
    let has_init = members.iter().any(|m| {
        matches!(&m.value, ClassMember::Method(me) if me.name == "init" && !me.is_static)
    });

    let mut own_fields: Vec<Rc<str>> = Vec::new();
    let mut statics: Vec<Rc<str>> = Vec::new();
    let mut instance_methods: Vec<(Rc<str>, usize)> = Vec::new();
    let mut static_methods: Vec<(Rc<str>, usize)> = Vec::new();

    for (i, m) in members.iter().enumerate() {
        match &m.value {
            ClassMember::Field {
                is_static,
                name: fname,
                default,
                ..
            } => {
                if *is_static || (!has_init && default.is_some()) {
                    statics.push(Rc::from(fname.as_str()));
                } else {
                    own_fields.push(Rc::from(fname.as_str()));
                }
            }
            ClassMember::Method(me) => {
                if me.is_static {
                    static_methods.push((Rc::from(me.name.as_str()), i));
                } else {
                    instance_methods.push((Rc::from(me.name.as_str()), i));
                }
            }
        }
    }

    // Fields: the parent's slots, then this class's own. A redeclared name
    // keeps the parent's slot so the prefix invariant survives shadowing.
    let parent_proto = parent.map(|p| &built[p as usize]);
    // Built through `FieldLayout::build`, the same call the tree-walker uses
    // when it constructs a class — so the two produce identical slots by
    // construction rather than by agreement (§24.2).
    let own_defs: Vec<saule_interpreter::value::FieldDef> = own_fields
        .iter()
        .map(|n| saule_interpreter::value::FieldDef {
            name: n.to_string(),
            default: None,
        })
        .collect();
    let layout = FieldLayout::build(parent_proto.map(|p| p.layout.as_ref()), &own_defs);
    if layout.len() > u8::MAX as usize {
        return Err(CompileError::unsupported(
            "a class with more than 255 fields",
            span.clone(),
        ));
    }

    // Vtable: a copy of the parent's, overrides written in place, new
    // methods appended. `usize::MAX` is a placeholder for "body not compiled
    // yet"; codegen fills it in.
    let mut vtable: Vec<u32> = parent_proto.map(|p| p.vtable.clone()).unwrap_or_default();
    let mut vindex: HashMap<Rc<str>, u16> = parent_proto
        .map(|p| p.vindex.clone())
        .unwrap_or_default();
    let mut member_of_slot: HashMap<u16, usize> = HashMap::new();
    for (mname, member_i) in instance_methods {
        match vindex.get(&mname) {
            Some(&slot) => {
                member_of_slot.insert(slot, member_i);
            }
            None => {
                let slot = vtable.len() as u16;
                vtable.push(u32::MAX);
                vindex.insert(Rc::clone(&mname), slot);
                member_of_slot.insert(slot, member_i);
            }
        }
    }
    if vtable.len() > u8::MAX as usize {
        return Err(CompileError::unsupported(
            "a class with more than 255 methods",
            span.clone(),
        ));
    }

    let mut smindex: HashMap<Rc<str>, u16> = HashMap::new();
    let mut static_slots: Vec<usize> = Vec::new();
    for (mname, member_i) in static_methods {
        smindex.insert(mname, static_slots.len() as u16);
        static_slots.push(member_i);
    }

    // Inherited entries keep pointing at the class that declared them, so
    // every reader and writer of an inherited static addresses the one cell
    // the parent initialized — the tree-walker's `declaring_static_field`
    // rule, expressed as data. Only this class's *own* statics claim slots
    // in this class's storage, so `n_statics` counts from zero rather than
    // continuing the parent's.
    let mut sindex: HashMap<Rc<str>, crate::chunk::StaticSlot> = parent_proto
        .map(|p| p.sindex.clone())
        .unwrap_or_default();
    let mut n_statics = 0u16;
    for s in statics {
        // A redeclared static shadows the parent's, which is what the
        // tree-walker does too: the subclass's own `static_fields` entry is
        // found before the chain walk reaches the parent.
        sindex.insert(
            s,
            crate::chunk::StaticSlot {
                class: self_idx,
                slot: n_statics,
            },
        );
        n_statics += 1;
    }

    let init = vindex.get("init").copied();

    Ok(ClassProto {
        name: Rc::from(name),
        parent,
        layout: Rc::new(layout),
        field_template: None,
        field_init: None,
        vtable,
        vindex,
        n_statics,
        sindex,
        statics_init: None,
        static_methods: static_slots.iter().map(|_| u32::MAX).collect(),
        smindex,
        itables: HashMap::new(),
        init,
        // Which member declaration fills each slot; codegen consumes this
        // and it is not part of the runtime shape.
        member_of_vslot: member_of_slot,
        member_of_sslot: static_slots,
    })
}
