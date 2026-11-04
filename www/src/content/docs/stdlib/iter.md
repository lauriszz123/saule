---
title: "Iter"
description: "Combinators over sequences. Where Table mutates a table or answers a question about one, Iter derives a new sequence and never writes to its argument."
sidebar:
  order: 6
---

<!-- Generated from DOCS.md by `npm run sync-docs`. Edit that file, not this one. -->

Combinators over sequences. Where [`Table`](/saule/stdlib/table/) mutates a table or
answers a question about one, `Iter` derives a **new** sequence and never
writes to its argument.

Every combinator is **eager**: `table` in, `table` out. That is what keeps
them typed. `Iter.map<V, U>(t: table<V>, f: fn(V) -> U)` binds `V` from the
receiver, so a lambda written without annotations still gets a real parameter
type, and the result is a real `table<U>` rather than an opaque iterator.

```saule
local nums: table<integer> = {1, 2, 3, 4, 5}

local doubled: table<integer> = Iter.map(nums, n => n * 2)
local evens: table<integer> = Iter.filter(nums, n => n % 2 == 0)
local total: integer = Iter.reduce(nums, 0, (acc, n) => acc + n)

println(Table.concat(doubled, ","))   -- 2,4,6,8,10
println(Table.concat(evens, ","))     -- 2,4
println(total)                        -- 15
```

A **step closure** or an **`Iterable`** reaches these through `Iter.collect`,
which drains it into a table first. That call is not overhead the design
added: an eager `map` has to drain the source before it can run, so `collect`
only makes visible a cost that was always there.

### Collecting

<div class="api-list">
<details class="api-entry" id="iter-collect">
<summary><code class="api-sig"><span class="sig-fn">Iter.collect</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">step</span><span class="sig-punct">:</span> <span class="sig-kw">fn</span><span class="sig-punct">(</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">V</span><span class="sig-punct">?</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span></code></summary>


Drains a step closure until it answers `nil`, collecting what it produced.
This is the bridge from anything iterable — a `String.iter`, a `File.lines`,
a class implementing `Iterable` — into the rest of this module.

A closure that never answers `nil` never terminates, exactly as a
hand-written `for` loop over it would not.

```saule
-- A step closure answers one value at a time, and `nil` when it is done.
local n: integer = 0
local countdown: fn() -> integer? = fn() -> integer?
    n = n + 1
    if n > 3 then
        return nil
    end
    return n
end

local collected: table<integer> = Iter.collect(countdown)
println(Table.concat(collected, ","))     -- 1,2,3

-- Which is what puts the combinators within reach of anything iterable: a
-- class with an `iter()` method reaches them as `Iter.collect(x.iter())`.
println(Iter.reduce(collected, 0, (acc, v) => acc + v))   -- 6
```

Note that a closure yielding **two** values — `String.iter`, or an
`Iterable2` — collects into a table of pairs, not a table of its first
value. Those are for `for k, v in ... do` rather than for this.


</details>
</div>

### Transforming

<div class="api-list">
<details class="api-entry" id="iter-map">
<summary><code class="api-sig"><span class="sig-fn">Iter.map</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">,</span> <span class="sig-type">U</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">t</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">,</span> <span class="sig-param">f</span><span class="sig-punct">:</span> <span class="sig-kw">fn</span><span class="sig-punct">(</span><span class="sig-type">V</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">U</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">U</span><span class="sig-punct">&gt;</span></code></summary>


Applies `f` to every element, giving a new table of the results. The result
type follows `f`'s return type, so mapping to a different type is ordinary.

