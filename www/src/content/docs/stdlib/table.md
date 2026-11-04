---
title: "Table"
description: "A table is one structure doing two jobs: an array keyed by consecutive integers from 1, and a map keyed by strings, integers or booleans. Most of what…"
sidebar:
  order: 5
---

<!-- Generated from DOCS.md by `npm run sync-docs`. Edit that file, not this one. -->

A table is one structure doing two jobs: an **array** keyed by consecutive
integers from 1, and a **map** keyed by strings, integers or booleans. Most
of what is here works on the array side; `keys` and `values` see both, and
map access itself is just `t[k]` and `t[k] = v`.

`Table` is the **mutating** half of the sequence API: it changes the table it
is given, or answers a question about one. For deriving a *new* sequence —
`map`, `filter`, `reduce` — see [`Iter`](/saule/stdlib/iter/). No name means two different
things across the two halves: `Table.reverse` reverses in place,
`Iter.reverse` returns a new table.

```saule
local scores: table<integer> = {10, 20, 30}
local byName: table<string, integer> = {ada: 1, grace: 2}

println(#scores)              -- 3, the array length
println(scores[2] ?? 0)       -- 20, 1-based
println(byName["ada"] ?? 0)   -- 1
```

### Changing a table

<div class="api-list">
<details class="api-entry" id="table-insert">
<summary><code class="api-sig"><span class="sig-fn">Table.insert</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">t</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">,</span> <span class="sig-param">value</span><span class="sig-punct">:</span> <span class="sig-type">V</span><span class="sig-punct">,</span> <span class="sig-param">pos</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">?</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-kw">nil</span></code></summary>


Appends `value` to the end of the array part, or inserts it at `pos`,
shifting everything from `pos` onward one place to the right.

```saule
local xs: table<integer> = {1, 2, 3}

Table.insert(xs, 4)
println(Table.concat(xs, ","))      -- 1,2,3,4

Table.insert(xs, 0, 1)
println(Table.concat(xs, ","))      -- 0,1,2,3,4

-- Building a list in a loop.
local squares: table<integer> = {}
for n = 1, 4 do
    Table.insert(squares, n * n)
end
println(Table.concat(squares, " "))  -- 1 4 9 16
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="table-remove">
<summary><code class="api-sig"><span class="sig-fn">Table.remove</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">t</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">,</span> <span class="sig-param">pos</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">?</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">V</span><span class="sig-punct">?</span></code></summary>


Removes and returns the last element, or the one at `pos`, shifting the rest
left. `nil` when there is nothing to remove, which is what makes a table
usable as a stack without a separate emptiness check.

```saule
local stack: table<integer> = {1, 2, 3}

println(Table.remove(stack) ?? 0)       -- 3, from the end
println(Table.concat(stack, ","))       -- 1,2
println(Table.remove(stack, 1) ?? 0)    -- 1, from the front
println(Table.concat(stack, ","))       -- 2

-- Draining a stack until it is empty.
local queue: table<string> = {"a", "b"}
while #queue > 0 do
    println(Table.remove(queue, 1) ?? "")   -- a, then b
