---
title: "Coroutines"
description: "A coroutine is a function that can pause in the middle — *yield* a value — and later carry on from exactly that point, with every local and its whole…"
sidebar:
  order: 13
---

<!-- Generated from LANGUAGE.md by `npm run sync-docs`. Edit that file, not this one. -->

A **coroutine** is a function that can pause in the middle — *yield* a value —
and later carry on from exactly that point, with every local and its whole call
stack still in place. Where an ordinary function runs start to finish and
returns once, a coroutine hands back a value every time it yields, as many
times as it likes.

That makes coroutines the natural shape for anything that produces a sequence
over time: a generator, a tokenizer that hands out one token at a time, a task
in a game loop that runs a little each frame.

```saule
local gen: Coroutine<integer> = Coroutine.create(fn(co)
    co.yield(1)
    co.yield(2)
    co.yield(3)
end)

println(gen.resume())   -- 1
println(gen.resume())   -- 2
println(gen.resume())   -- 3
println(gen.resume())   -- nil: the body has finished
```

Nothing in the body runs until the first `resume`. Each `resume` runs it until
the next `yield`, and the value given to `yield` is what that `resume` returns.

### Two Handles: `Coroutine<T>` and `Yielder<T>`

A coroutine has two sides, and each gets its own handle:

| Handle | Held by | What it can do |
|---|---|---|
| `Coroutine<T>` | Whoever created it | `resume()`, `status()`, `isDone()`, `error()`, and `for … in` |
| `Yielder<T>` | The body — its `co` parameter | `yield(value)` |

`T` is the type of the values that come out. The checker holds both sides to
it: yielding anything else is a compile error, and `resume()` on a
`Coroutine<integer>` answers an `integer?` — `nil` once the coroutine is done.

```saule
local names: Coroutine<string> = Coroutine.create(fn(co)
    co.yield("ada")
    co.yield(42)        -- ERROR: argument 1 of `Yielder.yield` expects `string`, got `integer`
end)

names.yield("x")        -- ERROR: no member `yield` on `Coroutine`
```

Splitting the handles is what makes `yield` typed. In Lua, `coroutine.yield`
suspends *whichever* coroutine happens to be running, so nothing can say what
type it should take. A `Yielder<T>` belongs to one coroutine, so it carries that
coroutine's `T`. It also keeps each side to the operations that make sense for
it: the body cannot resume itself, and outside code cannot yield it.

### Where `T` Comes From

You rarely write `T` twice. The checker learns it from where the coroutine
goes, so the body's `co` needs no annotation:

```saule
-- From the local's declared type.
local gen: Coroutine<integer> = Coroutine.create(fn(co)
    co.yield(1)
end)

-- From a function's declared return type — the generator-factory shape.
fn range(n: integer) -> Coroutine<integer>
    return Coroutine.create(fn(co)
        for i = 1, n do
            co.yield(i)
        end
    end)
end

-- From the parameter the call fills.
fn total(g: Coroutine<integer>) -> integer
    local sum: integer = 0
    for v in g do
        sum = sum + v
    end
    return sum
end

println(total(Coroutine.create(fn(co)
    co.yield(20)
    co.yield(22)
end)))                  -- 42
```

With nowhere to learn it from, annotate the yielder instead:
`local gen = Coroutine.create(fn(co: Yielder<string>) … end)` is a
`Coroutine<string>`. If the annotation and the destination disagree, the
checker reports it rather than picking one.

### Iterating

A coroutine is an `Iterable<T>`, so `for … in` resumes it until it finishes,
and the loop variable is a `T`:

```saule
for n in range(3) do
    println(n * 10)     -- 10, 20, 30 — `n` is an integer
end
```

`break` leaves the coroutine suspended where it was, so a later `resume` or
another loop picks up after the last value the loop saw.

### Stackful: Yield from Anywhere

The yielder is an ordinary value. Pass it into any function, as deep as you
like, and that function can yield on the coroutine's behalf — the whole call
stack in between is suspended with it:

