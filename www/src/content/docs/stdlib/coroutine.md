---
title: "Coroutine"
description: "A coroutine is a function that can stop part-way — yield a value — and later be resumed exactly where it stopped, with its locals and its whole call…"
sidebar:
  order: 7
---

<!-- Generated from DOCS.md by `npm run sync-docs`. Edit that file, not this one. -->

A coroutine is a function that can stop part-way — **yield** a value — and
later be **resumed** exactly where it stopped, with its locals and its whole
call stack intact. `Coroutine<T>` is the handle you resume; the body is handed
a `Yielder<T>`, the one thing that can yield it. The language guide's
[Coroutines](/saule/language/coroutines/) section explains the model; this is the
reference.

```saule
-- A generator: the declared return type tells the body what it yields.
fn countdown(start: integer) -> Coroutine<integer>
    return Coroutine.create(fn(co)
        for i = start, 1, -1 do
            co.yield(i)
        end
    end)
end

for n in countdown(3) do
    println(n)                 -- 3, then 2, then 1
end
```

Both types are generic. `T` is learned from wherever the coroutine goes — a
typed `local`, a declared return type, a parameter — or from an annotation on
the body's yielder, `fn(co: Yielder<string>)`. Coroutines are *stackful*:
pass the yielder to any function and that function can yield too.

### Creating

<div class="api-list">
<details class="api-entry" id="coroutine-create">
<summary><code class="api-sig"><span class="sig-fn">Coroutine.create</span><span class="sig-punct">&lt;</span><span class="sig-type">T</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">body</span><span class="sig-punct">:</span> <span class="sig-kw">fn</span><span class="sig-punct">(</span><span class="sig-type">Yielder</span><span class="sig-punct">&lt;</span><span class="sig-type">T</span><span class="sig-punct">&gt;</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-kw">nil</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">Coroutine</span><span class="sig-punct">&lt;</span><span class="sig-type">T</span><span class="sig-punct">&gt;</span></code></summary>


A new coroutine that has not started. Nothing in `body` runs until the first
`resume`, which calls it with the coroutine's yielder.

```saule
local ran: boolean = false
local gen: Coroutine<string> = Coroutine.create(fn(co)
    ran = true
    co.yield("first")
end)

println(ran)                   -- false: creating runs nothing
println(gen.resume() ?? "")    -- first
println(ran)                   -- true
```


</details>
</div>

### `Coroutine` methods

