//! One compiled function body, and the per-function tables that describe it.

use std::cell::RefCell;
use std::rc::Rc;

use saule_runtime::value::ClassObject;

use crate::op::Instruction;

use super::{ClassIdx, ProtoIdx, TypeIdx};

/// A compiled function body: the direct replacement for `FunctionObject`'s
/// `FunctionBody::Block(Arc<[Spanned<Stmt>]>)`.
#[derive(Debug)]
pub struct Proto {
    pub name: Option<Rc<str>>,
    pub n_params: u8,
    pub is_variadic: bool,
    /// Frame size: the register high-water mark recorded by §18's allocator.
    pub max_regs: u8,
    pub code: Vec<Instruction>,
    pub upvals: Vec<UpvalDesc>,
    /// Nested closures, as indices into [`Chunk::protos`]. `CLOSURE Bx`
    /// indexes *this* vector, not the chunk's.
    pub protos: Vec<ProtoIdx>,
    /// `try`/`catch` ranges, sorted by `pc_start` so unwinding binary-searches.
    pub handlers: Vec<Handler>,
    /// `pc -> span`, sorted by `pc`. Out of band: it never touches the
    /// instruction stream, so it costs nothing until something fails (§12.3).
    pub lines: Vec<LineEntry>,
    /// Per-call-site inline caches (§8.5). Runtime scratch, not serialized.
    pub caches: RefCell<Vec<InlineCache>>,
    /// Field paths `GETPATH C` walks, deduplicated per proto.
    pub paths: Vec<FieldPath>,
    pub owner_class: Option<ClassIdx>,
    /// Per-arity entry points for callees with defaulted parameters (§19).
    /// `entries[n]` is the pc to start at when called with `n` arguments;
    /// empty means "always start at 0".
    pub entries: Vec<u32>,
    /// The module-relative source this proto was compiled from. Carried per
    /// proto so an error inside an imported module renders against the right
    /// file — the job `FunctionObject.source` does today.
    pub source: Option<Rc<miette::NamedSource<String>>>,
}

impl Proto {
    /// A proto with nothing in it but a name and a body.
    pub fn new(name: Option<&str>, n_params: u8, max_regs: u8, code: Vec<Instruction>) -> Proto {
        Proto {
            name: name.map(Rc::from),
            n_params,
            is_variadic: false,
            max_regs,
            code,
            upvals: Vec::new(),
            protos: Vec::new(),
            handlers: Vec::new(),
            lines: Vec::new(),
            caches: RefCell::new(Vec::new()),
            paths: Vec::new(),
            owner_class: None,
            entries: Vec::new(),
            source: None,
        }
    }

    /// Source span for a program counter, for diagnostics. Binary search
    /// over the line table; an empty table yields `0..0`, which renders as
    /// a spanless error rather than pointing at the wrong text.
    pub fn span_at(&self, pc: u32) -> std::ops::Range<usize> {
        if self.lines.is_empty() {
            return 0..0;
        }
        let i = match self.lines.binary_search_by_key(&pc, |e| e.pc) {
            Ok(i) => i,
            // `pc` sits between entries: the covering entry is the one before.
            Err(0) => return 0..0,
            Err(i) => i - 1,
        };
        let e = &self.lines[i];
        e.span_start as usize..e.span_end as usize
    }

    /// The pc to enter at for a call with `n_args` arguments (§19's entry
    /// stubs). Falls back to 0 when the proto has no defaulted parameters.
    pub fn entry_for(&self, n_args: u8) -> u32 {
        self.entries.get(n_args as usize).copied().unwrap_or(0)
    }

    /// Human-readable name for diagnostics and the disassembler.
    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or("<lambda>")
    }
}

/// Where a closure's upvalue comes from at the moment the closure is built.
#[derive(Debug, Clone)]
pub struct UpvalDesc {
    /// `true`: a register of the *parent* frame. `false`: an upvalue of the
    /// parent closure.
    pub from_parent_stack: bool,
    pub index: u8,
    /// Diagnostics only.
    pub name: Rc<str>,
}

/// A `try`/`catch` range (§12.1). The happy path costs zero instructions:
/// entering a `try` emits nothing, and only a `throw` consults this table.
#[derive(Debug, Clone)]
pub struct Handler {
    pub pc_start: u32,
    /// Exclusive.
    pub pc_end: u32,
    /// Catch-block entry.
    pub target: u32,
    /// Register the caught value lands in.
    pub err_reg: u8,
    /// Type the `catch` clause filters on.
    pub catch_ty: TypeIdx,
}

