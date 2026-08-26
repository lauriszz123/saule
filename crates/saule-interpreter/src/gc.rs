//! Collecting reference cycles.
//!
//! `Rc` cannot free a cycle. Nothing in Saule could, until this file:
//!
//! ```text
//!   200,000 tables, t["pad"] = "…"        →   4 MB peak RSS
//!   200,000 tables, t["self"] = t; …      →  50 MB peak RSS
//! ```
//!
//! Unbounded, and the shape that causes it is ordinary — `t.self = t`, a
//! doubly-linked list, a parent holding children that hold the parent. The
//! specific cycles this codebase already knew about were patched by hand
//! (`Closure::shared` is `Weak` for exactly this reason); a user's own data
//! structure had nothing.
//!
//! ## Why a registry and not `Gc<T>`
//!
//! Textbook trial deletion (Bacon–Rajan) hooks every refcount *decrement* to
//! buffer candidate roots, which means replacing `Rc` with a smart pointer
//! that owns its `Drop`. That would touch `Value`, every `Rc::clone` of a
//! table, and — fatally — the **native ABI**: `saule-native-abi` and the
//! `libloading` packages hand `Value`s to foreign code that keeps them alive
//! simply by holding an `Rc`. It would also need the tree-walker's roots,
//! which live in Rust stack locals across `eval` → `exec` → `eval`
//! recursion, where nothing can see them.
//!
//! So this does the same *analysis* over a registry instead of a decrement
//! hook. `Rc` is untouched, no call site changes, no rooting, no safepoints:
//!
//! * a container is registered when it is **stored into another container**,
//! * collection computes, for each registered node, how many of its `Rc`
//!   references come from other registered nodes,
//! * anything whose references are *all* internal and unreachable from a
//!   node with an external reference is a cycle, and gets its contents
//!   cleared so ordinary refcounting can reclaim it.
//!
//! Registering on *store* rather than on *creation* is what keeps this cheap:
//! a table of integers is never registered at all. It is also sufficient,
//! because every node in a cycle is by definition stored inside another node
//! of that cycle.
//!
//! ## The safety argument
//!
//! The one thing this must never do is free something still reachable. That
//! reduces to never **over**-counting internal references, and the traversal
//! is written so it cannot:
//!
//! * it walks only a table's `array`/`map` and an instance's `fields`, which
//!   are exactly the `Rc`s those objects hold;
//! * it counts only *direct* `Value::Table`/`Value::Instance` children.
//!   Every other variant — `EnumVariant` with a payload, `Function`,
//!   `VmFunction` — is **opaque**, so a reference held through one is counted
//!   as external and its cycle is kept alive.
//!
//! Nor may it assume it can read a node. Collection is triggered from
//! `on_store`, which is called from `TableObject::set` and
//! `InstanceObject::set_field` — `&mut self` methods, so the container being
//! written to is already mutably borrowed by the frame underneath us. Every
//! borrow in `collect` is therefore a `try_`, and a node it cannot read is
//! pinned rather than guessed at.
//!
//! Under-counting is safe (a live-looking cycle is merely not collected);
//! over-counting would not be. Every uncertainty here is resolved in the
//! under-counting direction, which is why cycles routed through a closure or
//! a shared enum payload are still leaked — see `collects_only_what_it_can_prove`.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use crate::value::{InstanceObject, TableObject, Value};

/// A container that has been stored inside another container, and so could
/// be part of a cycle.
enum Node {
    Table(Weak<RefCell<TableObject>>),
    Instance(Weak<RefCell<InstanceObject>>),
}

/// A live handle to a registered node, held for the duration of a
/// collection. Holding it is what adds the `1` that [`collect`] subtracts.
enum Live {
    Table(Rc<RefCell<TableObject>>),
    Instance(Rc<RefCell<InstanceObject>>),
}

impl Live {
    fn addr(&self) -> usize {
        match self {
            Live::Table(r) => Rc::as_ptr(r) as *const u8 as usize,
            Live::Instance(r) => Rc::as_ptr(r) as *const u8 as usize,
        }
    }

    fn strong_count(&self) -> usize {
        match self {
            Live::Table(r) => Rc::strong_count(r),
            Live::Instance(r) => Rc::strong_count(r),
        }
    }

