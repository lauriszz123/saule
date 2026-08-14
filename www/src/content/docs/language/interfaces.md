---
title: "Interfaces"
description: "Interfaces define a contract — method signatures only, no fields, no bodies."
sidebar:
  order: 7
---

<!-- Generated from README.md by `npm run sync-docs`. Edit that file, not this one. -->

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

### Operator Overloading

`Iterable` isn't the only built-in contract. A family of `Op*` interfaces lets a class define what the operators mean for its own instances — Saule's answer to Lua's `__add`, `__sub`, `__concat`, … metamethods, with one interface per operator so a class opts into exactly what it can support.

| Interface | Operator | Method |
|---|---|---|
| `OpAdd<T, R>` | `a + b` | `fn add(other: T) -> R` |
| `OpSub<T, R>` | `a - b` | `fn sub(other: T) -> R` |
| `OpMul<T, R>` | `a * b` | `fn mul(other: T) -> R` |
| `OpDiv<T, R>` | `a / b` | `fn div(other: T) -> R` |
| `OpMod<T, R>` | `a % b` | `fn mod(other: T) -> R` |
| `OpPow<T, R>` | `a ^ b` | `fn pow(other: T) -> R` |
| `OpBAnd<T, R>` | `a & b` | `fn band(other: T) -> R` |
| `OpBOr<T, R>` | `a \| b` | `fn bor(other: T) -> R` |
| `OpBXor<T, R>` | `a ~ b` | `fn bxor(other: T) -> R` |
| `OpShl<T, R>` | `a << b` | `fn shl(other: T) -> R` |
| `OpShr<T, R>` | `a >> b` | `fn shr(other: T) -> R` |
| `OpNeg<R>` | `-a` | `fn neg() -> R` |
| `OpBNot<R>` | `~a` | `fn bnot() -> R` |
| `OpLen` | `#a` | `fn len() -> integer` |
| `OpConcat<T, R>` | `a .. b` | `fn concat(other: T) -> R` |
| `OpEq<T>` | `a == b`, `a != b` | `fn equals(other: T) -> boolean` |
| `OpCompare<T>` | `<`, `<=`, `>`, `>=` | `fn compare(other: T) -> integer` |
| `OpToString` | `tostring(a)`, `print(a)` | `fn toString() -> string` |

Four more are **behaviour** contracts rather than operators — no symbol triggers them:

| Interface | Fires on | Method |
|---|---|---|
| `OpIndex<K, V>` | `a[k]` | `fn index(key: K) -> V` |
| `OpNewIndex<K, V>` | `a[k] = v` | `fn newIndex(key: K, value: V) -> nil` |
| `Assignable<T>` | `local a: C = t` | `static fn of(value: T) -> C` |

They are always in scope — no import needed, like `Iterable`.

```saule
export class Vec2 implements OpAdd<Vec2, Vec2>, OpMul<Vec2, Vec2>, OpEq<Vec2>, OpToString
    local x: float
    local y: float

    fn init(x: float, y: float)
        self.x = x
        self.y = y
    end

    fn add(other: Vec2) -> Vec2
        return Vec2(self.x + other.x, self.y + other.y)
    end

    fn mul(other: Vec2) -> Vec2
        return Vec2(self.x * other.x, self.y * other.y)
    end

    fn equals(other: Vec2) -> boolean
        return self.x == other.x and self.y == other.y
    end

    fn toString() -> string
        return "(" .. self.x .. ", " .. self.y .. ")"
    end
end

local a: Vec2 = Vec2(1.0, 2.0)
local b: Vec2 = Vec2(3.0, 4.0)

local sum: Vec2 = a + b       -- (4.0, 6.0)
print(sum)                    -- toString() runs here
print(a == Vec2(1.0, 2.0))    -- true — equals(), not identity
```

The result type comes from the method's own return type, so `a + b` above is a `Vec2` and can fill a `Vec2` slot with no cast.

#### Dispatch Rules

**The `implements` clause is the opt-in.** Defining `fn add(...)` without listing `OpAdd` leaves `+` a compile error — the operator is part of a class's public contract, not something a method name enables by accident.

**Arithmetic and `..` dispatch on the left operand.** `vec - 2` looks for `Vec2.sub`; `2 - vec` is an error rather than silently computing `vec - 2`. Put the class on the left, or give the other type its own overload.

**`==` and the ordering operators are symmetric** — either side may carry the overload — and always produce a `boolean`.

