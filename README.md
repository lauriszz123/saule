# Saule Programming Language

Saule is a statically typed, class-oriented language inspired by Lua's simplicity and runtime model, designed to be minimal to write but powerful to use. It is structured around files, classes, interfaces, and scripts.

> 📚 Looking for the **standard library**? See **[DOCS.md](./DOCS.md)**.

---

## Table of Contents

- [Types](#types)
- [Variables](#variables)
- [Tables](#tables)
- [Functions](#functions)
- [Lambdas and Closures](#lambdas-and-closures)
- [Classes](#classes)
- [Interfaces](#interfaces)
- [Enums](#enums)
- [Pattern Matching](#pattern-matching)
- [Null Safety](#null-safety)
- [Error Handling](#error-handling)
- [Loops](#loops)
- [Imports and File Structure](#imports-and-file-structure)
- [Project Configuration](#project-configuration)
- [Standard Library →](./DOCS.md)

---

## Types

Saule has 9 primitive types, inherited from Lua's type system but statically declared. Lua's single `number` type is split into two distinct types in Saule:

| Type | Description |
|---|---|
| `integer` | Whole numbers, no decimal component |
| `float` | Decimal numbers, 64-bit precision |
| `string` | Immutable sequences of characters |
| `boolean` | `true` or `false` |
| `nil` | Absence of value |
| `function` | First-class function values |
| `table<T>` | The only data structure, typed generically |
| `any` | A value of unknown type. Anything may be assigned **to** an `any`; getting a value back **out** requires a checked [`as` cast](#escaping-any-with-as) |
| `userdata` | Raw memory for native integrations |
| `thread` | Coroutines |

All types must be declared. A variable cannot be `nil` unless its type is marked nullable with `?`.

### Integer vs Float

Use `integer` for whole values like counts, indices, and health. Use `float` for precision values like position, speed, and ratios:

```saule
local health: integer = 100
local speed: float = 3.14
local index: integer = 1
local ratio: float = 0.75
```

Mixing `integer` and `float` directly is a **compile error**:

```saule
local health: integer = 100
local dmg: float = 10.5
local result = health - dmg    -- ERROR: cannot mix integer and float
```

Saule never auto-promotes — the checker catches this at compile time, so a hidden `int / int` truncating into a `float` slot is impossible.

### Base Prefixes

Integers can be written in hex or binary, with `_` allowed anywhere as a digit
separator:

```saule
local mask: integer = 0xFF        -- 255
local flags: integer = 0b1010     -- 10
local colour: integer = 0xFF_80_00
local glyph: integer = 0xE5CD     -- a font codepoint
```

Both forms produce ordinary `integer` values — there is no separate type. A
prefix with no digits (`0x`) or an invalid digit for the base (`0xGG`, `0b102`)
is a lex error.

### Integer Division

`/` on two integers is **integer division** (Lua / C semantics) — the result is the truncated quotient, never a float:

```saule
local q: integer = 7 / 2     -- 3 (truncated, not 3.5)
local r: integer = 7 % 2     -- 1
```

If you want the real-number quotient, convert one operand first:

```saule
local q: float = float(7) / 2.0    -- 3.5
```

Because mixing kinds is a compile error, `7 / 2.0` won't silently produce `3.5` — the checker rejects it and forces an explicit `float(7)` (or `int(2.0)`) so the intent is visible at the call site.

### `nil` Is a Value, Not a Binding Type

`nil` exists only as the **value** that inhabits a nullable slot. Writing `: nil` as a binding type is rejected so the meaning of the type system stays "every variable has a real type, and `?` says whether it can be empty":

```saule
local nothing: nil = nil       -- ERROR: `nil` is not a valid binding type
local pending: string? = nil   -- ok — `string?` means "string or nil"
```

`nil` is still legal as a **value** (`return nil`, `x = nil`, `match v case nil then …`) and as the conventional `-> nil` return type meaning "this function returns nothing".

### Casting

Use `int()` and `float()` to explicitly convert between the two:

```saule
local health: integer = 100
local dmg: float = 10.5

local result: integer = health - int(dmg)       -- dmg truncated to 10
local precise: float = float(health) - dmg      -- health promoted to 100.0
```

Casting rules:
- `int(float)` — truncates toward zero, no rounding
- `float(integer)` — always safe, no precision loss

### Escaping `any` with `as`

`any` is the one type the checker cannot see through, so it is the one type
that needs a way out. `x as T` is a **checked** cast: it tests the value at
runtime and evaluates to `T?` — the value when it really is a `T`, and `nil`
when it isn't.

```saule
fn describe(y: any) -> string
    match type(y)
        case "integer" then return "int " .. tostring(y as integer ?? 0)
        case "string" then return "str " .. (y as string ?? "?")
        case _ then return "other"
    end
end
```

Because the result is nullable, the failure case cannot be ignored — combine
it with `??` for a fallback or `!` to turn it back into a throw:

```saule
local n: integer = value as integer ?? 0     -- default on mismatch
local m: integer = (value as integer)!       -- throw on mismatch
```

This is what makes `any` **sound**: a value annotated `integer` really is an
integer at runtime, because the only path from `any` to `integer` goes
through a test.

- `as` binds tighter than every binary operator, so `y as integer ?? 0`
  reads as `(y as integer) ?? 0`.
- Class casts respect inheritance — a `Dog` satisfies `as Animal`.
- `table<T>` is checked **elementwise**, so the element type is honest. That
  is O(n); an empty table satisfies any element type.
- `as` on a value whose type is already known is an error, not a no-op —
  use `int()` / `float()` for numeric conversion.
- Both are explicit — Saule **never** casts silently

```saule
local x: float = 7.9
print(int(x))    -- 7, not 8, truncation not rounding
```

---

## Variables

Saule follows Lua's scoping model: `local` makes a binding **lexically scoped** (visible only inside the block / function / file where it was declared), and an assignment **without** `local` creates an **implicit global** (visible from anywhere after that point). The type annotation is optional in either form — when omitted, the type is inferred from the initializer.

### Local (Recommended)

`local` is the workhorse — same lifetime as the surrounding block, no leak into the rest of the program:

```saule
local name: string = "Arthur"
local health: integer = 100
local speed: float = 1.5
local alive: boolean = true
```

### Global

Drop `local` to publish the binding as a top-level name. Use sparingly — globals defeat scope-based reasoning and are the usual source of "where did this value come from?" bugs:

```saule
appName: string = "MyGame"        -- global
version = 1                       -- global, type inferred

fn showHeader()
    print(appName .. " v" .. version)   -- visible here without import
end
```

A global is created on its **first** assignment. Subsequent `name = expr` (still no `local`) updates that global. Inside a function, an assignment to an undeclared name follows the same rule — it creates / updates the global.

### Inferred Type

When the right-hand side is unambiguous, drop the `: T`:

```saule
local name = "Arthur"        -- inferred string
local health = 100           -- inferred integer
local speed = 1.5            -- inferred float
local alive = true           -- inferred boolean
```

The explicit form is preferred for public APIs (function bodies, module-level constants, anything someone else will read); inferred bindings are fine for short-lived intermediates.

### Multiple Bindings

Declare and assign several names in one statement. Types can be mixed (each name carries its own optional annotation):

```saule
local x: integer, y: integer = 10, 20
local name, age = "Arthur", 36          -- both inferred
local q, r = divmod(17, 5)              -- unpack multi-return
```

### Nullable Without Initializer

A `local` declaration with no initializer is implicitly `nil`, so the type must be nullable:

```saule
local pending: string? = nil    -- ok
local pending: string?          -- ok, same thing
local pending: string           -- ERROR: `string` is never nil
```

### Reassignment

`local` introduces the binding once; subsequent writes use plain `name = expr` (no `local`):

```saule
local hp: integer = 100
hp = hp - 25                    -- reassign the local
local hp: integer = 0           -- ERROR: `hp` is already declared in this scope
```

> Class **fields** are a separate thing — they live on instances or the class itself, not in the surrounding scope. They use `name: T = expr` for public and `local name: T = expr` for private. See [Classes → Access Modifiers](#access-modifiers).

---

## Tables

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

`for v in t` walks the array part. `for k, v in t` walks both the array and map parts (key/value iteration). See [Loops](#loops) and the [Standard Library](./DOCS.md#table) for the full set of helpers (`Table.insert`, `Table.sort`, `Table.concat`, …).

---

## Functions

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

## Lambdas and Closures

### Arrow Lambda

Single expression, most common form:

```saule
local double: fn(integer) -> integer = x => x * 2
local square: fn(integer) -> integer = x => x * x
local greet: fn(string) -> nil       = name => print("Hi " .. name)
local half: fn(float) -> float       = x => x / 2.0
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

---

## Classes

### Declaring a Class

Each class lives in its own `.sau` file. Fields are declared at the top, followed by an `fn init` method (the constructor) and the rest of the methods.

```saule
export class Player
    local name: string
    local health: integer
    local speed: float

    static maxHealth: integer = 100

    fn init(name: string, health: integer, speed: float)
        self.name = name
        self.health = health
        self.speed = speed
    end

    fn greet()
        print("Hi, I am " .. self.name)
    end

    fn damage(amount: integer)
        self.health = self.health - amount
    end

    fn isAlive() -> boolean
        return self.health > 0
    end

    local fn secret()
        print("This is private")
    end

    static fn getMaxHealth() -> integer
        return maxHealth
    end
end
```

`fn init` is the **only** constructor — there is no `constructor` keyword.

### Instantiation

Call the class as if it were a function:
```saule
local p: Player = Player("Arthur", 100, 5)

p.greet()
```

### Implicit `self`

Inside a method body, `self` is always in scope if it is a non `static` and in an **instance method** (and `init`), so `self` is the instance.

In addition, every class member — static fields, static methods, instance methods — is reachable by its **bare name** from inside any method of the same class. Local variables and parameters can shadow them, which is what you want.

```saule
class Counter
    local count: integer = 0

    static local cap: integer = 10

    fn tick()
        if self.count >= cap then
            return
        end

        self.count = self.count + 1
        self.report()
    end

    fn report()
        print("Count is " .. self.count)
    end
end
```

### Access Modifiers

| Syntax | Access |
|---|---|
| `name: string` | public field |
| `local name: string` | private field |
| `fn method()` | public method |
| `local fn method()` | private method |
| `static field: T = value` | class-level, shared, public |
| `static local field: T = value` | class-level, shared, private |

### Static Members

Static fields and methods belong to the class itself, not to instances. They are accessed via the class name from the outside, or by bare name (or `self.name` in a `static fn`) from inside:

```saule
print(Player.maxHealth)         -- 100
print(Player.getMaxHealth())    -- 100
```

Static fields are shared across all instances. Modifying them affects the class globally:

```saule
Player.maxHealth = 200
```

A class with **no** `fn init` promotes every `local field = expr` to a static, evaluated once at class-declaration time. This makes a class usable as a small module:

```saule
class Main
    static local lauris: Person = Person("Laurynas")

    static fn main()
        lauris.introduce()     -- `lauris` resolves via the class
    end
end
```

### Inheritance

Use `extends` to inherit from another class. Call the parent's `init` with `self.super(...)` from inside `init`:

```saule
export class Entity
    name: string

    fn init(name: string)
        self.name = name
    end

    fn getName() -> string
        return self.name
    end
end

export class Player extends Entity
    local health: integer
    local speed: float

    fn init(name: string, health: integer, speed: float)
        self.super(name)

        self.health = health
        self.speed = speed
    end

    fn greet()
        print("Hi, I am " .. self.getName())
    end
end
```

Rules:
- A class can only extend **one** parent
- Private members from the parent are **not accessible** in the child
- Public and static members are inherited

### Overriding

Redeclaring an inherited method overrides it. The override has to be usable
everywhere the parent's version was, because a caller holding the parent type
cannot know which one it will get. The checker enforces:

- **Same parameter count and types.** Widening or renaming the shape of the
  call is a compile error.
- **A return type the parent's callers accept.** Narrowing is fine — an
  override may return a *subclass* of what the parent declared. Returning
  something unrelated is a compile error.
- **Instance stays instance, static stays static.**

```saule
class Base
    fn get() -> integer
        return 1
    end
end

class Derived extends Base
    fn get() -> string        -- ERROR: the parent returns `integer`
        return "oops"
    end
end
```

`fn init` is exempt — constructors aren't dispatched through a parent
reference, so a subclass constructor may take whatever parameters it needs
and forward what the parent wants via `self.super(...)`.

If a method wasn't meant to override anything, give it a different name.

---

## Interfaces

Interfaces define a contract — method signatures only, no fields, no bodies.

### Declaring an Interface

```saule
interface Greetable
    fn greet() -> nil
end

interface Damageable
    fn damage(amount: integer) -> nil
    fn isAlive() -> boolean
end
```

### Implementing an Interface

A class can implement multiple interfaces:

```saule
export class Player extends Entity implements Greetable, Damageable
    local health: integer

    fn init(name: string, health: integer)
        self.super(name)

        self.health = health
    end

    fn greet()
        print("Hi, I am " .. self.getName())
    end

    fn damage(amount: integer)
        self.health = self.health - amount
    end

    fn isAlive() -> boolean
        return self.health > 0
    end
end
```

### Interface Composition

Interfaces can extend other interfaces:

```saule
interface Combatant extends Damageable
    fn attack(target: Damageable) -> nil
end
```

### Interfaces as Types

This is the main power — use interfaces as parameter and variable types:

```saule
fn processEntity(target: Damageable, amount: integer) -> nil
    if target.isAlive() then
        target.damage(amount)
    end
end

local p: Player = Player("Arthur", 100)
processEntity(p, 30)    -- works, Player implements Damageable
```

### Generic Interfaces

```saule
interface Repository<T>
    fn save(item: T) -> nil
    fn findById(id: integer) -> T
    fn delete(id: integer) -> nil
end

export class PlayerRepository implements Repository<Player>
    local items: table<Player>

    fn init()
        self.items = {}
    end

    fn save(item: Player)
        self.items[#self.items + 1] = item
    end

    fn findById(id: integer) -> Player
        return self.items[id]
    end

    fn delete(id: integer)
        self.items[id] = nil
    end
end
```

### Custom Iterable

Any class implementing `Iterable<T>` works inside a `for-in` loop automatically. The contract is a single method `iter()` that returns a **step closure**: each call returns the next element, or `nil` to signal the end. The loop stops on the first `nil`.

```saule
interface Iterable<T>
    fn iter() -> fn() -> T?
end

export class PlayerQueue implements Iterable<Player>
    local items: table<Player>

    fn init()
        self.items = {}
    end

    fn push(p: Player)
        self.items[#self.items + 1] = p
    end

    fn iter() -> fn() -> Player?
        local cursor: integer = 1

        return fn()
            if cursor > #self.items then
                return nil
            end

            local p: Player = self.items[cursor]
            cursor = cursor + 1
            return p
        end
    end
end

local queue: PlayerQueue = PlayerQueue()
queue.push(Player("Arthur", 100))
queue.push(Player("Merlin", 80))

for player: Player in queue do
    player.greet()
end
```

For iteration that yields **pairs** (key + value, index + value, etc.), implement `Iterable2<K, V>` whose `iter()` returns a closure with two return values:

```saule
interface Iterable2<K, V>
    fn iter() -> fn() -> (K?, V?)
end

for key: string, value: Player in playerMap do
    print(key .. " = " .. value.getName())
end
```

The loop also accepts raw step closures and plain `table` values directly — `Iterable` is just the contract that makes user-defined classes look the same.

---

## Enums

### Simple Enum

```saule
enum Direction
    North
    South
    East
    West
end

local d: Direction = Direction.North
```

### Valued Enum

```saule
enum Status
    Alive = "alive"
    Dead = "dead"
    Unknown = "unknown"

    fn describe() -> string
        return "Status is: " .. self.value
    end
end

local s: Status = Status.Alive
print(s.describe())    -- "Status is: alive"
```

### Enums as Types

```saule
fn move(self, dir: Direction) -> nil
    if dir == Direction.North then
        self.y = self.y + 1
    end
end
```

---

## Pattern Matching

`match` selects one of several branches by structurally inspecting a value. Unlike a C-style `switch`, every arm is independent (no fall-through), patterns can **bind** parts of the value, and the typechecker requires the arms to be **exhaustive** — so a forgotten enum variant or unhandled `nil` is a compile error, not a production incident.

`match` is also an **expression**: every arm produces a value of the same type, and the whole `match` evaluates to that value.

### Basic Match

```saule
local label: string = match status
    case Status.Ok then "fine"
    case Status.Warn then "watch out"
    case Status.Error then "oops"
end
```

Each arm is `case <pattern> then <expression-or-block>`. The block runs until the next `case` or the closing `end`.

### Matching Enum Payloads

When a variant carries data, the pattern binds those fields into locals visible inside the arm:

```saule
enum Event
    Click(x: integer, y: integer),
    Key(code: string),
    Quit
end

fn describe(e: Event) -> string
    return match e
        case Event.Click(x, y) then "click at " .. x .. "," .. y
        case Event.Key(code) then "key: " .. code
        case Event.Quit then "bye"
    end
end
```

### Matching Nullables

`match` is the cleanest way to consume a `T?` — `nil` is just another pattern:

```saule
match repo.findById(id)
    case nil then println("not found")
    case user then println("hi " .. user.name)
end
```

The bare name `user` here is a **binding pattern**: it captures the non-nil value. Inside that arm `user` has type `Player`, not `Player?`.

### Wildcard and Bindings

Use `_` to ignore a value and any identifier to bind it:

```saule
match n
    case 0 then println("zero")
    case 1 then println("one")
    case other then println("something else: " .. other)
end

match pair
    case (0, _) then println("x is zero")
    case (_, 0) then println("y is zero")
    case _ then println("neither")
end
```

A pattern that starts with a lowercase identifier (and isn't an enum variant) binds; `_` matches anything without binding.

### Guards with `when`

Add a condition that must also hold for the arm to fire:

```saule
match n
    case x when x < 0 then println("negative")
    case 0 then println("zero")
    case x then println("positive " .. x)
end
```

Guards are evaluated **after** the pattern matches. If the guard is false, matching continues with the next arm.

### Literal Patterns

Numbers, strings, booleans, and `nil` are all valid patterns:

```saule
match command
    case "quit" then exit()
    case "help" then showHelp()
    case other then println("unknown: " .. other)
end
```

### Multiple Return Values

Match destructures tuples returned by multi-value functions:

```saule
fn divmod(a: integer, b: integer) -> (integer, integer)
    return a / b, a % b
end

match divmod(a, b)
    case (q, 0) then println("clean: " .. q)
    case (q, r) then println(q .. " rem " .. r)
end
```

### Exhaustiveness

The typechecker verifies that **every possible value** of the scrutinee is covered:

```saule
enum Color
    Red, Green, Blue
end

-- ❌ compile error: non-exhaustive match, missing case `Color.Blue`
local name: string = match c
    case Color.Red then "red"
    case Color.Green then "green"
end
```

Add the missing variant or a wildcard arm to fix it. For nullables, both `nil` and a binding (or `_`) must be covered. Guards never count toward exhaustiveness — if every arm has a guard, you still need a wildcard fallback.

### As an Expression

Because `match` produces a value, it composes naturally with `local`, `return`, and arguments:

```saule
fn priceFor(tier: Tier) -> integer
    return match tier
        case Tier.Free then 0
        case Tier.Pro then 9
        case Tier.Enterprise then 99
    end
end
```

If used as a statement, the result is simply discarded — same rule as any other expression.

---

## Null Safety

Saule enforces null safety at compile time. A type is only nullable if declared with `?`.

```saule
local name: string? = nil       -- ok, nullable
local name: string = nil        -- ERROR, string is never nil
```

### Safe Access

Use `?.` to access a member that may be nil. Returns nil instead of crashing:

```saule
local player: Player? = repo.findById(id)
local name: string? = player?.name      -- nil if `player` was nil
```

For lengths of strings and tables, use `#`:

```saule
local greeting: string? = nil
local len: integer = #(greeting ?? "")     -- 0 when nil
```

### Null Coalescing

Use `??` to provide a fallback when a value is nil:

```saule
local display: string = name ?? "Unknown"
```

### Force Unwrap

Use `!` to assert a value is not nil. Crashes at runtime if it is:

```saule
local forced: string = name!
```

### Combining Operators

```saule
local players: table<Player?> = getPlayers()

for player: Player? in players do
    local name: string = player?.getName() ?? "Unknown"

    print(name)
end
```

---

## Error Handling

Saule uses `try / catch` for unexpected runtime errors. For expected, recoverable failures — missing data, invalid input, parse errors — prefer **nullable return types** (`-> T?`) and let null safety carry the failure through the type system. A user-defined `Result<T>` class is trivial to write on top of classes and generics if you want richer error payloads.

### Throwing Errors

```saule
fn damage(amount: integer)
    if amount < 0 then
        throw "Damage cannot be negative"
    end

    self.health = self.health - amount
end
```

### Try / Catch

```saule
try
    local p: Player = Player("Arthur", 100)
    p.damage(-10)
catch e: string
    print("Caught: " .. e)
end
```

The `catch` clause names the thrown value and its expected type. Inside the catch block, code runs as if the `try` block had returned normally.

### Nullable returns for expected failures

```saule
fn findPlayer(id: integer) -> Player?
    if id < 0 then
        return nil
    end
    return self.items[id]
end

local name: string = repo.findPlayer(5)?.getName() ?? "Unknown"
```

Reserve `try / catch` for truly unexpected runtime errors — bad data from an external source, file I/O failures, contract violations. For everyday "this lookup might miss", `T?` plus `?.` / `??` keeps the failure mode visible in the type signature.

---

## Loops

### Numeric For

```saule
for i: integer = 1, 10 do
    print(i)
end

-- with step
for i: integer = 0, 100, 5 do
    print(i)
end

-- counting down
for i: integer = 10, 1, -1 do
    print(i)
end
```

### For Each

```saule
local names: table<string> = {"Arthur", "Merlin", "Lancelot"}

for name: string in names do
    print(name)
end

-- with index
for i: integer, name: string in names do
    print(i .. ": " .. name)
end

-- types are optional — inferred from the iterated value
for name in names do
    print(name)
end
```

### While

```saule
local hp: integer = 100

while hp > 0 do
    hp = hp - 10
end
```

### Repeat Until

Runs at least once, checks the condition at the end:

```saule
local input: string? = nil

repeat
    input = getInput()
until input != nil
```

### Break and Continue

```saule
for i: integer = 1, 10 do
    if i == 5 then continue end
    if i == 8 then break end
    print(i)    -- prints 1, 2, 3, 4, 6, 7
end
```

---

## Imports and File Structure

### Importing

An import names either a single `.sau` file or a folder module (a directory with an `init.sau` — see [Folder Modules](#folder-modules-initsau)). The path is relative to the importing file's directory, then the project's `src_dirs`.

```saule
-- single import
import Player from "entities/Player"

-- import with alias
import PlayerRepository as PlayerRepo from "data/PlayerRepository"

-- import a utility module
import Math from "utils/Math"

-- pull every exported name out of one file
import * from "entities/Player"
```

The path may be written **with or without quotes**. Unquoted, `.` separates folders — the two lines below mean exactly the same thing:

```saule
import * from "some/folder/module"
import * from some.folder.module
```

### Apps and Libraries

A project is one of two shapes, declared by `kind:` in `saule.config`:

| `kind` | Has `entry:` | `saule run` | Purpose |
|---|---|---|---|
| `"app"` (default) | yes | runs it | a program |
| `"library"` | no | refuses, and says why | imported by other projects |

Scaffold either with `saule init`:

```sh
saule init myapp          # an app, with src/main.sau
saule init mylib --lib    # a library, with src/init.sau
```

A library's `src/init.sau` is its public surface — whatever that file exports
is what importers see. Running one is a category error and reports as such
rather than failing on a missing entry file.

### Importing from a Dependency

A project listed in `dependencies:` is reachable by its `name:`. Naming the
dependency on its own imports **the package itself**:

```saule
import Json from "json"          -- the `json` package
import Parser from "json/lexer"  -- a specific module inside it
```

A package exposes itself through an **`init.sau`** in one of its `src_dirs` —
the same [folder module](#folder-modules-initsau) rule that applies anywhere
else, so there is one convention to learn rather than a special case for
dependencies. A package without one can still have its modules imported by
path, but its name alone won't resolve.

```
json/
├── saule.config          name: "json"
└── src/
    └── init.sau          ← what `import ... from "json"` gets
```

### Folder Modules (`init.sau`)

A folder becomes a single importable **module** by giving it an `init.sau`. That file is a *barrel*: whatever it imports becomes the module's public surface, so a folder of files can be consumed as one unit.

```saule
-- some/folder/module/init.sau
-- Paths are relative to this file. This is all the barrel does: it lists
-- the files whose exports should be visible to importers of the module.
import * from view
import * from button
```

Consumers then import the folder itself and get everything the barrel pulled in:

```saule
import * from some.folder.module

local view: View = View("Name")
local b: Button = Button()
```

Named and aliased imports work against a barrel too:

```saule
import View from some.folder.module
import View as V, Button from some.folder.module
```

Re-exporting is **only** done by `init.sau` / `init.saule`. Any other file keeps its imports private — importing a regular file gives you the names it declared with `export`, never the ones it imported. That keeps a file's imports an implementation detail.

### Exporting

Add `export` before a class, interface, enum, or function to make it accessible from other files:

```saule
export class Player
    -- ...
end

export fn clamp(value: integer, min: integer, max: integer) -> integer
    if value < min then return min end
    if value > max then return max end

    return value
end
```

A file without `export` is private to itself — even sibling files in the same folder can't see its declarations. The only way to share code across files is to `export` it and `import` it explicitly.

### Utility Modules

Not everything needs a class. Export standalone functions from a utility file:

```saule
-- utils/Math.sau

export fn clamp(value: integer, min: integer, max: integer) -> integer
    if value < min then return min end
    if value > max then return max end
    return value
end

export fn lerp(a: float, b: float, t: float) -> float
    return a + (b - a) * t
end
```

```saule
import Math from "utils/Math"

local clamped: integer = Math.clamp(150, 0, 100)   -- 100
local smooth: float = Math.lerp(0.0, 1.0, 0.5)    -- 0.5
```

### Visibility Rules

| Situation | Accessible from |
|---|---|
| `export class Foo` | anywhere that imports it |
| `class Foo` without export | only inside the same file |
| `local` field or method | only within the class |
| `static` field or method | via `ClassName.x` anywhere |

### Circular Imports

Saule forbids circular imports at compile time:

```
ERROR: Circular import detected
  Player.sau → Inventory.sau → Player.sau

  Hint: Extract shared types into a separate file
```

---

## Project Configuration

Every Saule project has a `saule.config` file at the root:

```
name: "myproject"
version: "1.0.0"
entry: "main.sau"
src_dirs: ["src"]
dependencies: ["../shared-lib", "~/code/json"]
min_saule_version: "2026.1.0"
indent_style: "space"
indent_width: 2
```

Recognised keys:

| Key | Purpose |
|---|---|
| `name` | Project name; also the import prefix exposed to dependents |
| `version` | Free-form version string (semver recommended) |
| `entry` | Path to the entry `.sau` file, relative to the project root (apps only) |
| `kind` | `"app"` (default) or `"library"` — a library has no entry point and is imported rather than run |
| `src_dirs` | List of directories to search when resolving imports |
| `dependencies` | List of paths to other Saule projects (each must itself contain a `saule.config`); `~/` expands to the home directory |
| `min_saule_version` | Refuses to run if the toolchain reports a lower version |
| `indent_style` | Formatting: `"tab"` or `"space"` (default `"space"`) |
| `indent_width` | Formatting: columns per indent level, 1–16 (default `2`) |

Unknown keys are ignored.

The two `indent_*` keys are what `saule fmt` and the language server both read,
so a project's declared style survives a Reformat in the IDE and a `saule fmt -w`
in a terminal alike. They override the editor's own Code Style settings; the
`saule fmt --indent <n>` / `--tabs` / `--spaces` flags override them in turn.

### Recommended Project Structure

```
myproject/
├── saule.config
├── main.sau
├── entities/
│   ├── Entity.sau
│   ├── Player.sau
│   └── Enemy.sau
├── data/
│   ├── Repository.sau
│   └── PlayerRepository.sau
├── utils/
│   ├── Math.sau
│   └── Logger.sau
└── enums/
    ├── Direction.sau
    └── Status.sau
```

### Entry Point

There are two ways to run Saule code, with different rules about what the entry file must contain:

**Project mode** — `saule run` in a directory containing `saule.config`, or `saule run <dir>` naming one. The file pointed to by `entry:` must declare:

```saule
class Main
    static fn main()
        -- your code here
    end
end
```

Top-level statements in the entry file still execute first (handy for one-off setup or imports), and then `Main.main()` is called. Without a `Main` class the runner exits with `error: '<entry>' must declare 'class Main' with a 'static fn main()' entry point`.

**Single-file mode** — `saule run path/to/file.sau`, naming a file rather than a directory. The file is executed top-to-bottom like a Lua script; no `class Main` is required, and any surrounding `saule.config` is ignored. If the script happens to define a `Main` with a `static fn main()`, it is invoked as a convenience after the top-level body finishes.

Whether the target is a directory is the *only* thing that picks between the two modes. Arguments for the program itself go after `--`, where the CLI passes them through untouched to `Os.args()`:

```sh
saule run -- input.bf          # project in the cwd, Os.args() = ["input.bf"]
saule run tool.sau -- -v file  # single file; script args may start with `-`
```

A typical project-mode entry file:

```saule
import Player from "entities/Player"
import Math from "utils/Math"

class Main
    static fn main()
        local p: Player = Player("Arthur", 100, 1.5)
        p.greet()

        local dmg: integer = Math.clamp(50, 0, 100)
        p.damage(dmg)
    end
end
```

---

## Quick Reference

### Keywords

| Keyword | Purpose |
|---|---|
| `class` | Declare a class |
| `interface` | Declare an interface |
| `enum` | Declare an enum |
| `fn` | Declare a function or method |
| `extends` | Inherit from a class |
| `implements` | Fulfill one or more interfaces |
| `super` | Call the parent's `init` |
| `self` | Reference the current instance |
| `static` | Declare a class-level member |
| `local` | Declare a private member or variable |
| `export` | Make a file member publicly importable |
| `import` | Import from another file |
| `return` | Return a value from a function |
| `throw` | Raise an error |
| `try` | Begin an error-handled block |
| `catch` | Handle a thrown error |
| `for` | Begin a loop |
| `while` | Begin a while loop |
| `repeat` | Begin a repeat-until loop |
| `until` | End condition for repeat loop |
| `break` | Exit a loop |
| `continue` | Skip to the next iteration |
| `if / elseif / else / end` | Conditional logic |
| `match` | Begin a pattern-matching expression |
| `case` | Introduce a pattern arm inside `match` |
| `when` | Attach a guard condition to a `case`, or start a `when(...)` pipeline |
| `then` | Begin a `match` arm body / `if` branch |
| `nil` | Absence of value |
| `true / false` | Boolean literals |

### Operators

| Operator | Meaning |
|---|---|
| `?.` | Safe member access |
| `??` | Null coalescing fallback |
| `!` | Force unwrap nullable |
| `..` | String concatenation |
| `#` | Length of table or string |
| `==`, `!=` | Equality checks |
| `>`, `<`, `>=`, `<=` | Comparisons |
| `and`, `or`, `not` | Boolean logic |
| `+`, `-`, `*`, `/`, `%` | Arithmetic (`/` on two `integer`s truncates) |
| `:` | Pipeline stage call inside `when(...)` |
| `int()` | Cast float to integer, truncates toward zero |
| `float()` | Cast integer to float, always safe |