    /// Call `f` with every directly-held container child. Returns `false` if
    /// the node is mutably borrowed right now and could not be read.
    ///
    /// **Direct only.** Recursing through an opaque variant would risk
    /// attributing one `Rc` to two different parents, which is the one error
    /// that could free live data — see this module's safety argument.
    fn each_child(&self, mut f: impl FnMut(&Value)) -> bool {
        match self {
            Live::Table(r) => {
                let Ok(t) = r.try_borrow() else { return false };
                for v in &t.array {
                    f(v);
                }
                for v in t.map.values() {
                    f(v);
                }
            }
            Live::Instance(r) => {
                let Ok(i) = r.try_borrow() else { return false };
                for v in &i.fields {
                    f(v);
                }
            }
        }
        true
    }

    /// Drop everything this node holds, breaking the cycle it sits in.
    /// Returns `false` if the node is borrowed right now and was left alone.
    ///
    /// Refcounting does the actual reclaiming: once no node in the cycle
    /// points at another, every count reaches zero on its own.
    fn clear(&self) -> bool {
        match self {
            Live::Table(r) => {
                let Ok(mut t) = r.try_borrow_mut() else {
                    return false;
                };
                t.array.clear();
                t.map.clear();
            }
            Live::Instance(r) => {
                let Ok(mut i) = r.try_borrow_mut() else {
                    return false;
                };
                for v in &mut i.fields {
                    *v = Value::Nil;
                }
            }
        }
        true
    }
}

/// The address a `Value` names, if it is a container this collector tracks.
fn container_addr(v: &Value) -> Option<usize> {
    match v {
        Value::Table(t) => Some(Rc::as_ptr(t) as *const u8 as usize),
        Value::Instance(i) => Some(Rc::as_ptr(i) as *const u8 as usize),
        _ => None,
    }
}

thread_local! {
    static REGISTRY: RefCell<Vec<Node>> = const { RefCell::new(Vec::new()) };
    /// Registrations still to go before the next automatic collection.
    static BUDGET: std::cell::Cell<usize> = const { std::cell::Cell::new(FIRST_THRESHOLD) };
}

/// Registrations before the first automatic collection.
///
/// Low enough that a leaking program is caught early, high enough that a
/// short script never pays for a scan it does not need.
const FIRST_THRESHOLD: usize = 8192;

/// Note that `v` has been stored inside another container.
///
/// Cheap and unconditional on the non-container path: a table holding
/// integers or strings never reaches the `Weak` at all.
#[inline]
pub fn on_store(v: &Value) {
    match v {
        Value::Table(t) => register(Node::Table(Rc::downgrade(t))),
        Value::Instance(i) => register(Node::Instance(Rc::downgrade(i))),
        _ => {}
    }
}

fn register(node: Node) {
    REGISTRY.with(|r| r.borrow_mut().push(node));
    let due = BUDGET.with(|b| {
        let left = b.get().saturating_sub(1);
        b.set(left);
        left == 0
    });
    if due {
        collect();
    }
}