/// `pc -> source span`, sorted by `pc`.
#[derive(Debug, Clone, Copy)]
pub struct LineEntry {
    pub pc: u32,
    pub span_start: u32,
    pub span_end: u32,
}

/// A chain of field reads — `a.b.c`, `a?.b?.c`, `a.b!.c!` — walked by one
/// [`GETPATH`](crate::op::Op::GETPATH) instead of one instruction per link.
///
/// Worked out by the compiler, which has already proved every class on the
/// path and resolved every name to a slot; the VM only follows the slots.
///
/// **What it saves is not mostly the dispatch.** Each `GETF` of a link
/// copied the intermediate object into a register — a reference-count
/// increment on an object the program is only passing through, and a
/// decrement on whatever that register held before. Walking the path reads
/// each link in place and copies only the value at the end, which in a
/// path ending in a field like `.v` is a scalar and costs nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldPath {
    /// From the receiver outwards: `links[0]` reads a field of `R[B]`.
    pub links: Box<[PathLink]>,
}

/// One `.name`, `?.name` or `.name!` of a [`FieldPath`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathLink {
    /// The field's slot in the receiver's layout.
    pub slot: u16,
    pub kind: LinkKind,
    /// What a failure on this link blames: the member expression for `.`
    /// and `?.`, the `!` for an unwrapping link — the spans `GETF` and
    /// `GETFU` carried when each link was its own instruction.
    pub span_start: u32,
    pub span_end: u32,
}

impl PathLink {
    pub fn span(&self) -> std::ops::Range<usize> {
        self.span_start as usize..self.span_end as usize
    }
}

/// How a [`PathLink`] treats its receiver and its result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    /// `.name`: the receiver is an instance.
    Plain,
    /// `?.name`: a nil receiver yields nil, and the read is skipped.
    Safe,
    /// `.name!`: as [`Plain`](LinkKind::Plain), and a nil field raises
    /// `ForceUnwrapNil`.
    Unwrap,
}

/// One remembered resolution: receivers of `class` call `(module, target)`.
///
/// The design sketch stores a `*const ClassObject`. This stores the `Rc`
/// instead: a raw pointer that outlives its class can be matched by a *new*
/// class allocated at the same address, which would silently read the wrong
/// field. Holding the `Rc` makes that impossible, and the hit path still
/// only does a pointer compare — `Rc::ptr_eq` touches no refcount.
#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub class: Rc<ClassObject>,
    /// Module the resolved method was compiled in.
    pub module: u16,
    /// Proto index within that module.
    pub target: u32,
}

/// How many receiver classes one call site remembers before it gives up.
///
/// Four is the usual answer, and the suite agrees: the most a site in it
/// sees is three (`iface`'s `shapes[i]!.area()` over squares, rectangles
/// and rings). A site past this is a dispatcher over an open set of
/// classes, and probing a longer list on every call costs more than the
/// map lookup it would save.
pub const POLY_ENTRIES: usize = 4;

/// A call-site cache (§8.5): the receiver classes this `CALLM` or `CALLIF`
/// has seen, and what each resolved to.
///
/// No invalidation is needed: Saule has no metatables and no runtime class
/// mutation, so a `(class, slot)` pair is permanently valid once observed.
/// Caches the **resolved callee**, not the slot: the slot is already in the
/// instruction, and what the probe actually costs is turning it into a
/// `(module, proto)` pair — an `Rc::as_ptr`, a hash, a map probe and two
/// indexed loads. A hit replaces all of that with a pointer compare.
///
/// ## Why it is not only monomorphic any more
///
/// It was, and a site that saw two classes in turn missed on every call —
/// worse, it *rewrote* itself on every call, an `Rc` clone, a `borrow_mut`
/// and a drop of the class it evicted, on top of the full probe. `iface`
/// visits three classes in strict rotation, so its one hot site missed
/// 100% of the time and the cache was 20% of the program: more than no
/// cache would have cost. So a site now widens from one entry to
/// [`POLY_ENTRIES`], and past that it stops writing altogether.
///
/// `Mono` stays inline because nearly every site is one: its hit is the
/// pointer compare it always was, behind no extra indirection. Only a site
/// that has proved itself polymorphic pays for the `Box`, once.
#[derive(Debug, Clone, Default)]
pub enum InlineCache {
    #[default]
    Empty,
    /// One receiver class seen — almost every site, forever.
    Mono(CacheEntry),
    /// Two to [`POLY_ENTRIES`] classes, filled in the order first seen.
    Poly(Box<[Option<CacheEntry>; POLY_ENTRIES]>),
    /// More classes than that: every call takes the full probe, and nothing
    /// is written back, so there is nothing left to thrash.
    Mega,
}