```saule
println(Table.concat(Iter.map({1, 2, 3}, n => n * 2), ","))        -- 2,4,6

-- The element type can change.
local lengths: table<integer> = Iter.map({"a", "bbb"}, s => String.len(s))
println(Table.concat(lengths, ","))                                -- 1,3

-- The original is untouched.
local xs: table<integer> = {1, 2}
local ys: table<integer> = Iter.map(xs, n => n + 10)
println(Table.concat(xs, ",") .. " / " .. Table.concat(ys, ","))   -- 1,2 / 11,12
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="iter-filter">
<summary><code class="api-sig"><span class="sig-fn">Iter.filter</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">t</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">,</span> <span class="sig-param">pred</span><span class="sig-punct">:</span> <span class="sig-kw">fn</span><span class="sig-punct">(</span><span class="sig-type">V</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">boolean</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span></code></summary>


Keeps the elements `pred` accepts, in their original order.

```saule
println(Table.concat(Iter.filter({1, 2, 3, 4}, n => n % 2 == 0), ","))   -- 2,4
println(#Iter.filter({1, 2, 3}, n => n > 9))                             -- 0

-- Chaining with `map`: filter first, so `map` does less work.
local names: table<string> = {"ada.sau", "readme.md", "main.sau"}
local saule: table<string> = Iter.filter(names, n => String.ends(n, ".sau"))
println(Table.concat(saule, ", "))                                       -- ada.sau, main.sau
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="iter-reduce">
<summary><code class="api-sig"><span class="sig-fn">Iter.reduce</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">,</span> <span class="sig-type">A</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">t</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">,</span> <span class="sig-param">init</span><span class="sig-punct">:</span> <span class="sig-type">A</span><span class="sig-punct">,</span> <span class="sig-param">f</span><span class="sig-punct">:</span> <span class="sig-kw">fn</span><span class="sig-punct">(</span><span class="sig-type">A</span><span class="sig-punct">,</span> <span class="sig-type">V</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">A</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">A</span></code></summary>


Folds left from `init`, threading an accumulator through every element. The
accumulator may be any type, not only the element type.

```saule
println(Iter.reduce({1, 2, 3}, 0, (acc, n) => acc + n))        -- 6
println(Iter.reduce({1, 2, 3}, 1, (acc, n) => acc * n))        -- 6

-- The accumulator can be a different type from the elements.
local joined: string = Iter.reduce({"a", "b"}, "", (acc, s) => acc .. s)
println(joined)                                                -- ab

-- An empty table gives back `init` untouched.
println(Iter.reduce({}, 42, (acc, n) => acc + 1))              -- 42
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="iter-foreach">
<summary><code class="api-sig"><span class="sig-fn">Iter.forEach</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">t</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">,</span> <span class="sig-param">f</span><span class="sig-punct">:</span> <span class="sig-kw">fn</span><span class="sig-punct">(</span><span class="sig-type">V</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-kw">nil</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-kw">nil</span></code></summary>


Runs `f` for its effect on every element and answers nothing. Use it when the
point is the side effect; use `map` when the point is the result.

```saule
Iter.forEach({1, 2, 3}, n => println(n))     -- 1, then 2, then 3

-- Accumulating into something outside the loop.
local total: integer = 0
Iter.forEach({10, 20}, n => bump(n))
println(total)                               -- 30

fn bump(n: integer)
    total = total + n
end
```


</details>
</div>

### Search

<div class="api-list">
<details class="api-entry" id="iter-find">
<summary><code class="api-sig"><span class="sig-fn">Iter.find</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">t</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">,</span> <span class="sig-param">pred</span><span class="sig-punct">:</span> <span class="sig-kw">fn</span><span class="sig-punct">(</span><span class="sig-type">V</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">boolean</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">V</span><span class="sig-punct">?</span></code></summary>


The first element `pred` accepts, or `nil` when none does.

```saule
println(Iter.find({1, 5, 9}, n => n > 4) ?? 0)        -- 5
println(Iter.find({1, 2}, n => n > 9) == nil)         -- true

-- Looking something up by a property of it.
local names: table<string> = {"ada", "grace", "alan"}
println(Iter.find(names, n => String.starts(n, "g")) ?? "none")   -- grace
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="iter-findindex">
<summary><code class="api-sig"><span class="sig-fn">Iter.findIndex</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">t</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">,</span> <span class="sig-param">pred</span><span class="sig-punct">:</span> <span class="sig-kw">fn</span><span class="sig-punct">(</span><span class="sig-type">V</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">boolean</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">integer</span><span class="sig-punct">?</span></code></summary>


The 1-based position of the first element `pred` accepts, or `nil`.

Named apart from [`Table.indexOf`](/saule/stdlib/table/#table-indexof), which
searches for a **value** rather than with a predicate.

```saule
println(Iter.findIndex({1, 5, 9}, n => n > 4) ?? 0)       -- 2
println(Iter.findIndex({1, 2}, n => n > 9) == nil)        -- true

-- Finding a position in order to change it.
local xs: table<integer> = {3, 8, 2}
local at: integer? = Iter.findIndex(xs, n => n > 5)
if at != nil then
    Table.remove(xs, at!)
end
println(Table.concat(xs, ","))                            -- 3,2
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="iter-any">
<summary><code class="api-sig"><span class="sig-fn">Iter.any</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">t</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">,</span> <span class="sig-param">pred</span><span class="sig-punct">:</span> <span class="sig-kw">fn</span><span class="sig-punct">(</span><span class="sig-type">V</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">boolean</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">boolean</span></code></summary>


Whether **any** element matches. `false` for an empty table.

```saule
println(Iter.any({1, 2}, n => n > 1))      -- true
println(Iter.any({1, 2}, n => n > 9))      -- false
println(Iter.any({}, n => true))           -- false

-- Guarding on a property of a whole list.
local files: table<string> = {"a.md", "b.sau"}
if Iter.any(files, f => String.ends(f, ".sau")) then
    println("has Saule sources")           -- has Saule sources
end
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="iter-all">
<summary><code class="api-sig"><span class="sig-fn">Iter.all</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">t</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">,</span> <span class="sig-param">pred</span><span class="sig-punct">:</span> <span class="sig-kw">fn</span><span class="sig-punct">(</span><span class="sig-type">V</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">boolean</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">boolean</span></code></summary>


Whether **every** element matches. `true` for an empty table — there is no
element that fails.

```saule
println(Iter.all({1, 2}, n => n > 0))      -- true
println(Iter.all({1, 2}, n => n > 1))      -- false
println(Iter.all({}, n => false))          -- true, vacuously

-- Validating every entry before acting on any.
local ages: table<integer> = {36, 45}
println(Iter.all(ages, a => a >= 18))      -- true
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="iter-count">
<summary><code class="api-sig"><span class="sig-fn">Iter.count</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">t</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">,</span> <span class="sig-param">pred</span><span class="sig-punct">:</span> <span class="sig-kw">fn</span><span class="sig-punct">(</span><span class="sig-type">V</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">boolean</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">integer</span></code></summary>


How many elements match.

```saule
println(Iter.count({1, 2, 3}, n => n % 2 == 1))   -- 2
println(Iter.count({}, n => true))                -- 0

-- Cheaper than building the filtered table when only the number is wanted.
local scores: table<integer> = {84, 17, 96, 42}
println(Iter.count(scores, s => s >= 50))         -- 2
```


</details>
</div>

### Slicing

<div class="api-list">
<details class="api-entry" id="iter-take">
<summary><code class="api-sig"><span class="sig-fn">Iter.take</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">t</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">,</span> <span class="sig-param">n</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span></code></summary>


The first `n` elements, or all of them when there are fewer. Never an error.

```saule
println(Table.concat(Iter.take({1, 2, 3}, 2), ","))    -- 1,2
println(Table.concat(Iter.take({1, 2}, 9), ","))       -- 1,2, all of them
println(#Iter.take({1, 2}, 0))                         -- 0

-- A top-three, after sorting.
local scores: table<integer> = {84, 17, 96, 42}
Table.sort(scores, (a, b) => a > b)
println(Table.concat(Iter.take(scores, 3), ","))       -- 96,84,42
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="iter-skip">
<summary><code class="api-sig"><span class="sig-fn">Iter.skip</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">t</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">,</span> <span class="sig-param">n</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span></code></summary>


Everything after the first `n` elements. Empty when `n` is at or past the
length.

```saule
println(Table.concat(Iter.skip({1, 2, 3}, 2), ","))   -- 3
println(#Iter.skip({1, 2}, 9))                        -- 0

-- Dropping a header row.
local rows: table<string> = {"name,age", "ada,36"}
println(Table.concat(Iter.skip(rows, 1), ""))         -- ada,36
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="iter-first">
<summary><code class="api-sig"><span class="sig-fn">Iter.first</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">t</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">V</span><span class="sig-punct">?</span></code></summary>


The first element, or `nil` when the table is empty.

```saule
println(Iter.first({1, 2}) ?? 0)      -- 1
println(Iter.first({}) == nil)        -- true

-- Safer than `t[1]` when the table might be empty, because the nullability
-- is in the type rather than in your head.
local queue: table<string> = {}
println(Iter.first(queue) ?? "<empty>")   -- <empty>
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="iter-last">
<summary><code class="api-sig"><span class="sig-fn">Iter.last</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">t</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">V</span><span class="sig-punct">?</span></code></summary>


The last element, or `nil` when the table is empty.

```saule
println(Iter.last({1, 2}) ?? 0)       -- 2
println(Iter.last({}) == nil)         -- true

-- The most recent entry in a log.
local log: table<string> = {"start", "step", "done"}
println(Iter.last(log) ?? "")         -- done
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="iter-chunk">
<summary><code class="api-sig"><span class="sig-fn">Iter.chunk</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">t</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">,</span> <span class="sig-param">size</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">&gt;</span></code></summary>


Fixed-size groups, in order. The last group is short when the length does not
divide evenly.

```saule
local groups: table<table<integer>> = Iter.chunk({1, 2, 3, 4, 5}, 2)

println(#groups)                                  -- 3
println(Table.concat(groups[1] ?? {}, ","))       -- 1,2
println(Table.concat(groups[3] ?? {}, ","))       -- 5, the short one

-- Paging through a list.
for page in Iter.chunk({1, 2, 3, 4}, 2) do
    println("[" .. Table.concat(page, ",") .. "]")   -- [1,2], then [3,4]
end
```


</details>
</div>

### Shaping

<div class="api-list">
<details class="api-entry" id="iter-zipwith">
<summary><code class="api-sig"><span class="sig-fn">Iter.zipWith</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">,</span> <span class="sig-type">U</span><span class="sig-punct">,</span> <span class="sig-type">A</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">a</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">,</span> <span class="sig-param">b</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">U</span><span class="sig-punct">&gt;</span><span class="sig-punct">,</span> <span class="sig-param">f</span><span class="sig-punct">:</span> <span class="sig-kw">fn</span><span class="sig-punct">(</span><span class="sig-type">V</span><span class="sig-punct">,</span> <span class="sig-type">U</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">A</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">A</span><span class="sig-punct">&gt;</span></code></summary>


Pairs the two tables up and combines each pair with `f`, stopping at the
shorter one.

There is no plain `zip`, because a pair has no representation here: a
two-element table holding an `integer` and a `V` types as `table<any>`, which
loses both. Combining at the point the two elements meet keeps the types.

```saule
println(Table.concat(Iter.zipWith({1, 2}, {10, 20}, (a, b) => a + b), ","))   -- 11,22

-- Stops at the shorter one.
println(#Iter.zipWith({1, 2, 3}, {10}, (a, b) => a + b))                      -- 1

-- Combining two columns into labelled rows.
local names: table<string> = {"ada", "alan"}
local ages: table<integer> = {36, 41}
local rows: table<string> = Iter.zipWith(names, ages, (n, a) => n .. " (" .. a .. ")")
println(Table.concat(rows, ", "))        -- ada (36), alan (41)
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="iter-flatten">
<summary><code class="api-sig"><span class="sig-fn">Iter.flatten</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">t</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">&gt;</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span></code></summary>


Concatenates the inner tables into one. **One level only** — nest the call to
go deeper.

```saule
println(Table.concat(Iter.flatten({{1, 2}, {3}}), ","))   -- 1,2,3
println(#Iter.flatten({{}, {}}))                          -- 0

-- Pairs naturally with `chunk`, which is its inverse.
local chunked: table<table<integer>> = Iter.chunk({1, 2, 3, 4}, 2)
println(Table.concat(Iter.flatten(chunked), ","))         -- 1,2,3,4
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="iter-reverse">
<summary><code class="api-sig"><span class="sig-fn">Iter.reverse</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">t</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span></code></summary>


A **new** table in reverse order. [`Table.reverse`](/saule/stdlib/table/#table-reverse)
reverses in place instead.

```saule
local xs: table<integer> = {1, 2, 3}
println(Table.concat(Iter.reverse(xs), ","))   -- 3,2,1
println(Table.concat(xs, ","))                 -- 1,2,3, untouched
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="iter-unique">
<summary><code class="api-sig"><span class="sig-fn">Iter.unique</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">t</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span></code></summary>


Drops later duplicates, keeping the first occurrence and the original order.
Compares with `==`, so a class implementing [`OpEq`](/saule/stdlib/prelude-always-in-scope/#prelude-interfaces) is
deduplicated by its own rule.

```saule
println(Table.concat(Iter.unique({1, 2, 1, 3, 2}), ","))   -- 1,2,3
println(Table.concat(Iter.unique({"a", "a"}), ","))        -- a

-- Order is the order of first appearance, not sorted order.
println(Table.concat(Iter.unique({3, 1, 3}), ","))         -- 3,1
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="iter-sortby">
<summary><code class="api-sig"><span class="sig-fn">Iter.sortBy</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">,</span> <span class="sig-type">K</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">t</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">,</span> <span class="sig-param">key</span><span class="sig-punct">:</span> <span class="sig-kw">fn</span><span class="sig-punct">(</span><span class="sig-type">V</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">K</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span></code></summary>


A **new** table sorted ascending by the extracted key, using `<` — so a class
implementing [`OpCompare`](/saule/stdlib/prelude-always-in-scope/#prelude-interfaces) sorts by its own rule. Stable:
elements with equal keys keep their original order.

For a bespoke ordering, [`Table.sort`](/saule/stdlib/table/#table-sort) takes
a comparator directly.

```saule
local words: table<string> = {"pear", "fig", "banana"}
println(Table.concat(Iter.sortBy(words, w => String.len(w)), ","))   -- fig,pear,banana

-- Sorting by the element itself needs no annotation.
println(Table.concat(Iter.sortBy({3, 1, 2}, n => n), ","))                     -- 1,2,3

-- The original is untouched, unlike `Table.sort`.
println(Table.concat(words, ","))                                             -- pear,fig,banana
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="iter-groupby">
<summary><code class="api-sig"><span class="sig-fn">Iter.groupBy</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">,</span> <span class="sig-type">K</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">t</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">,</span> <span class="sig-param">key</span><span class="sig-punct">:</span> <span class="sig-kw">fn</span><span class="sig-punct">(</span><span class="sig-type">V</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">K</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">K</span><span class="sig-punct">,</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">&gt;</span></code></summary>


Buckets the elements by key. The key must be a `string`, `integer` or
`boolean` — the types a table can be keyed by.

```saule
local words: table<string> = {"fig", "pear", "kiwi", "a"}
local byLength: table<integer, table<string>> = Iter.groupBy(words, w => String.len(w))

println(Table.concat(byLength[3] ?? {}, ","))    -- fig
println(Table.concat(byLength[4] ?? {}, ","))    -- pear,kiwi
println(#(byLength[9] ?? {}))                    -- 0, no bucket

-- Grouping by a boolean splits a list in two.
local nums: table<integer> = {1, 2, 3, 4}
local parity: table<boolean, table<integer>> = Iter.groupBy(nums, n => n % 2 == 0)
println(Table.concat(parity[true] ?? {}, ","))   -- 2,4
println(Table.concat(parity[false] ?? {}, ","))  -- 1,3
```


</details>
</div>

### There is no `Iter.zip` or `Iter.enumerate`

Both would have to return pairs, and a pair has no representation here —
`zipWith` combines at the point the two elements meet instead, and a
`for i, v in t do` loop already gives indices alongside values.
