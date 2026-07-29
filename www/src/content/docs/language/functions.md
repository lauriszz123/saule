---
title: "Functions"
description: "The when(...) keyword starts a colon-based pipeline (\"Saule style\"). It wraps a value, and every subsequent :func(args) calls func with the upstream…"
sidebar:
  order: 4
---

<!-- Generated from README.md by `npm run sync-docs`. Edit that file, not this one. -->

### Basic Functions

```saule
fn add(a: integer, b: integer) -> integer
    return a + b
end

fn average(a: float, b: float) -> float
    return (a + b) / 2.0
end

fn greet(name: string) -> nil
    print("Hello, " .. name)
end
```

### Default Parameters

```saule
fn createPlayer(name: string, health: integer = 100, score: integer = 0) -> Player
    return Player(name, health, score)
end

local p: Player = createPlayer("Arthur")         -- health=100, score=0
local p: Player = createPlayer("Arthur", 50)     -- health=50, score=0
```

### Named Parameters

```saule
fn setupGame(width: integer, height: integer, title: string, fullscreen: boolean = false) -> nil
    -- ...
end

setupGame(width: 1920, height: 1080, title: "My Game", fullscreen: true)
```

### Multiple Return Values

```saule
fn minMax(items: table<integer>) -> (integer, integer)
    local min: integer = items[1]
    local max: integer = items[1]
    for val: integer in items do
        if val < min then min = val end
        if val > max then max = val end
    end
    return min, max
end

local min: integer, max: integer = minMax({3, 1, 7, 2, 9})
```

### Variadic Functions

```saule
fn sum(...values: integer) -> integer
    local total: integer = 0

    for v: integer in values do
        total = total + v
    end

    return total
end

print(sum(1, 2, 3, 4, 5))    -- 15
```

### Generic Functions

```saule
fn filter<T>(items: table<T>, predicate: fn(T) -> boolean) -> table<T>
    local result: table<T> = {}

    for item: T in items do
        if predicate(item) then
            result[#result + 1] = item
        end
    end

    return result
end

local nums: table<integer> = {1, 2, 3, 4, 5, 6}
local evens: table<integer> = filter<integer>(nums, x => x % 2 == 0)
```

### Piping with `when(...):`

The `when(...)` keyword starts a **colon-based pipeline** ("Saule style"). It wraps a value, and every subsequent `:func(args)` calls `func` with the upstream value threaded in as the **first argument**:

```saule
local result: string = when("Hello, "):pipe()
-- equivalent to:  pipe("Hello, ")
```

Each stage feeds its result into the next, so a chain reads top-to-bottom even though every step is an ordinary free-function call:

```saule
local size: integer = when({1, 2, 3, 4, 5, 6, 7, 8, 9, 10})
    :filter<integer>(x => x % 2 == 0)
    :map(x => x * x)
    :length_of()
```

That's exactly the same as `length_of(map(filter<integer>({...}, x => x % 2 == 0), x => x * x))` — just easier to read.

#### Static type checking along the chain

The type of the upstream value must match the **first parameter** of the next stage, otherwise the typechecker rejects the chain at compile time:

```saule
fn to_str(n: integer) -> string ... end
fn square(n: integer) -> integer ... end

local err = when(5):to_str():square()
-- COMPILE ERROR: pipeline stage `square` expects `integer` as first
--                argument, got `string`
```

Rules:
- The chain needs **at least one** `:stage()` after `when(...)` — a bare `when(x)` is a parse error so the keyword's purpose stays unambiguous.
- Stage targets are resolved as **free functions** in scope (locals, globals, top-level `fn`). Class methods and lambdas aren't pipeable today.
- The piped value always becomes argument `#1`; declared defaults and the variadic tail still apply to the remaining parameters as usual.

---
