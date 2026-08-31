---
title: "Table"
description: "Tables in Saule double as arrays (integer-keyed, 1-based) and maps. Most of these helpers operate on the array side; keys / values see both, and map…"
sidebar:
  order: 4
---

<!-- Generated from DOCS.md by `npm run sync-docs`. Edit that file, not this one. -->

Tables in Saule double as arrays (integer-keyed, 1-based) and maps. Most of
these helpers operate on the array side; `keys` / `values` see both, and map
access itself is just `t[k]` / `t[k] = v`.

| Signature | Description |
| --- | --- |
`Table` is the **mutating** half of the sequence API: it changes a table in
place, or answers a question about one. For deriving a *new* sequence from an
existing one — `map`, `filter`, `reduce` — see [`Iter`](/saule/stdlib/iter/). No name means
two different things across the two.

### Changing a table

| Signature | Description |
| --- | --- |
| `Table.insert<V>(t: table<V>, value: V, pos: integer?) -> nil` | Append by default, or insert at `pos` shifting elements right. |
| `Table.remove<V>(t: table<V>, pos: integer?) -> V?` | Pop from the end by default, or remove at `pos` shifting left. Returns the removed value (or `nil`). |
| `Table.sort<V>(t: table<V>, cmp: fn(V, V) -> boolean) -> nil` | Sort in place; `cmp(a, b)` should return `true` when `a` precedes `b`. |
| `Table.reverse<V>(t: table<V>) -> nil` | Reverse the array part in place. `Iter.reverse` returns a new table instead. |
| `Table.clear<V>(t: table<V>) -> nil` | Remove every entry, array and map alike. |

### Reading a table

| Signature | Description |
| --- | --- |
| `Table.len<V>(t: table<V>) -> integer` | Array length — the same number `#t` gives, as a value you can pass around. |
| `Table.contains<V>(t: table<V>, value: V) -> boolean` | Is `value` in the array part? Compares with `==`, so a class implementing `OpEq` is matched by its own rule. |
| `Table.indexOf<V>(t: table<V>, value: V) -> integer?` | 1-based position of the first `==` match, or `nil`. For a *predicate* search use `Iter.findIndex`. |
| `Table.keys<K, V>(t: table<K, V>) -> table<K>` | Every key: array indices `1..#t` in order, then the map's keys in no particular order. |
| `Table.values<K, V>(t: table<K, V>) -> table<V>` | Every value, in the same order as `Table.keys`. |
| `Table.slice<V>(t: table<V>, from: integer, to: integer?) -> table<V>` | New table over the 1-based range; `to` defaults to the end. Negative indices count from the end, and an out-of-range slice is empty rather than an error. |
| `Table.copy<V>(t: table<V>) -> table<V>` | Shallow copy of both halves. Elements are shared, so copying a table of instances gives a new table pointing at the same instances. |
| `Table.concat<V>(t: table<V>, sep: string?, from: integer?, to: integer?) -> string` | Join the elements with `sep` (default `""`), each rendered as `tostring` would. `String.join(sep, t)` is the same operation with the arguments the other way round. |

```saule
local xs: table<integer> = {3, 1, 4, 1, 5}
Table.sort(xs, (a, b) => a < b)
println(Table.concat(xs, ", "))                 -- 1, 1, 3, 4, 5
local last: integer = Table.remove(xs)!         -- 5

println(Table.indexOf(xs, 3) ?? 0)              -- 3
println(Table.concat(Table.slice(xs, -2), " ")) -- 3 4
```

There is no `Table.unpack`. A function returning "however many values the
table holds" has no type Saule can write — the arity is not known until the
call runs — so the table is passed as a table.

---