<div class="api-list">
<details class="api-entry" id="gen-resume">
<summary><code class="api-sig"><span class="sig-fn">gen.resume</span><span class="sig-punct">(</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">T</span><span class="sig-punct">?</span></code></summary>


Run the coroutine until it next yields, and answer the value it yielded. When
the body finishes — or has already — it answers `nil`, and so it does when
the body **fails**: a runtime error or an uncaught `throw` inside a coroutine
does not end the program. The coroutine is dead afterwards and
[`error()`](/saule/stdlib/coroutine/#gen-error) says what went wrong.

Resuming a coroutine that is already running — from inside itself, or one
waiting on a coroutine it resumed — is a runtime error.

```saule
local gen: Coroutine<integer> = Coroutine.create(fn(co)
    co.yield(10)
    co.yield(20)
end)

println(gen.resume() ?? 0)     -- 10
println(gen.resume() ?? 0)     -- 20
println(gen.resume() == nil)   -- true: finished
println(gen.resume() == nil)   -- true: still finished
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="gen-status">
<summary><code class="api-sig"><span class="sig-fn">gen.status</span><span class="sig-punct">(</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">string</span></code></summary>


Where the coroutine is in its life:

| Status | Meaning |
| --- | --- |
| `"suspended"` | Created and not yet started, or stopped at a `yield` |
| `"running"` | Its code is running now — only seen from inside it |
| `"normal"` | It resumed another coroutine and is waiting on it |
| `"dead"` | Its body finished, or failed |

```saule
local gen: Coroutine<integer> = Coroutine.create(fn(co)
    co.yield(1)
end)

println(gen.status())          -- suspended
gen.resume()
println(gen.status())          -- suspended, at the yield
gen.resume()
println(gen.status())          -- dead
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="gen-isdone">
<summary><code class="api-sig"><span class="sig-fn">gen.isDone</span><span class="sig-punct">(</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">boolean</span></code></summary>


`true` once the coroutine is dead — finished or failed. Shorthand for
`gen.status() == "dead"`, and the natural test for a loop that drives several
coroutines at once.

```saule
local gen: Coroutine<integer> = Coroutine.create(fn(co)
    co.yield(1)
end)

while not gen.isDone() do
    gen.resume()
end
println(gen.status())          -- dead
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="gen-error">
<summary><code class="api-sig"><span class="sig-fn">gen.error</span><span class="sig-punct">(</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">string</span><span class="sig-punct">?</span></code></summary>


Why the coroutine failed, or `nil` if it has not. For an uncaught `throw` it
is the thrown value; for any other failure, the runtime error's message.

```saule
local gen: Coroutine<integer> = Coroutine.create(fn(co)
    co.yield(1)
    throw "out of data"
end)

gen.resume()                   -- 1
println(gen.resume() == nil)   -- true: it failed
println(gen.status())          -- dead
println(gen.error() ?? "")     -- out of data
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="gen-iter">
<summary><code class="api-sig"><span class="sig-fn">gen.iter</span><span class="sig-punct">(</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-kw">fn</span><span class="sig-punct">(</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">T</span><span class="sig-punct">?</span></code></summary>


A step closure that resumes the coroutine once per call — what
`for v in gen do` uses, so you rarely call it yourself. A coroutine is an
`Iterable<T>`, and the loop variable is a `T`.

Unlike `resume`, iterating **raises** if the coroutine fails part-way: a loop
has no return value to report a failure in, and ending quietly would make a
generator that crashed look like one that finished. Breaking out of the loop
leaves the coroutine suspended where it was.

```saule
local gen: Coroutine<integer> = Coroutine.create(fn(co)
    for i = 1, 5 do
        co.yield(i * i)
    end
end)

for sq in gen do
    if sq > 10 then
        break                  -- gen stays suspended
    end
    println(sq)                -- 1, 4, 9
end
println(gen.resume() ?? 0)     -- 25: picks up after the break
```


</details>
</div>

### `Yielder` methods

<div class="api-list">
<details class="api-entry" id="co-yield">
<summary><code class="api-sig"><span class="sig-fn">co.yield</span><span class="sig-punct">(</span><span class="sig-param">value</span><span class="sig-punct">:</span> <span class="sig-type">T</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-kw">nil</span></code></summary>


Suspend the coroutine this yielder belongs to and hand `value` to whoever
resumed it. The call returns when the coroutine is next resumed.

Only the coroutine's own code can yield it. Two places cannot, and saying
so is a runtime error:

- **inside a callback a built-in makes** — a `Table.sort` comparator, an
  operator overload, a `toString`. The built-in's own call is in the way and
  cannot be suspended. The coroutine fails, and `error()` says why;
- **outside the coroutine** — a yielder that escaped its body and is used
  while some other coroutine, or none, is running.

```saule
-- A yielder passes down like any value, so any function can yield.
fn walk(co: Yielder<integer>, depth: integer)
    if depth == 0 then
        return
    end
    co.yield(depth)
    walk(co, depth - 1)
end

local gen: Coroutine<integer> = Coroutine.create(fn(co)
    walk(co, 3)
end)

for d in gen do
    println(d)                 -- 3, 2, 1
end
```


</details>
</div>

### Dynamic coroutines

The typed contract is `fn(Yielder<T>) -> nil`: values go *out* through
`yield`, and `resume` takes nothing. Lua's coroutines also pass values *in* —
`resume(x)` becomes the result of the `yield` it resumes — and give the body's
return value to the last `resume`. Saule's runtime does all of that; a body
held as `any` is how to reach it, and everything about it is checked at run
time instead.

```saule
local body: any = fn(co: any, start: integer) -> string
    local got: any = co.yield(start)
    return "got " .. tostring(got)
end

local gen: any = Coroutine.create(body)
println(gen.resume(10))        -- 10: the first resume's arguments reach the body
println(gen.resume("back"))    -- got back
```

---
