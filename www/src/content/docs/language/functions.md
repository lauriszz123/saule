---
title: "Functions"
description: "Functions are declared with fn, take typed parameters, and state what they return. Parameters may carry defaults, be passed by name, or be variadic."
sidebar:
  order: 4
---

<!-- Generated from README.md by `npm run sync-docs`. Edit that file, not this one. -->

Functions are declared with `fn`, take typed parameters, and state what they return. Parameters may carry defaults, be passed by name, or be variadic.

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
class Player
    name: string = ""
    health: integer = 0
    score: integer = 0

    fn init(name: string, health: integer, score: integer)
        self.name = name
        self.health = health
        self.score = score
    end
end

fn createPlayer(name: string, health: integer = 100, score: integer = 0) -> Player
    return Player(name, health, score)
end

local a: Player = createPlayer("Arthur")         -- health=100, score=0
local b: Player = createPlayer("Arthur", 50)     -- health=50

println(a.health)                                -- 100
println(b.health)                                -- 50, score=0
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

Inside the body a type parameter is **rigid**: `T` stands for whatever the caller picked, so it matches only itself. Widening into `any` is free, but narrowing to a concrete type is a downcast and goes through the checked `as` — the same escape `any` uses:

```saule
fn onlyInts<T>(items: table<T>) -> table<integer>
    local result: table<integer> = {}

    for item: T in items do
        local n: integer = item          -- rejected: a `T` is not an `integer`
        local n: integer? = item as integer   -- checked at runtime, may be nil

        if n != nil then
            result[#result + 1] = n!
        end
    end

    return result
end
```

Two type parameters are independent for the same reason: nothing proves a `T` is a `U`. That holds across functions too — a `T` declared by the function you are *calling* is not the `T` declared by the one you are writing, however alike they read.

Classes, interfaces and enums take type parameters the same way — see [Generic Classes](/saule/language/classes/#generic-classes), [Generic Interfaces](/saule/language/interfaces/#generic-interfaces) and [Generic Enums](/saule/language/enums/#generic-enums).

### Bounds

A bare `<T>` says only "some type the caller picks", so there is nothing a body can *do* with a `T` but move it around. A **bound** says what the parameter has:

```saule
interface Named
    fn name() -> string
end

fn describe<T: Named>(x: T) -> string
    return "<" .. x.name() .. ">"      -- `Named` promises `name`
end
```

The bound is checked in both directions, which is the whole point of writing one:

- **Inside the body**, a `T` has exactly the members its bound declares — `x.name()` above resolves against `Named`, and a typo or a method the interface does not declare is a compile error.
- **At every call site**, the type filling `T` has to satisfy the bound. `describe(5)` is rejected because `integer` has no `name`.

Without a bound, neither half is possible, so reading a member off an unconstrained parameter is refused outright:

```saule
fn show<T>(x: T) -> string
    return x.name()    -- ERROR: no member `name` on the unbounded type parameter `T`
end
```

A bound names an **interface** the type must implement or a **class** it must be, with its own type arguments where it has them. Operator interfaces are the ones that pay off most, because they are what makes arithmetic on a type parameter possible at all:

```saule
fn total<T: OpAdd<T, T>>(a: T, b: T) -> T
    return a + b                        -- `OpAdd<T, R>` puts the result last
end

fn larger<T: OpCompare<T>>(a: T, b: T) -> T
    if a > b then
        return a
    end
    return b
end
```

Classes, interfaces and enums take bounds on their own parameters too, and a method's bound nests inside its class's:

```saule
interface Named
    fn name() -> string
end

class Person implements Named
    local who: string
    fn init(who: string)
        self.who = who
    end
    fn name() -> string
        return self.who
    end
end

class Registry<T: Named>
    items: table<T> = {}

    fn names() -> string
        return String.join(", ", Iter.map(self.items, i => i.name()))
    end
end

local people: Registry<Person> = Registry<Person>()   -- ok: `Person` is `Named`
local wrong: Registry<integer> = Registry<integer>()  -- ERROR: `integer` does not satisfy `Named`
```

Two rules worth knowing. A parameter may be handed to another bounded parameter when its own bound implies the target's — all the caller knows about a `T` is its bound, so that is what has to be enough. And a bound is still *rigid*: `<T: Named>` does not make a `T` interchangeable with `Named`, it makes a `T` usable as one.

### Piping with `when(...):`

The `when(...)` keyword starts a **colon-based pipeline** ("Saule style"). It wraps a value, and every subsequent `:func(args)` calls `func` with the upstream value threaded in as the **first argument**:

```saule
fn pipe(s: string) -> string
    return s .. "world"
end

local result: string = when("Hello, "):pipe()
-- equivalent to:  pipe("Hello, ")
println(result)                 -- Hello, world
```

Each stage feeds its result into the next, so a chain reads top-to-bottom even though every step is an ordinary free-function call:

```saule
fn filter<T>(items: table<T>, keep: fn(T) -> boolean) -> table<T>
    return Iter.filter(items, keep)
end

fn map<T>(items: table<T>, f: fn(T) -> T) -> table<T>
    return Iter.map(items, f)
end

fn length_of<T>(items: table<T>) -> integer
    return #items
end

local size: integer = when({1, 2, 3, 4, 5, 6, 7, 8, 9, 10})
    :filter<integer>(x => x % 2 == 0)
    :map(x => x * x)
    :length_of()

println(size)                   -- 5
```

That's exactly the same as `length_of(map(filter<integer>({...}, x => x % 2 == 0), x => x * x))` — just easier to read.

#### Static type checking along the chain

The type of the upstream value must match the **first parameter** of the next stage, otherwise the typechecker rejects the chain at compile time:

```saule
fn to_str(n: integer) -> string
    return tostring(n)
end

fn square(n: integer) -> integer
    return n * n
end

local err = when(5):to_str():square()   -- ERROR: stage `square` expects `integer`, got `string`
```

Rules:
- The chain needs **at least one** `:stage()` after `when(...)` — a bare `when(x)` is a parse error so the keyword's purpose stays unambiguous.
- Stage targets are resolved as **free functions** in scope (locals, module variables, top-level `fn`). Class methods and lambdas aren't pipeable today.
- The piped value always becomes argument `#1`; declared defaults and the variadic tail still apply to the remaining parameters as usual.

---
