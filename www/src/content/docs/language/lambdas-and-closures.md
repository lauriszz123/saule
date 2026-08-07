---
title: "Lambdas and Closures"
description: "Single expression, most common form:"
sidebar:
  order: 5
---

<!-- Generated from README.md by `npm run sync-docs`. Edit that file, not this one. -->

### Arrow Lambda

Single expression, most common form:

```saule
local double: fn(integer) -> integer = x => x * 2
local square: fn(integer) -> integer = x => x * x
local greet: fn(string) -> nil       = name => print("Hi " .. name)
local half: fn(float) -> float       = x => x / 2.0
```

A single parameter needs no parentheses. Wrap them when there are none, when
there are several, or when a parameter carries a type — and an explicit return
type may follow the list:

```saule
local now: fn() -> integer                  = () => 0
local add: fn(integer, integer) -> integer  = (a, b) => a + b
local scale: fn(float) -> float             = (x: float) -> float => x * 2.0
local pack: fn(integer) -> table<integer>   = (n: integer) -> table<integer> => {n}
```

### Full Lambda

Multi-line anonymous function:

```saule
local double: fn(integer) -> integer = fn(x)
    return x * 2
end
```

### Parameter Types Are Inferred

Lambda parameters may omit `: T`. The type comes from the function type the
lambda is being assigned to, so `x` below is a real `integer` inside the body
— using it as anything else is a compile error, not a silent `any`:

```saule
local double: fn(integer) -> integer = fn(x)
    return #x            -- ERROR: cannot take length of an `integer`
end
```

This works in all three lambda forms, and an explicit annotation still wins:

```saule
local add: fn(integer, integer) -> integer = (a, b) => a + b
local negate: fn(integer) -> integer       = x => -x
local shout: fn(string) -> string          = fn(s: string) return s .. "!" end
```

Top-level `fn` declarations are different — their parameters **must** be
typed. A declaration's signature is the contract its callers read, and there
is no target to infer it from.

### Functions as Parameters

```saule
fn map(items: table<integer>, transform: fn(integer) -> integer) -> table<integer>
    local result: table<integer> = {}
    for i: integer, val: integer in items do
        result[i] = transform(val)
    end
    return result
end

local doubled: table<integer> = map(nums, x => x * 2)
```

### Functions as Return Values

```saule
fn multiplier(factor: integer) -> fn(integer) -> integer
    return x => x * factor
end

local triple: fn(integer) -> integer = multiplier(3)
print(triple(5))    -- 15
```

### Closures

Lambdas capture their surrounding scope:

```saule
fn makeCounter(start: integer = 0) -> fn() -> integer
    local count: integer = start

    return fn()
        count = count + 1
        return count
    end
end

local counter: fn() -> integer = makeCounter()
print(counter())    -- 1
print(counter())    -- 2
print(counter())    -- 3
```

### Trailing Blocks

When the last argument to a call is a function, it can be written after the
closing parenthesis as a `do ... end` block:

```saule
fn view(spacing: integer, body: fn() -> nil) -> nil
    Layout.push(spacing)
    body()
    Layout.pop()
end

view(spacing: 10) do
    text("Hello")
    button(label: "OK")
end
```

That is sugar, nothing more — it produces the same call as writing the lambda
out in full:

```saule
view(spacing: 10, fn()
    text("Hello")
    button(label: "OK")
end)
```

Braces stay reserved for tables. A trailing block is code, so it is delimited
the way every other block in Saule is: a keyword opens it and `end` closes it.
`do` is the same keyword that introduces a loop body, and it means the same
thing here — a block of statements follows.

#### Parameters

A trailing block takes parameters and a return type in parentheses after `do`,
with the same rules as any other lambda — types are optional and inferred from
the callee's signature:

```saule
fn mapEach(items: table<integer>, transform: fn(integer) -> integer) -> table<integer>
    local out: table<integer> = {}
    for i, v in items do
        out[i] = transform(v)
    end
    return out
end

local doubled: table<integer> = mapEach({1, 2, 3}) do (n) -> integer
    return n * 2
end
```

`n` is a real `integer` inside the block, not an `any` — using it as anything
else is a compile error.

#### Which Parameter It Fills

A trailing block binds to the callee's **last function-typed parameter that no
other argument claimed**, wherever the arguments before it landed. This is also
what lets it follow named arguments, even though a normal positional argument
cannot:

```saule
view(spacing: 10) do    -- `spacing` is named, so the block fills `body`
    text("Hello")
end
```

Binding to the callback slot — rather than to the next unfilled one — is what
makes the form work with defaults in between:

```saule
fn panel(title: string, spacing: integer = 0, body: fn() -> nil) -> nil
    body()
end

panel(title: "Stats") do    -- block fills `body`; `spacing` takes its default
    text("Hello")
end

panel("Stats", 2) do        -- block still fills `body`; `spacing` is 2
    text("Hello")
end
```

The callback does not have to come last. A parameter that cannot hold a
function is skipped over, so a trailing `enabled: boolean` doesn't get in the
way:

```saule
fn menuItem(label: string, onSelected: fn() -> nil, enabled: boolean = true) -> nil
    onSelected()
end

menuItem("Open") do         -- block fills `onSelected`; `enabled` defaults
    showToast("Open")
end
```

Only *free* slots are candidates. If something else already filled the callback
parameter, the block falls through to the last parameter and you get the type
error you asked for rather than a silently misplaced argument — which is also
how supplying the callback *and* a trailing block reads as a duplicate-argument
error:

```saule
menuItem("Open", () => nil) do    -- error: a function where `enabled` wants a boolean
    showToast("Open")
end
```

#### Blocks and Loop Headers

`while` and `for` end their header with `do`, so a `do` there always belongs to
the loop, never to a call in the condition:

```saule
while queue.pop() do    -- the loop's `do`, not a trailing block on `pop()`
    ...
end
```

Parenthesise the call if you really want a trailing block inside a loop header:

```saule
while (frame() do return true end) do
    ...
end
```

Only a call can carry a trailing block. `view do ... end` is an error — write
`view() do ... end`.

#### Why Blocks and Not Tables

A trailing block holds statements, so ordinary control flow works inside it.
That is the practical reason to reach for one instead of passing a table of
values:

```saule
panel(title: "Stats") do
    for _, p in players do
        row(p.name)
    end
    if debug then
        text("fps: " .. fps)
    end
end
```

---
