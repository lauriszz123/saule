---
title: "Table"
description: "Tables in Saule double as arrays (integer-keyed, 1-based) and maps. These helpers operate on the array side; map operations are just t[k] / t[k] = v."
sidebar:
  order: 4
---

<!-- Generated from DOCS.md by `npm run sync-docs`. Edit that file, not this one. -->

Tables in Saule double as arrays (integer-keyed, 1-based) and maps. These
helpers operate on the array side; map operations are just `t[k]` / `t[k] = v`.

| Signature | Description |
| --- | --- |
| `Table.insert(t: table<any>, value: any, pos: integer?) -> nil` | Append by default, or insert at `pos` shifting elements right. |
| `Table.remove(t: table<any>, pos: integer?) -> any?` | Pop from the end by default, or remove at `pos` shifting left. Returns the removed value (or `nil`). |
| `Table.sort(t: table<any>, cmp: fn(any, any): boolean) -> nil` | Sort in place; `cmp(a, b)` should return `true` when `a` precedes `b`. |
| `Table.concat(t: table<any>, sep: string?, from: integer?, to: integer?) -> string` | Join string/number elements with `sep` (default `""`). |

```saule
local xs: table<integer> = {3, 1, 4, 1, 5}
Table.sort(xs, fn(a, b) => a < b)
println(Table.concat(xs, ", "))                 -- 1, 1, 3, 4, 5
local last: integer = Table.remove(xs)!         -- 5
```

---