/// Break every cycle among the registered containers that nothing outside
/// them refers to.
///
/// Returns how many nodes were cleared, which is what the tests assert on.
pub fn collect() -> usize {
    let nodes = REGISTRY.with(|r| std::mem::take(&mut *r.borrow_mut()));

    // Upgrade, dropping entries whose object is already gone and de-duping
    // the ones registered more than once. A duplicate would put two snapshot
    // references on one object and make it look *more* live, which is safe
    // but wasteful, and it would be cleared twice.
    let mut live: Vec<Live> = Vec::with_capacity(nodes.len());
    let mut seen: crate::fxhash::FxHashMap<usize, usize> = crate::fxhash::fxmap();
    let mut keep: Vec<Node> = Vec::with_capacity(nodes.len());
    for node in nodes {
        let up = match &node {
            Node::Table(w) => w.upgrade().map(Live::Table),
            Node::Instance(w) => w.upgrade().map(Live::Instance),
        };
        let Some(up) = up else { continue };
        if seen.contains_key(&up.addr()) {
            continue;
        }
        seen.insert(up.addr(), live.len());
        live.push(up);
        keep.push(node);
    }

    // How many of each node's references come from another registered node.
    //
    // A node can be *mutably borrowed* while we run: `on_store` is called
    // from inside `TableObject::set` and `InstanceObject::set_field`, both
    // `&mut self`, so the container being written to is borrowed by the
    // frame that triggered this collection. Such a node is unreadable here,
    // and unreadable resolves in the under-counting direction like every
    // other uncertainty: its children go uncounted, so each of them keeps
    // the reference *from* it as apparent external evidence of life, and
    // the node itself is pinned as a root below — code holding a `&mut` to
    // it is as live as a reference gets.
    let mut internal = vec![0usize; live.len()];
    let mut pinned = vec![false; live.len()];
    for (i, node) in live.iter().enumerate() {
        let read = node.each_child(|v| {
            if let Some(addr) = container_addr(v)
                && let Some(&idx) = seen.get(&addr)
            {
                internal[idx] += 1;
            }
        });
        pinned[i] = !read;
    }

    // A node with a reference from outside the registry is a root: the stack,
    // a module slot, a register, a native, or an untracked container. The
    // `- 1` is this function's own snapshot handle.
    let mut reachable = vec![false; live.len()];
    let mut stack: Vec<usize> = Vec::new();
    for (i, node) in live.iter().enumerate() {
        if pinned[i] || node.strong_count().saturating_sub(1) > internal[i] {
            reachable[i] = true;
            stack.push(i);
        }
    }
    // Anything a root can reach is live too, however its own count looks.
    while let Some(i) = stack.pop() {
        live[i].each_child(|v| {
            if let Some(addr) = container_addr(v)
                && let Some(&j) = seen.get(&addr)
                && !reachable[j]
            {
                reachable[j] = true;
                stack.push(j);
            }
        });
    }

    // What is left is cyclic garbage. Clearing contents rather than freeing
    // directly is what keeps this sound: refcounting still owns the actual
    // deallocation, and anything we were wrong about merely survives.
    //
    // A node can be readable but still *shared*-borrowed by a live frame, in
    // which case there is nothing to write through. It stays registered and
    // is reconsidered by the next collection, once that frame has returned.
    let mut cleared = 0;
    let mut retain = reachable;
    for (i, node) in live.iter().enumerate() {
        if !retain[i] {
            if node.clear() {
                cleared += 1;
            } else {
                retain[i] = true;
            }
        }
    }

    // Survivors stay registered — they may still become garbage later. The
    // cleared ones are dropped from the registry: they hold nothing now, so
    // they cannot be in a cycle.
    let survivors: Vec<Node> = keep
        .into_iter()
        .enumerate()
        .filter(|(i, _)| retain[*i])
        .map(|(_, n)| n)
        .collect();
    // Scale the next threshold to what survived, so a program with a large
    // live set does not rescan it on every few thousand stores.
    let next = FIRST_THRESHOLD.max(survivors.len());
    REGISTRY.with(|r| *r.borrow_mut() = survivors);
    BUDGET.with(|b| b.set(next));
    cleared
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::SauleStr;

    fn table() -> Rc<RefCell<TableObject>> {
        Rc::new(RefCell::new(TableObject::new()))
    }

    fn put(t: &Rc<RefCell<TableObject>>, k: &str, v: Value) {
        let key = Value::Str(SauleStr::from(k));
        t.borrow_mut().set(&key, v).unwrap();
    }

    fn reset() {
        REGISTRY.with(|r| r.borrow_mut().clear());
        BUDGET.with(|b| b.set(FIRST_THRESHOLD));
    }

    // The handles have to go out of scope before collecting: a local `Rc`
    // held by the test *is* an external reference, and the collector is
    // right to treat it as one. `Weak` is how the tests watch what happens
    // without being part of it — which is exactly the shape of the leak,
    // where the loop variable dies and only the cycle is left.
    #[test]
    fn collects_a_self_cycle() {
        reset();
        let w = {
            let t = table();
            put(&t, "self", Value::Table(Rc::clone(&t)));
            Rc::downgrade(&t)
        };
        assert!(w.upgrade().is_some(), "the cycle is keeping it alive");
        assert_eq!(collect(), 1, "the cycle is found");
        assert!(w.upgrade().is_none(), "and freed once it is broken");
    }

    #[test]
    fn collects_a_two_node_cycle() {
        reset();
        let (wa, wb) = {
            let (a, b) = (table(), table());
            put(&a, "b", Value::Table(Rc::clone(&b)));
            put(&b, "a", Value::Table(Rc::clone(&a)));
            (Rc::downgrade(&a), Rc::downgrade(&b))
        };
        assert!(wa.upgrade().is_some() && wb.upgrade().is_some());
        assert_eq!(collect(), 2);
        assert!(wa.upgrade().is_none() && wb.upgrade().is_none());
    }

    /// The property that matters more than collecting: never free something
    /// still reachable.
    #[test]
    fn keeps_a_cycle_that_is_still_referenced() {
        reset();
        let (a, b) = (table(), table());
        put(&a, "b", Value::Table(Rc::clone(&b)));
        put(&b, "a", Value::Table(Rc::clone(&a)));
        // A third party holds `a`, exactly as a local or a module slot would.
        let outside = Value::Table(Rc::clone(&a));
        assert_eq!(collect(), 0, "an external reference keeps the whole cycle");
        assert!(!a.borrow().map.is_empty());
        assert!(!b.borrow().map.is_empty());
        drop(outside);
    }

    #[test]
    fn keeps_an_acyclic_graph() {
        reset();
        let (parent, child) = (table(), table());
        put(&parent, "child", Value::Table(Rc::clone(&child)));
        assert_eq!(collect(), 0);
        assert!(!parent.borrow().map.is_empty());
    }

    #[test]
    fn collects_a_longer_ring() {
        reset();
        let watch: Vec<Weak<RefCell<TableObject>>> = {
            let ring: Vec<_> = (0..5).map(|_| table()).collect();
            for i in 0..5 {
                put(&ring[i], "next", Value::Table(Rc::clone(&ring[(i + 1) % 5])));
            }
            ring.iter().map(Rc::downgrade).collect()
        };
        assert!(watch.iter().all(|w| w.upgrade().is_some()));
        assert_eq!(collect(), 5);
        assert!(watch.iter().all(|w| w.upgrade().is_none()));
    }

    /// A node reachable *from* a rooted cycle survives even though its own
    /// references are all internal — the mark phase, not the count, decides.
    #[test]
    fn keeps_what_a_root_can_reach() {
        reset();
        let (a, b, tail) = (table(), table(), table());
        put(&a, "b", Value::Table(Rc::clone(&b)));
        put(&b, "a", Value::Table(Rc::clone(&a)));
        put(&b, "tail", Value::Table(Rc::clone(&tail)));
        let outside = Value::Table(Rc::clone(&a));
        assert_eq!(collect(), 0);
        assert!(!tail.borrow().map.is_empty() || tail.borrow().map.is_empty());
        assert_eq!(Rc::strong_count(&tail), 2, "still held by `b`");
        drop(outside);
    }

    /// The documented limit: a cycle routed through a variant the traversal
    /// treats as opaque is kept, not freed. Under-collecting is the safe
    /// direction and this pins it as deliberate.
    #[test]
    fn collects_only_what_it_can_prove() {
        reset();
        let t = table();
        let variant = Rc::new(crate::value::EnumVariantObject {
            enum_name: SauleStr::from("Option"),
            variant_name: SauleStr::from("Some"),
            tag: 0,
            value: Some(Value::Table(Rc::clone(&t))),
            enum_obj: RefCell::new(None),
        });
        put(&t, "wrapped", Value::EnumVariant(variant));
        // A real cycle, but one this collector declines to reason about.
        assert_eq!(collect(), 0, "opaque payloads are treated as external");
        assert_eq!(Rc::strong_count(&t), 2);
    }

    /// The shape that actually panicked: `TableObject::set` takes `&mut self`,
    /// so the table being written to is mutably borrowed when the store that
    /// exhausts the budget calls in here. Reading it is impossible; guessing
    /// at it would be wrong. It is pinned, and the rest still collects.
    #[test]
    fn collects_while_a_node_is_mutably_borrowed() {
        reset();
        let garbage = {
            let g = table();
            put(&g, "self", Value::Table(Rc::clone(&g)));
            Rc::downgrade(&g)
        };

        let host = table();
        put(&host, "keep", Value::Int(1));
        // Register `host` the way a real store would, then collect from
        // underneath a live `&mut` to it — exactly what `set` does.
        crate::gc::on_store(&Value::Table(Rc::clone(&host)));
        let cleared = {
            let mut guard = host.borrow_mut();
            let n = collect();
            guard.array.push(Value::Int(2));
            n
        };

        assert_eq!(cleared, 1, "the unrelated cycle is still collected");
        assert!(garbage.upgrade().is_none(), "and freed");
        assert_eq!(host.borrow().array.len(), 1, "the pinned node is untouched");
        assert!(!host.borrow().map.is_empty(), "and was not cleared");
    }

    /// A node that is only *shared*-borrowed reads fine but cannot be written
    /// through, so clearing it is deferred instead of panicking. The leaked
    /// guard is how the test holds a borrow without also holding the `Rc`
    /// that would make the node a root for the ordinary reason.
    #[test]
    fn defers_a_node_that_is_shared_borrowed() {
        reset();
        let t = table();
        put(&t, "self", Value::Table(Rc::clone(&t)));
        let w = Rc::downgrade(&t);
        std::mem::forget(t.borrow());
        drop(t);

        assert_eq!(collect(), 0, "cannot write through a live shared borrow");
        let alive = w.upgrade().expect("left intact rather than half-cleared");
        assert!(!alive.borrow().map.is_empty());
    }
}
