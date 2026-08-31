---
title: "Iter"
description: "Combinators over sequences. Where Table mutates a table or answers a question about one, Iter derives a new sequence and never writes to its argument."
---

<!-- Generated from DOCS.md by `npm run sync-docs`. Edit that file, not this one. -->

Combinators over sequences. Where [`Table`](/saule/stdlib/table/) mutates a table or
answers a question about one, `Iter` derives a **new** sequence and never
writes to its argument.

Every combinator is **eager**: `table` in, `table` out. That is what keeps
them typed — `Iter.map<V, U>(t: table<V>, f: fn(V) -> U)` binds `V` from the
receiver, so a lambda written without annotations still gets real parameter
types, and the result is a real `table<U>`:

```saule
local names: table<string> = Iter.map(users, u => u.name)
local adults: table<User>  = Iter.filter(users, u => u.age >= 18)
local total: integer       = Iter.reduce(users, 0, (acc, u) => acc + u.age)
```

### Other sources

A **step closure** or an **`Iterable`** reaches the combinators through
`Iter.collect`, which drains it into a table:

```saule
Iter.map(Iter.collect(step), f)          -- a bare step closure
Iter.map(Iter.collect(list.iter()), f)   -- anything Iterable
```

That call is not overhead the design added: an eager `map` has to drain the
source before it can run, so `collect` only makes visible a cost that was
always there. It is a separate call rather than an overload because
`Iterable<V>` cannot be written in a native signature — but `iter()` is
declared by your own class, so `Iter.collect(list.iter())` is checked end to
end. A closure that never returns `nil` never terminates, exactly as the
hand-written `for` loop would not.

### Core

| Signature | Description |
| --- | --- |
| `Iter.collect<V>(step: fn() -> V?) -> table<V>` | Drain a step closure until it answers `nil`. |
| `Iter.map<V, U>(t: table<V>, f: fn(V) -> U) -> table<U>` | Apply `f` to every element. |
| `Iter.filter<V>(t: table<V>, pred: fn(V) -> boolean) -> table<V>` | Keep the elements `pred` accepts. |
| `Iter.reduce<V, A>(t: table<V>, init: A, f: fn(A, V) -> A) -> A` | Fold left from `init`. |
| `Iter.forEach<V>(t: table<V>, f: fn(V) -> nil) -> nil` | Run `f` for its effect on every element. |

### Search

| Signature | Description |
| --- | --- |
| `Iter.find<V>(t: table<V>, pred: fn(V) -> boolean) -> V?` | First element `pred` accepts, or `nil`. |
| `Iter.findIndex<V>(t: table<V>, pred: fn(V) -> boolean) -> integer?` | Its 1-based position. Named apart from `Table.indexOf`, which searches for a *value* rather than with a predicate. |
| `Iter.any<V>(t: table<V>, pred: fn(V) -> boolean) -> boolean` | Does any element match? `false` for an empty table. |
| `Iter.all<V>(t: table<V>, pred: fn(V) -> boolean) -> boolean` | Do all of them? `true` for an empty table. |
| `Iter.count<V>(t: table<V>, pred: fn(V) -> boolean) -> integer` | How many match. |

### Slicing

| Signature | Description |
| --- | --- |
| `Iter.take<V>(t: table<V>, n: integer) -> table<V>` | The first `n`, or all of them if there are fewer. |
| `Iter.skip<V>(t: table<V>, n: integer) -> table<V>` | Everything after the first `n`. |
| `Iter.first<V>(t: table<V>) -> V?` | First element, `nil` when empty. |
| `Iter.last<V>(t: table<V>) -> V?` | Last element, `nil` when empty. |
| `Iter.chunk<V>(t: table<V>, size: integer) -> table<table<V>>` | Fixed-size groups; the last is short when the length doesn't divide evenly. |

### Shaping

| Signature | Description |
| --- | --- |
| `Iter.zipWith<V, U, A>(a: table<V>, b: table<U>, f: fn(V, U) -> A) -> table<A>` | Pair the two up and combine, stopping at the shorter. |
| `Iter.flatten<V>(t: table<table<V>>) -> table<V>` | Concatenate the inner tables. One level only — nest the call to go deeper. |
| `Iter.reverse<V>(t: table<V>) -> table<V>` | A new reversed table. `Table.reverse` reverses in place instead. |
| `Iter.unique<V>(t: table<V>) -> table<V>` | Drop later duplicates, keeping input order. Compares with `==`, so `OpEq` applies. |
| `Iter.sortBy<V, K>(t: table<V>, key: fn(V) -> K) -> table<V>` | New table sorted ascending by the extracted key, using `<` (so `OpCompare` applies). Stable. For a bespoke ordering use `Table.sort`, which takes the comparator directly. |
| `Iter.groupBy<V, K>(t: table<V>, key: fn(V) -> K) -> table<K, table<V>>` | Bucket the elements by key. The key must be a `string`, `integer` or `boolean` — the types a table can be keyed by. |

There is no `Iter.zip` or `Iter.enumerate`. Both would have to return pairs,
and a pair has no representation here: a two-element table holding an
`integer` and a `V` types as `table<any>`, which loses both. `zipWith`
combines at the point the two elements meet instead, and a `for i, v in t do`
loop already gives you indices alongside values.

```saule
local scores: table<integer> = {84, 17, 96, 42}

println(Iter.count(scores, s => s >= 50))                   -- 2
println(Iter.find(scores, s => s > 90) ?? 0)                -- 96
println(String.join(",", Iter.sortBy(scores, s => s)))      -- 17,42,84,96
println(#Iter.chunk(scores, 3))                             -- 2
```

---