```saule
class Tree
    local value: integer
    local left: Tree?
    local right: Tree?

    fn init(value: integer, left: Tree?, right: Tree?)
        self.value = value
        self.left = left
        self.right = right
    end

    -- In-order traversal that yields as it goes.
    fn walk(co: Yielder<integer>)
        self.left?.walk(co)
        co.yield(self.value)
        self.right?.walk(co)
    end
end

local root: Tree = Tree(2, Tree(1, nil, nil), Tree(3, nil, nil))
local values: Coroutine<integer> = Coroutine.create(fn(co)
    root.walk(co)
end)

for v in values do
    println(v)          -- 1, 2, 3
end
```

Writing the same traversal without a coroutine means keeping an explicit stack
of nodes by hand. Here the recursion *is* the stack.

### Lifecycle and Status

`status()` says where a coroutine is:

| Status | Meaning |
|---|---|
| `"suspended"` | Created but not started, or stopped at a `yield` |
| `"running"` | Its code is running right now — only visible from inside it |
| `"normal"` | It resumed another coroutine and is waiting for that one to yield |
| `"dead"` | Its body finished, or failed |

`isDone()` is `true` once it is dead. Resuming a dead coroutine is not an
error — it simply answers `nil` again, which is what lets a loop drain a
generator without checking first.

Coroutines can resume other coroutines, and do so freely: a pipeline of
generators each pulling from the one before is just coroutines calling
`resume` on each other.

```saule
fn evens(src: Coroutine<integer>) -> Coroutine<integer>
    return Coroutine.create(fn(co)
        for v in src do
            if v % 2 == 0 then
                co.yield(v)
            end
        end
    end)
end

for v in evens(range(10)) do
    println(v)          -- 2, 4, 6, 8, 10
end
```

### When a Coroutine Fails

A runtime error or an uncaught `throw` inside a coroutine does **not** end the
program. The coroutine becomes dead, `resume()` answers `nil`, and `error()`
says what went wrong — the thrown value for a `throw`, the message for anything
else:

```saule
local job: Coroutine<integer> = Coroutine.create(fn(co)
    co.yield(1)
    throw "disk full"
end)

println(job.resume())            -- 1
println(job.resume())            -- nil
println(job.status())            -- dead
println(job.error() ?? "")       -- disk full
```

A `throw` the coroutine catches itself is ordinary control flow and does not
fail it — a `yield` inside `try` resumes inside the same `try`.

`for … in` is the exception. A loop has no return value to report a failure
in, and quietly ending would make a generator that crashed look like one that
finished — so a coroutine that fails mid-loop **raises** out of the loop.

### The Rules

Coroutines run on one thread: resuming one *transfers control* to it, and
nothing ever runs in parallel. Within that, three things are not allowed, and
each is reported:

- **A coroutine cannot resume itself**, nor one that is waiting on a coroutine
  it resumed. Both are already running.
- **A coroutine cannot yield from inside a callback that a built-in makes** —
  a `Table.sort` comparator, an operator overload, a `toString`. The built-in's
  own call sits between the callback and the coroutine and cannot be suspended.
  The coroutine fails, and `error()` explains. Resuming *another* coroutine from
  inside a callback is fine.
- **A yielder only yields its own coroutine, while that coroutine is running.**
  One that escapes the body and is used later reports that it is not the one
  running.

### Dynamic Coroutines

The typed contract is `fn(Yielder<T>) -> nil`: values go out through `yield`,
and `resume` takes no arguments. Lua's coroutines go further — `resume(x)`
becomes the result of the `yield` it resumes, the first `resume` passes
arguments to the body, and the body's return values reach the last `resume`.
Saule's runtime does all of that. A body held as `any` is how to ask for it,
and like everything `any`, it is checked when it runs rather than before:

```saule
local body: any = fn(co: any, start: integer) -> string
    local got: any = co.yield(start)
    return "got " .. tostring(got)
end

local gen: any = Coroutine.create(body)
println(gen.resume(10))       -- 10
println(gen.resume("back"))   -- got back
```

A worked tour of all of this — lazy stream combinators that are themselves
coroutines, a tokenizer that yields from inside the functions it calls, a
parser that pulls one token at a time, and a cooperative scheduler whose tasks
interleave on one thread — is in
[`examples/coroutines`](/saule/examples/coroutines/).

See the [`Coroutine` reference](/saule/stdlib/coroutine/) for every method.

---