**One `compare` covers all four ordering operators.** It returns an `integer`: negative when `self` sorts first, zero when the two are equivalent, positive when `self` sorts last.

```saule
export class Version implements OpCompare<Version>
    local major: integer
    local minor: integer

    fn init(major: integer, minor: integer)
        self.major = major
        self.minor = minor
    end

    fn compare(other: Version) -> integer
        if self.major != other.major then
            return self.major - other.major
        end

        return self.minor - other.minor
    end
end

local old: Version = Version(1, 9)
local new: Version = Version(2, 0)

print(old < new)     -- true
print(new >= old)    -- true
```

**`nil` never reaches an overload.** `v == nil` stays the nullability check it looks like rather than calling `equals(other: Vec2)` with nothing in hand — the same restriction Lua puts on `__eq`.

**`OpToString` also drives `..`.** A class with `OpToString` but no `OpConcat` can sit on either side of `..` and renders itself into the resulting string; `OpConcat` takes over when you want `..` to build something other than a string.

**`OpConcat` takes `..` over completely.** Because `..` is right-associative and dispatches left, writing `"path = " .. somePath` puts the class on the left of a string and calls its `concat`, which is a type error when `concat` expects its own type. Reach for `tostring(somePath)` in that case. This only affects classes that implement `OpConcat` — one with just `OpToString` interpolates the way you'd expect.

**Overloads are inherited.** A subclass gets its parent's operators, and can override any of them by redefining the method.


### Wrapper Types

Two more contracts cover what Lua does with `__index` and `__newindex`, plus
one for building an object straight from a value — all in a form the
typechecker can still see through.

#### `OpIndex` / `OpNewIndex` — full control of get and set

Saule's `__index` / `__newindex`, with one deliberate difference from Lua's:
they are **not** miss handlers over a stored key space. A class instance has
no keys of its own, so the method *is* the lookup and runs on every access.

```saule
class Settings implements OpIndex<string, string>, OpNewIndex<string, string>
    local data: table<string, string>
    fn init() self.data = {} end
    fn index(key: string) -> string
        return self.data[key] ?? "(unset)"
    end
    fn newIndex(key: string, value: string) -> nil
        self.data[key] = String.lower(value)   -- normalise on every write
    end
end

local s: Settings = Settings()
s["theme"] = "SOLARIZED"
println(s["theme"])       -- solarized
println(s["missing"])     -- (unset)
```

The element type comes from `index`'s own return type, and the key is checked
against its parameter — so `s[42]` is a compile error.

**`obj.name` is deliberately not routed here.** Field and method names are
resolved statically, and sending their misses to a hook would give up
"unknown member" diagnostics for the whole class. Dynamic access is
`obj[key]`; a fixed surface is declared as ordinary methods.

A hook that indexes `self` re-enters itself. Lua answers that with `rawget` /
`rawset`; Saule caps the depth and reports it, so the mistake is a diagnostic
naming the class rather than a hang.

#### `Assignable<T>` — build from an assigned value

```saule
class Text implements Assignable<string>, OpToString
    local raw: string
    fn init(raw: string)  self.raw = raw end

    static fn of(s: string) -> Text return Text(s) end

    -- Everything this type exposes, it declares. `string` has no methods,
    -- so each one calls the `String` class explicitly.
    fn upper() -> string  return String.upper(self.raw) end
    fn length() -> integer return String.len(self.raw) end
    fn toString() -> string return self.raw end
end

local greeting: Text = "hello, world"   -- runs Text.from
println(greeting)                        -- hello, world  (OpToString)
println(greeting.upper())                -- HELLO, WORLD
```

The method is **static** — there is no instance yet to call it on — and
`from` is usable as a name here even though it opens an `import` tail
elsewhere, because after `fn` and after `.` an import cannot start.

The annotation picks the target, so this is *target-typed*: there is never a
question of which class a bare value should become, only whether the one
asked for accepts it.

**Conversion applies at exactly two kinds of site**: an annotated `local` or
module variable, and a user function's or method's parameters. Everywhere
else the ordinary rule stands:

```saule
local all: table<Text> = {"a"}   -- ERROR: table elements do not convert
local t: Text = "a"
t = "b"                           -- ERROR: only the declaration converts
```

That boundary is soundness rather than an unfinished edge. The interpreter
converts at those sites and only those, so relaxing the checker anywhere else
would typecheck a value that never gets built — leaving a raw `string` inside
a `table<Text>` for the first `Text` member call to trip over.

See [`examples/wrapper-types`](./examples/wrapper-types) for all three
working together.

---
