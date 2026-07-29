---
title: "Tables"
description: "Tables are Saule's only data structure — same model as Lua. A single table holds both an array part (contiguous 1-based integer keys) and a map part…"
sidebar:
  order: 3
---

<!-- Generated from README.md by `npm run sync-docs`. Edit that file, not this one. -->

Tables are Saule's only data structure — same model as Lua. A single table holds both an **array part** (contiguous 1-based integer keys) and a **map part** (everything else: strings, booleans, non-positive integers). The two parts share one value space and one length.

### Table Types

A table type is written in one of two forms:

| Form | Meaning |
|---|---|
| `table<V>` | Array: integer keys, `V` values |
| `table<K, V>` | Map: `K` keys (`integer` or `string`), `V` values |
| `table` | Any table — element types unknown |

`table<V>` and `table<integer, V>` are the **same type**; the array form just
leaves the implicit integer key unwritten.

```saule
local names: table<string> = {"alice", "bob"}          -- array of string
local same: table<integer, string> = names             -- identical type
local ages: table<string, integer> = {alice: 30}       -- string-keyed map
local nested: table<table<string>> = {names}           -- tables nest
```

### Element Types Are Invariant

Tables are mutable, so a `table<Dog>` is **not** a `table<Animal>` — in
either direction. Writing through the wider name would put an `Animal` into
a table the narrower name still believes holds only `Dog`s:

```saule
local dogs: table<Dog> = {}
local animals: table<Animal> = dogs    -- ERROR: table<Dog> is not table<Animal>
Table.insert(animals, Animal())        -- ...this is why
```

The same applies to key types: `table<string, integer>` and
`table<integer, integer>` are unrelated.

An empty `{}` literal has no element type yet, so it fills any table slot,
and a bare `table` annotation accepts anything.

### Literals

```saule
-- Array part (positional entries — auto-indexed from 1).
local nums: table<integer> = {10, 20, 30}
print(nums[1])     -- 10
print(#nums)       -- 3

-- Map part (named entries).
local p: table = { name: "Arthur", health: 100, alive: true }

-- Mixed: positional first, then named.
local mix: table = { "a", "b", color: "red", 99 }
```

Keys in `{ key: value }` literals can be bare identifiers or quoted strings — both produce a string-keyed map entry.

### Indexed Access

`t[k]` accepts any value as a key:

```saule
local scores: table = {}
scores["arthur"] = 50
scores["merlin"] = 80
scores[1] = "first place"
```

### Lua-style Dotted Access

`t.foo` is equivalent to `t["foo"]` — both read and write, in any combination:

```saule
local cfg: table = {}
cfg.title = "My Game"        -- same as cfg["title"] = "My Game"
cfg["width"] = 1920          -- same as cfg.width = 1920

print(cfg.title)             -- "My Game"
print(cfg["width"])          -- 1920
print(cfg.missing)           -- nil (no error — missing keys yield nil)
```

This is plain map sugar, so it only applies to **tables**. Class instances and statics keep their strict `obj.field` semantics: writing a previously-undeclared field on an instance is still a compile error.

### Length

`#t` returns the array length — the count of contiguous integer keys starting at `1`. Map entries don't contribute to `#`:

```saule
local t: table = {10, 20, 30, name: "tags"}
print(#t)    -- 3
```

### Removing Keys

Assigning `nil` does **not** delete a map entry (so JSON-style `{"x": null}` round-trips faithfully). Use `Table.remove(t, key)` to actually drop a key:

```saule
local user: table = { name: "Arthur", draft: true }
Table.remove(user, "draft")
print(user.draft)    -- nil
```

For the array part, `Table.remove(t, i)` shifts subsequent elements down (standard Lua behaviour).

### Iterating

`for v in t` walks the array part. `for k, v in t` walks both the array and map parts (key/value iteration). See [Loops](/saule/language/loops/) and the [Standard Library](/saule/stdlib/table/) for the full set of helpers (`Table.insert`, `Table.sort`, `Table.concat`, …).

---