end
println(Table.remove(queue) == nil)         -- true, nothing left
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="table-sort">
<summary><code class="api-sig"><span class="sig-fn">Table.sort</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">t</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">,</span> <span class="sig-param">cmp</span><span class="sig-punct">:</span> <span class="sig-kw">fn</span><span class="sig-punct">(</span><span class="sig-type">V</span><span class="sig-punct">,</span> <span class="sig-type">V</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">boolean</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-kw">nil</span></code></summary>


Sorts the array part in place. `cmp(a, b)` answers `true` when `a` must come
before `b`.

For ordering by an extracted key rather than a comparator, see
[`Iter.sortBy`](/saule/stdlib/iter/#iter-sortby), which returns a new table.

```saule
local xs: table<integer> = {3, 1, 4, 1, 5}

Table.sort(xs, (a, b) => a < b)
println(Table.concat(xs, ","))       -- 1,1,3,4,5

Table.sort(xs, (a, b) => a > b)
println(Table.concat(xs, ","))       -- 5,4,3,1,1

-- Sorting by something other than the value itself. The comparator's
-- parameters take their types from the table, so they need no annotation.
local words: table<string> = {"pear", "fig", "banana"}
Table.sort(words, (a, b) => String.len(a) < String.len(b))
println(Table.concat(words, ","))    -- fig,pear,banana
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="table-reverse">
<summary><code class="api-sig"><span class="sig-fn">Table.reverse</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">t</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-kw">nil</span></code></summary>


Reverses the array part in place. [`Iter.reverse`](/saule/stdlib/iter/#iter-reverse)
returns a new table instead.

```saule
local xs: table<integer> = {1, 2, 3}
Table.reverse(xs)
println(Table.concat(xs, ","))      -- 3,2,1

-- In place: the original binding changes.
local letters: table<string> = {"a", "b", "c"}
Table.reverse(letters)
println(letters[1] ?? "")           -- c
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="table-clear">
<summary><code class="api-sig"><span class="sig-fn">Table.clear</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">t</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-kw">nil</span></code></summary>


Removes every entry, array part and map part alike. The table itself stays
the same table, so anything else holding a reference to it sees it empty.

```saule
local named: table<string, integer> = {a: 1, b: 2}
Table.clear(named)
println(named["a"] == nil)  -- true

local xs: table<integer> = {1, 2, 3}
Table.clear(xs)
println(#xs)                -- 0

-- Reusing a buffer rather than allocating a new one.
local batch: table<integer> = {}
for round = 1, 2 do
    Table.clear(batch)
    Table.insert(batch, round)
    println(#batch)         -- 1, both times
end
```


</details>
</div>

### Reading a table

<div class="api-list">
<details class="api-entry" id="table-len">
<summary><code class="api-sig"><span class="sig-fn">Table.len</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">t</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">integer</span></code></summary>


The array length — the same number `#t` gives, as a value you can pass
around.

```saule
println(Table.len({1, 2, 3}))        -- 3
println(Table.len({}))               -- 0

-- `#` is the operator form; this is the one you can hand to something else.
local sizes: table<integer> = Iter.map({{1}, {1, 2}}, t => Table.len(t))
println(Table.concat(sizes, ","))    -- 1,2
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="table-contains">
<summary><code class="api-sig"><span class="sig-fn">Table.contains</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">t</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">,</span> <span class="sig-param">value</span><span class="sig-punct">:</span> <span class="sig-type">V</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">boolean</span></code></summary>


Whether `value` appears in the array part. Compares with `==`, so a class
implementing [`OpEq`](/saule/stdlib/prelude-always-in-scope/#prelude-interfaces) is matched by its own rule rather
than by identity.

```saule
println(Table.contains({1, 2, 3}, 2))        -- true
println(Table.contains({1, 2, 3}, 9))        -- false
println(Table.contains({"a", "b"}, "b"))     -- true

-- Filtering out what is already known.
local seen: table<string> = {"ada"}
for name in {"ada", "grace"} do
    if not Table.contains(seen, name) then
        println("new: " .. name)             -- new: grace
    end
end
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="table-indexof">
<summary><code class="api-sig"><span class="sig-fn">Table.indexOf</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">t</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">,</span> <span class="sig-param">value</span><span class="sig-punct">:</span> <span class="sig-type">V</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">integer</span><span class="sig-punct">?</span></code></summary>


The 1-based position of the first `==` match, or `nil`. For a **predicate**
search, use [`Iter.findIndex`](/saule/stdlib/iter/#iter-findindex).

```saule
println(Table.indexOf({"a", "b", "c"}, "b") ?? 0)    -- 2
println(Table.indexOf({1, 2}, 9) == nil)             -- true

-- Removing a known value.
local xs: table<string> = {"a", "b", "c"}
local at: integer? = Table.indexOf(xs, "b")
if at != nil then
    Table.remove(xs, at!)
end
println(Table.concat(xs, ","))                       -- a,c
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="table-keys">
<summary><code class="api-sig"><span class="sig-fn">Table.keys</span><span class="sig-punct">&lt;</span><span class="sig-type">K</span><span class="sig-punct">,</span> <span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">t</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">K</span><span class="sig-punct">,</span> <span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">K</span><span class="sig-punct">&gt;</span></code></summary>


Every key: the array indices `1..#t` in order, then the map's keys in no
particular order.

```saule
local t: table<string, integer> = {}
t["ada"] = 1
t["grace"] = 2

local names: table<string> = Table.keys(t)
println(#names)                      -- 2

-- Array indices come out in order.
println(Table.concat(Table.keys({"x", "y", "z"}), ","))   -- 1,2,3
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="table-values">
<summary><code class="api-sig"><span class="sig-fn">Table.values</span><span class="sig-punct">&lt;</span><span class="sig-type">K</span><span class="sig-punct">,</span> <span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">t</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">K</span><span class="sig-punct">,</span> <span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span></code></summary>


Every value, in the same order as `Table.keys`.

```saule
println(Table.concat(Table.values({"x", "y"}), ","))   -- x,y

local ages: table<string, integer> = {}
ages["ada"] = 36
println(Table.values(ages)[1] ?? 0)                    -- 36

-- Summing a map's values, whatever its keys are.
local counts: table<string, integer> = {}
counts["a"] = 2
counts["b"] = 3
println(Iter.reduce(Table.values(counts), 0, (acc, n) => acc + n))   -- 5
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="table-slice">
<summary><code class="api-sig"><span class="sig-fn">Table.slice</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">t</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">,</span> <span class="sig-param">from</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">,</span> <span class="sig-param">to</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">?</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span></code></summary>


A **new** table over the 1-based range, `to` defaulting to the end. Negative
indices count from the end, and an out-of-range slice is empty rather than an
error.

```saule
local xs: table<integer> = {1, 2, 3, 4, 5}

println(Table.concat(Table.slice(xs, 2, 3), ","))    -- 2,3
println(Table.concat(Table.slice(xs, 3), ","))       -- 3,4,5
println(Table.concat(Table.slice(xs, -2), ","))      -- 4,5
println(#Table.slice(xs, 99, 120))                   -- 0

-- The original is untouched.
println(Table.concat(xs, ","))                       -- 1,2,3,4,5
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="table-copy">
<summary><code class="api-sig"><span class="sig-fn">Table.copy</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">t</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span></code></summary>


A shallow copy of both halves. Elements are **shared**, so copying a table of
instances gives a new table pointing at the same instances.

```saule
local xs: table<integer> = {1, 2}
local ys: table<integer> = Table.copy(xs)

Table.insert(ys, 3)
println(Table.concat(xs, ","))      -- 1,2, unchanged
println(Table.concat(ys, ","))      -- 1,2,3

-- Shallow: a nested table is the same table in both copies.
local nested: table<table<integer>> = {{1}}
local clone: table<table<integer>> = Table.copy(nested)
Table.insert(clone[1] ?? {}, 2)
println(#(nested[1] ?? {}))         -- 2, the inner table is shared
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="table-concat">
<summary><code class="api-sig"><span class="sig-fn">Table.concat</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">t</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">,</span> <span class="sig-param">sep</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">?</span><span class="sig-punct">,</span> <span class="sig-param">from</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">?</span><span class="sig-punct">,</span> <span class="sig-param">to</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">?</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">string</span></code></summary>


Joins the elements with `sep` between them, rendering each as `tostring`
would. `sep` defaults to the empty string, and `from` / `to` restrict the
range.

[`String.join`](/saule/stdlib/string/#string-join) is the same operation with
the arguments the other way round.

```saule
println(Table.concat({1, 2, 3}, ", "))        -- 1, 2, 3
println(Table.concat({"a", "b"}))             -- ab, no separator
println(Table.concat({1, 2, 3, 4}, "-", 2, 3))   -- 2-3

-- Rendering a row.
local row: table<string> = {"ada", "36"}
println("| " .. Table.concat(row, " | ") .. " |")   -- | ada | 36 |
```


</details>
</div>

### There is no `Table.unpack`

A function returning "however many values the table holds" has no type Saule
can write — the arity is not known until the call runs — so the table is
passed as a table.