impl InlineCache {
    /// What this site resolved receivers of `class` to, if it has seen one.
    #[inline(always)]
    pub fn probe(&self, class: &Rc<ClassObject>) -> Option<(u16, u32)> {
        match self {
            InlineCache::Mono(e) if Rc::ptr_eq(&e.class, class) => Some((e.module, e.target)),
            InlineCache::Poly(entries) => entries
                .iter()
                .map_while(Option::as_ref)
                .find(|e| Rc::ptr_eq(&e.class, class))
                .map(|e| (e.module, e.target)),
            _ => None,
        }
    }

    /// Whether a miss here is still worth remembering.
    #[inline]
    pub fn learns(&self) -> bool {
        !matches!(self, InlineCache::Mega)
    }

    /// Remember a miss: `class` resolves to `(module, target)` at this site.
    ///
    /// Only ever called after [`probe`](Self::probe) missed for `class`, so
    /// it never adds a class twice.
    #[cold]
    pub fn record(&mut self, class: Rc<ClassObject>, module: u16, target: u32) {
        let entry = CacheEntry {
            class,
            module,
            target,
        };
        *self = match std::mem::take(self) {
            InlineCache::Empty => InlineCache::Mono(entry),
            InlineCache::Mono(first) => {
                InlineCache::Poly(Box::new([Some(first), Some(entry), None, None]))
            }
            InlineCache::Poly(mut entries) => match entries.iter_mut().find(|e| e.is_none()) {
                Some(free) => {
                    *free = Some(entry);
                    InlineCache::Poly(entries)
                }
                // Full: this site dispatches over an open set of classes.
                // Dropping the entries frees the box and the classes it was
                // pinning; from here on nothing is written.
                None => InlineCache::Mega,
            },
            InlineCache::Mega => InlineCache::Mega,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class(name: &str) -> Rc<ClassObject> {
        Rc::new(ClassObject {
            name: name.to_string(),
            parent: None,
            layout: Rc::new(Default::default()),
            methods: Default::default(),
            static_fields: Default::default(),
            slot_statics: None,
            static_methods: Default::default(),
        })
    }

    #[test]
    fn a_site_widens_one_class_at_a_time_then_gives_up() {
        let classes: Vec<_> = (0..=POLY_ENTRIES)
            .map(|i| class(&format!("C{i}")))
            .collect();
        let mut site = InlineCache::Empty;
        for (i, c) in classes.iter().take(POLY_ENTRIES).enumerate() {
            assert_eq!(site.probe(c), None, "C{i} answered before it was recorded");
            site.record(Rc::clone(c), i as u16, 100 + i as u32);
            let expected = if i == 0 { "Mono" } else { "Poly" };
            assert!(
                format!("{site:?}").starts_with(expected),
                "after {} classes the site should be {expected}: {site:?}",
                i + 1
            );
            // Every class recorded so far still answers, with its own target
            // — widening must not evict or cross-wire an earlier entry.
            for (j, earlier) in classes.iter().take(i + 1).enumerate() {
                assert_eq!(site.probe(earlier), Some((j as u16, 100 + j as u32)));
            }
        }

        // One class too many: the site stops caching rather than evicting.
        let extra = &classes[POLY_ENTRIES];
        site.record(Rc::clone(extra), 9, 999);
        assert!(matches!(site, InlineCache::Mega), "{site:?}");
        assert!(!site.learns());
        for c in &classes {
            assert_eq!(site.probe(c), None, "a megamorphic site answers nothing");
        }
    }

    #[test]
    fn a_megamorphic_site_lets_go_of_the_classes_it_held() {
        // The cache pins classes alive with `Rc`s so a reused address cannot
        // be mistaken for a cached class. Going megamorphic drops the box,
        // and with it every class only the cache was still holding.
        let classes: Vec<_> = (0..=POLY_ENTRIES)
            .map(|i| class(&format!("C{i}")))
            .collect();
        let mut site = InlineCache::Empty;
        for (i, c) in classes.iter().enumerate() {
            site.record(Rc::clone(c), 0, i as u32);
        }
        assert!(matches!(site, InlineCache::Mega));
        for c in &classes {
            assert_eq!(
                Rc::strong_count(c),
                1,
                "the megamorphic cache still pins `{}`",
                c.name
            );
        }
    }
}
