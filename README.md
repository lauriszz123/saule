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
- [Quick Reference](#quick-reference)
- [Grammar](#grammar)
- [Standard Library →](./DOCS.md)

---

## Types

Saule has 8 primitive types, inherited from Lua's type system but statically declared. Lua's single `number` type is split into two distinct types in Saule:

| Type | Description |
|---|---|
| `integer` | Whole numbers, no decimal component. 64-bit signed; [overflow wraps](#integer-overflow) |
| `float` | Decimal numbers, 64-bit precision |
| `string` | Immutable sequences of characters |
| `boolean` | `true` or `false` |
| `nil` | Absence of value |
| `fn(A, B) -> R` | First-class function values, typed by [signature](#function-types) |
| `table<T>` | The only data structure, typed generically |
| `any` | A value of unknown type. Anything may be assigned **to** an `any`; getting a value back **out** requires a checked [`as` cast](#escaping-any-with-as) |

Parameters and fields must declare their type; on a local or a return type the annotation is optional and inferred from what you wrote. A variable cannot be `nil` unless its type is marked nullable with `?`.

### Function types

A function value's type is its **signature**: the types it is called with and the type it hands back. There is no bare `function` type — a slot that holds a callable has to say which calls are legal against it.

```saule
local onTick: fn(float) -> nil = fn(dt: float)
  advance(dt)
end

local compare: fn(integer, integer) -> boolean = (a, b) => a < b
```

The parts are the same ones a declaration writes: a parenthesised parameter-type list, then `->` and the return type. `-> nil` is the "returns nothing" spelling. Nullability wraps the whole type, so an optional callback needs parentheses — `(fn(string) -> nil)?`, not `fn(string) -> nil?`, which is a callback returning a nullable `nil`.

```saule
class TextField
  -- Optional callbacks: absent until the caller supplies one.
  onSubmitted: (fn(string) -> nil)?
  onChanged: (fn(string) -> nil)?
end
```

The payoff is that anonymous functions assigned into such a slot are checked against it — arity, parameter types, and return type — instead of being accepted on the grounds that they are *some* function:

```saule
local field = TextField()
field.onChanged = fn(count: integer)   -- error: expected fn(string) -> nil
  print(count)
end
```

It also means the parameters of a lambda written into a typed slot don't need annotations. The signature supplies them, and the body is checked with the real types:

```saule
field.onChanged = text => print(text:upper())   -- `text` is a `string`
```

An earlier version of the language accepted a bare `function` annotation. It carried no arity and no parameter types, so every lambda fit every slot and the mistake surfaced at the call — or, for a callback stored now and invoked later, not at all. Widen deliberately with `any` if a slot really does accept anything.

### Reserved: `userdata` and `thread`

Lua has these and Saule keeps the names reserved, but **neither is implemented yet**. The typechecker will accept `local h: userdata` as an annotation and then nothing in the language can produce a value to satisfy it — there is no literal, no constructor, and no standard-library function returning either. Don't reach for them:

- `userdata` — raw memory for native integrations. Native packages currently exchange values through `table` and the native SDK's own types instead.
- `thread` — coroutines. Saule has no concurrency primitives today; there is no `coroutine` module.

They are listed here so the omission is visible rather than surprising.

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

### Float Literals

Because the two types never mix implicitly, a whole number that belongs in a
`float` has to *say* it is one. There are two ways to write it:

```saule
local ratio: float = 0.75
local half: float = .5         -- the integer part may be omitted
local speed: float = 10f       -- `f` / `F` suffix: this is 10.0
local scale: float = 1.0       -- the same thing, written out
local exact: float = 2.5f      -- allowed, though the `.5` already decided it
```

The suffix earns its keep in expressions, where `10f` is considerably easier to
read than `float(10)`:

```saule
local speed: float = 3.5
println(speed * 2f)            -- 7.0
```

A trailing dot is **not** a float: `1.` lexes as `1` followed by `.`, which is
what keeps `1..2` (concatenation) and `1.foo` (member access) unambiguous.
Write `1.0` or `1f` instead.

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
is a lex error. The `_` separator belongs to these two forms only: in decimal,
`1_000` is the number `1` followed by an identifier.

### String Literals

Strings are written with either `"` or `'`, exactly as in Lua. The two are the
same type and the same syntax — only the quote that opened a literal closes it,
so each style lets the other appear unescaped:

```saule
local plain: string = "hello"
local same: string = 'hello'

local quoted: string = 'he said "hi"'
local possessive: string = "it's fine"
```

Seven escapes are recognised, and both quote escapes work in either style, so
moving a literal between them never invalidates one:

| Escape | Meaning |
|---|---|
| `\n` | Newline |
| `\t` | Tab |
| `\r` | Carriage return |
| `\0` | NUL |
| `\\` | Backslash |
| `\"` | Double quote |
| `\'` | Single quote |

Anything else after a backslash is a lex error — there is no hex or unicode
escape, so use `String.char` for codepoints. A literal newline inside a string
is allowed and is kept as-is.

`saule fmt` leaves the choice alone. It reads the delimiter back out of your
source, so formatting never rewrites `'hello'` into `"hello"` — in a string
literal, a pattern, a table key or an import path.

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

Dividing by zero is a runtime error for both `/` and `%`, not a `nan` or an `inf`:

```saule
local q: integer = 7 / 0     -- ERROR: division by zero
```

### Integer Overflow

`integer` is a signed 64-bit value, spanning `-9223372036854775808` to `9223372036854775807`. Arithmetic that leaves that range **wraps around** rather than trapping — the same rule Lua 5.4 uses:

```saule
local big: integer = 9223372036854775807
println(big + 1)             -- -9223372036854775808, not an error
```

This is the one place Saule does not catch a numeric mistake for you. It is a deliberate trade — a check on every add costs more than it saves for a language used at this scale — but it means a counter or an accumulator that could plausibly reach 2⁶³ needs its own bound. Use `float` when a value's magnitude is genuinely unbounded and precision matters more than exactness.

### Exponentiation

`^` raises a number to a power. It binds tighter than every other arithmetic operator — tighter than unary minus, too — and is right-associative:

```saule
local squared: integer = 5 ^ 2      -- 25
local tower: integer = 2 ^ 3 ^ 2    -- 512, i.e. 2 ^ (3 ^ 2)
local neg: integer = -2 ^ 2         -- -4, i.e. -(2 ^ 2)
local root: float = 2.0 ^ 0.5       -- 1.4142135623730951
```

Like `/`, `^` follows the type of its operands rather than promoting: `integer ^ integer` stays an `integer`. A negative exponent has no integer answer, so it is an error — convert first if you want one:

```saule
local half: float = 2.0 ^ -1.0      -- 0.5
local bad: integer = 2 ^ -1         -- ERROR: negative exponent on integers
```

### Bitwise Operators

Six operators work on the bits of an `integer`, spelled as in Lua 5.3:

| Operator | Meaning |
|---|---|
| `a & b` | and |
| `a \| b` | or |
| `a ~ b` | xor |
| `~a` | complement (one's complement) |
| `a << b` | left shift |
| `a >> b` | right shift |

`~` carries xor because `^` is already exponentiation — the same trade Lua 5.3 made. It is unary complement in prefix position and binary xor in infix position, told apart by where it appears, exactly as `-` already is.

```saule
local flags: integer = 0b1100 | 0b0011   -- 15
local common: integer = 0b1100 & 0b1010  -- 8
local toggled: integer = 0b1100 ~ 0b1010 -- 6
local inverted: integer = ~0             -- -1
local doubled: integer = 1 << 4          -- 16
local halved: integer = 255 >> 4         -- 15
```

**Integers only.** Unlike Lua 5.3, a `float` is rejected rather than converted when it happens to have no fractional part — Saule never mixes the two numeric kinds implicitly, and this is not the place to start:

```saule
local f: float = 6.0
local bad: integer = f & 1               -- ERROR: `&` expects `integer`
local ok: integer = int(f) & 1           -- 0
```

**Shifts fill with zeros in both directions**, which is Lua's rule and means `>>` is a *logical* shift, not an arithmetic one — the sign bit is not replicated. A negative shift count shifts the other way, and shifting by 64 or more shifts every bit out:

```saule
local logical: integer = -1 >> 63        -- 1, not -1
local flipped: integer = 16 >> -2        -- 64 — negative count reverses
local gone: integer = 1 << 64            -- 0
```

**Precedence follows Lua**: `|` is loosest, then `~`, then `&`, then the shifts, and all of them bind looser than `..`, `+` and `*` but tighter than any comparison. So the mask-test idiom needs no parentheses:

```saule
if flags & 0b0100 != 0 then              -- (flags & 0b0100) != 0
    println("bit set")
end
```

**Compound assignment** exists for four of the five: `&=`, `|=`, `<<=`, `>>=`.

```saule
local bits: integer = 0b0001
bits |= 0b0100                           -- 5
bits <<= 2                               -- 20
```

There is deliberately no `~=`. That spelling is how Lua writes "not equal", which Saule spells `!=`; reading it as xor-assignment would turn a habitual `if a ~= b then` into a silent mutation, so `~=` is left as a syntax error instead. Write `a = a ~ b`.

Classes can overload all six through `OpBAnd`, `OpBOr`, `OpBXor`, `OpShl`, `OpShr` and `OpBNot` — see [Operator Overloading](#operator-overloading).

### `nil` Is a Value, Not a Binding Type

`nil` exists only as the **value** that inhabits a nullable slot. Writing `: nil` as a binding type is rejected so the meaning of the type system stays "every variable has a real type, and `?` says whether it can be empty":

```saule
local nothing: nil = nil       -- ERROR: `nil` is not a valid binding type
local pending: string? = nil   -- ok — `string?` means "string or nil"
```

`nil` is still legal as a **value** (`return nil`, `x = nil`, `match v case nil then …`) and as the conventional `-> nil` return type meaning "this function returns nothing".

### Casting

`as` converts between them, explicitly:

```saule
local health: integer = 100
local dmg: float = 10.5

local result: integer = health - (dmg as integer)   -- dmg truncated to 10
local precise: float = (health as float) - dmg      -- health promoted to 100.0
```

The pairs `as` converts, and nothing else:

| Cast | Result | Rule |
| --- | --- | --- |
| `float as integer` | `integer` | truncates toward zero, no rounding |
| `integer as float` | `float` | always safe, no precision loss |
| `integer` / `float` / `boolean` `as string` | `string` | the text `tostring` gives |
| `string as integer` / `as float` | `integer?` / `float?` | parses; `nil` when the text is not a number |

Two rules keep a cast from ever being decoration:

- **A cast to the type the value already has is an error**, not a no-op.
  `n as integer` on an `integer` does nothing, so the compiler says so.
- **A pair not in the table is an error too.** There is no `integer as
  boolean`: which of `0` and `""` counts as false is a convention, not a
  fact, so the language makes you write the one you meant.

A cast off a nullable value converts the payload and passes `nil` through,
so `maybeFloat as integer` is `integer?` — nil in, nil out.

The old `int()` and `float()` prelude functions still work and mean exactly
what the first two rows mean.

### Escaping `any` with `as`

The same keyword does a second job, and which one it is depends on what is
on the left. On a value whose type is known it converts, as above. On an
`any` there is nothing to convert *from*, so it **tests** instead.

`any` is the one type the checker cannot see through, so it is the one type
that needs a way out. `x as T` there is a **checked** cast: it tests the
value at runtime and evaluates to `T?` — the value when it really is a `T`,
and `nil` when it isn't.

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
- The two readings never blur into each other. A `3.9` that arrives inside
  an `any` is **not** an integer, and `x as integer` says `nil` rather than
  silently truncating it — the truncation is what you get when you cast a
  value the checker already knows is a `float`.
- Every cast is explicit — Saule **never** converts on its own.

```saule
local x: float = 7.9
print(x as integer)          -- 7, not 8: truncation, not rounding

local boxed: any = 7.9
print(boxed as integer ?? -1)   -- -1: a float is not an integer
```

---

## Variables

Saule follows Lua's scoping model with one deliberate departure: `local` makes a binding **lexically scoped** (visible only inside the block / function / file where it was declared), and there are no implicit globals — assigning to a name that was never declared is an error, not a new binding. To publish a name beyond its file, declare it with `export`. The type annotation is optional in either form — when omitted, the type is inferred from the initializer.

### Local (Recommended)

`local` is the workhorse — same lifetime as the surrounding block, no leak into the rest of the program:

```saule
local name: string = "Arthur"
local health: integer = 100
local speed: float = 1.5
local alive: boolean = true
```

### Module Variables

`export name: T = value` declares a variable at module scope — the file-level counterpart of a class's public field. It is visible to every function in the file and importable by name from other modules:

```saule
-- config.sau
export appName: string = "MyGame"
export version = 1                  -- inferred integer

export fn showHeader()
    print(appName .. " v" .. version)   -- visible file-wide
end
```

```saule
-- main.sau
import appName, showHeader from "config"

print(appName)
```

Module variables are mutable, and every write is checked against the declared type:

```saule
version = version + 1        -- ok
version = "two"              -- ERROR: `string` into `integer`
```

Use them sparingly — mutable state reachable from anywhere is the usual source of "where did this value come from?" bugs. A name with no `export` stays private to its file; write those as ordinary `local`s.

There is no implicit-global form. Assigning to a name that was never declared is an error, so a misspelled target is reported instead of silently creating a second variable:

```saule
apName = "MyGame"            -- ERROR: cannot assign to undeclared variable
```

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

The same applies to any name a multiple binding leaves without a value, and to a module variable:

```saule
local host: string, port: integer = "localhost"   -- ERROR: `port` is nil
export appName: string                            -- ERROR: `string` is never nil
```

### Reassignment

`local` introduces the binding once; subsequent writes use plain `name = expr` (no `local`):

```saule
local hp: integer = 100
hp = hp - 25                    -- reassign the local
local hp: integer = 0           -- ERROR: `hp` is already declared in this scope
```

> Class **fields** are a separate thing — they live on instances or the class itself, not in the surrounding scope. They use `name: T = expr` for public and `local name: T = expr` for private. See [Classes → Access Modifiers](#access-modifiers).

### Compound Assignment

`target op= value` reads the target, applies `op`, and writes the result back.
There is one form per arithmetic operator, plus `..=` for concatenation and
`&=` `|=` `<<=` `>>=` for the bitwise ones:

```saule
local hp: integer = 100
hp -= 25                        -- same as `hp = hp - 25`
hp *= 2
hp %= 7

local scale: float = 1.0
scale /= 4.0

local label: string = "level "
label ..= "3"                   -- "level 3"

local charge: integer = 2
charge ^= 10                    -- 1024

local flags: integer = 0b0001
flags |= 0b0100                 -- 5
flags <<= 2                     -- 20
```

Bitwise xor is the one operator with no compound form: `~=` is how Lua spells
"not equal", so reading it as `a = a ~ b` would turn a habitual comparison into
a silent mutation. It is a syntax error instead — write `a = a ~ b`.

The right-hand side is a **full expression**, so it is combined before the
operator applies — `p *= 3 + 4` multiplies by 7, not by 3:

```saule
local p: integer = 2
p *= 3 + 4                      -- 14
```

Any assignable target works — locals, module variables, table elements,
instance fields, and statics:

```saule
local scores: table<integer> = {10, 20}
scores[2] += 5                  -- 25

class Counter
  n: integer
  static total: integer = 0

  fn init()
    self.n = 0
  end

  fn bump()
    self.n += 1
    Counter.total += 1
  end
end
```

The target is evaluated **exactly once**, so a side-effecting subscript or
receiver runs a single time — `queue[next()] += 1` calls `next()` once, not
twice.

Typing follows `target = target op value` exactly: the operator's own operand
rules apply, and the *result* has to fit the target's declared type. Both of
these are compile errors:

```saule
local n: integer = 1
n /= 2.0                        -- ERROR: cannot mix integer and float
n ..= "x"                       -- ERROR: `..` yields a string, `n` is an integer
```

Compound assignment is a **statement**, not an expression — `local x = (y += 1)`
does not parse. It also routes through operator overloads, so a class that
implements `OpAdd` supports `+=` with no extra work:

```saule
local v: Vec = Vec(1, 2)
v += Vec(10, 20)                -- calls Vec.add
```

There is deliberately no compound form for the comparison or logical operators:
`and=` / `or=` would have to answer whether the right-hand side is evaluated
when the operator short-circuits.

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
- Stage targets are resolved as **free functions** in scope (locals, module variables, top-level `fn`). Class methods and lambdas aren't pipeable today.
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

### Field Initialization

The rule for [locals](#nullable-without-initializer) holds for fields too: a non-nullable field is never allowed to start out `nil`. Every field must therefore get its value from one of three places — a default in the declaration, an assignment in `init`, or a `?` on its type:

```saule
class Player
    local name: string = "anon"     -- ok: default
    local level: integer            -- ok: `init` assigns it
    local clan: string?             -- ok: nullable, starts nil

    fn init(level: integer)
        self.level = level
    end
end
```

Leave all three off and the field is reported at compile time:

```saule
class Player
    local level: integer            -- ERROR: never initialized
end
```

That covers a class with no `init` at all (there is nowhere to assign the field) as well as an `init` that forgets one.

Static fields are stricter — nothing runs before the first read of a static, so `init` is not an option and the value has to be in the declaration:

```saule
static local scores: table<integer> = {}    -- ok
static local scores: table<integer>?        -- ok, starts nil
static local scores: table<integer>         -- ERROR: never initialized
```

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

Add `export` before a class, interface, enum, function, or variable to make it accessible from other files:

```saule
export class Player
    -- ...
end

export fn clamp(value: integer, min: integer, max: integer) -> integer
    if value < min then return min end
    if value > max then return max end

    return value
end

export maxPlayers: integer = 8
```

An `export name: T = value` is a **module variable** (see [Variables](#variables)) — a single value shared by every file that imports it, not a copy per importer.

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
min_saule_version: "26.1"
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

### Execution Engines

Saule ships two engines that run the same language, and `saule run` picks the
bytecode VM by default:

```sh
saule run app.sau              # the bytecode VM (default)
saule run --interp app.sau     # the tree-walking interpreter
saule run --vm app.sau         # the VM, stated explicitly
```

`SAULE_ENGINE=vm` and `SAULE_ENGINE=interp` do the same thing for a whole test
run or benchmark sweep without touching call sites.

The two are held to identical observable behaviour — output, exit status and
error text — by a differential harness that runs every fixture and every
example project under both. **`--interp` is an escape hatch, not a tuning
knob**: if a program needs it, that is a bug worth reporting along with the
program.

The bytecode compiler is still learning parts of the language. A module it
cannot compile yet runs on the tree-walking interpreter instead, silently and
with the same result — so the only thing a gap costs is speed. Pass `--vm`
to hear about it:

```
$ saule run --vm .
note: the bytecode compiler does not handle `a tuple pattern` yet — running on the tree-walking interpreter
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

The tables below are for looking something up at a glance. For the exact shape
of every construct, see the [Grammar](#grammar).

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

### Literals

| Literal | Type | Notes |
|---|---|---|
| `42` | `integer` | |
| `0xFF`, `0b1010` | `integer` | `_` may separate digits: `0xFF_80_00` |
| `3.14` | `float` | |
| `.5` | `float` | The integer part may be omitted |
| `10f`, `10F` | `float` | Suffix form of `10.0` |
| `"text"` | `string` | |
| `true`, `false` | `boolean` | |
| `nil` | — | Only assignable to a nullable (`T?`) slot |

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
| `^` | Exponentiation (right-associative, binds tighter than unary `-`) |
| `&`, `\|`, `~`, `<<`, `>>` | Bitwise and / or / xor / shifts — `integer` only |
| `~a` | Bitwise complement (prefix `~`; infix `~` is xor) |
| `+=`, `-=`, `*=`, `/=`, `%=`, `^=`, `..=`, `&=`, `\|=`, `<<=`, `>>=` | Compound assignment (no `~=` — that is Lua's `!=`) |
| `:` | Pipeline stage call inside `when(...)` |
| `int()` | Cast float to integer, truncates toward zero |
| `float()` | Cast integer to float, always safe |

## Grammar

The complete syntax of Saule, in the notation the Lua reference manual uses:
`{a}` means zero or more `a`, `[a]` means an optional `a`, `|` separates
alternatives, and quoted text is literal. Names in `Title` case are lexical
tokens defined at the end.

It is transcribed from the recursive-descent parser in `crates/saule-parser`,
so it describes what the compiler actually accepts — including the corners the
guide simplifies.

### Chunks and statements

```ebnf
chunk ::= {stat}

stat ::= ';'
       | local
       | assign
       | compoundAssign
       | exp
       | if
       | while
       | repeat
       | forNum
       | forIn
       | try
       | 'return' [explist]
       | 'throw' exp
       | 'break'
       | 'continue'
       | decl

local  ::= 'local' nameDecl {',' nameDecl} ['=' explist]
assign ::= exp {',' exp} '=' explist

compoundAssign ::= exp compoundOp exp
compoundOp     ::= '+=' | '-=' | '*=' | '/=' | '%=' | '^=' | '..='
                 | '&=' | '|=' | '<<=' | '>>='

if     ::= 'if' exp 'then' chunk
           {'elseif' exp 'then' chunk}
           ['else' chunk] 'end'
while  ::= 'while' exp 'do' chunk 'end'
repeat ::= 'repeat' chunk 'until' exp
forNum ::= 'for' nameDecl '=' exp ',' exp [',' exp] 'do' chunk 'end'
forIn  ::= 'for' nameDecl {',' nameDecl} 'in' exp 'do' chunk 'end'
try    ::= 'try' chunk 'catch' Name ':' type chunk 'end'

nameDecl ::= Name [':' type]
explist  ::= exp {',' exp}
```

An assignment target is parsed as a full expression; whether it is something
you can actually assign to is decided later, by the semantic pass.

### Declarations

```ebnf
decl ::= ['export'] (function | class | interface | enum)
       | 'local' function
       | import

function  ::= 'fn' Name [typeParams] params ['->' type] chunk 'end'

class     ::= 'class' Name [typeArgs]
              ['extends' Name [typeArgs]]
              ['implements' Name [typeArgs] {',' Name [typeArgs]}]
              {member} 'end'
member    ::= modifiers (method | field)
modifiers ::= ['static'] ['local'] | ['local'] ['static']
method    ::= 'fn' Name [typeParams] params ['->' type] chunk 'end'
field     ::= Name ':' type ['=' exp]

interface ::= 'interface' Name [typeArgs]
              ['extends' Name [typeArgs] {',' Name [typeArgs]}]
              {methodSig} 'end'
methodSig ::= 'fn' Name [typeArgs] params ['->' type]

enum      ::= 'enum' Name {variant} {enumMethod} 'end'
variant   ::= [','] Name ['=' exp | params]
enumMethod::= 'fn' Name params ['->' type] chunk 'end'

import    ::= 'import' ('*' | importName {',' importName})
              'from' (String | Name {'.' Name})
importName::= Name ['as' Name]

params ::= '(' [param {',' param}] ')'
param  ::= ['...'] (Name | 'self') [':' type] ['=' exp]
```

`export` applies only to the four declaration forms listed; there is no
`export local`. On a declared `fn` or method every parameter needs its type —
the `[':' type]` above is optional only inside a lambda, where the target type
supplies it, and on `self`, which is typed as the enclosing class.

### Types

```ebnf
type      ::= baseType ['?']
baseType  ::= Name [typeArgs]
            | 'table' '<' type [',' type] '>'
            | 'nil'
            | 'fn' '(' [type {',' type}] ')' '->' type
            | '(' [type {',' type}] ')'

typeArgs   ::= '<' type {',' type} '>'
typeParams ::= '<' Name {',' Name} '>'
```

A parenthesised list of one type is just grouping; two or more is a tuple,
which is how a function returning multiple values states its return type.
`table<V>` is the array form and `table<K, V>` the map form.

### Expressions

Written as a precedence ladder, loosest binding first, because that is how the
parser reads them. Each level is left-associative unless marked otherwise.

```ebnf
exp        ::= orExp
orExp      ::= andExp {'or' andExp}
andExp     ::= eqExp {'and' eqExp}
eqExp      ::= cmpExp {('==' | '!=') cmpExp}
cmpExp     ::= borExp {('<' | '<=' | '>' | '>=') borExp}
borExp     ::= bxorExp {'|' bxorExp}
bxorExp    ::= bandExp {'~' bandExp}
bandExp    ::= shiftExp {'&' shiftExp}
shiftExp   ::= coalesce {('<<' | '>>') coalesce}
coalesce   ::= concat ['??' coalesce]                    (* right *)
concat     ::= additive ['..' concat]                    (* right *)
additive   ::= multiply {('+' | '-') multiply}
multiply   ::= unary {('*' | '/' | '%') unary}
unary      ::= ('-' | 'not' | '#' | '~') unary | power
power      ::= cast ['^' unary]                          (* right *)
cast       ::= postfix {'as' type}
postfix    ::= primary {suffix}

suffix ::= '.' (Name | 'super')
         | '?.' Name
         | '[' exp ']'
         | [typeArgs] args
         | 'do' [params ['->' type]] chunk 'end'
         | '!'

args ::= '(' [arg {',' arg}] ')'
arg  ::= [Name ':'] exp

primary ::= Numeral | String | 'true' | 'false' | 'nil' | 'self'
          | Name
          | table
          | lambda
          | match
          | pipeline
          | '(' exp ')'

table ::= '{' [entry {',' entry} [',']] '}'
entry ::= Name ':' exp | String ':' exp | exp

lambda ::= 'fn' params ['->' type] chunk 'end'
         | '(' [param {',' param}] ')' ['->' type] '=>' exp
         | Name '=>' exp

match   ::= 'match' exp {arm} 'end'
arm     ::= 'case' pattern ['when' exp] 'then' (exp | chunk)
pattern ::= 'nil' | 'true' | 'false' | ['-'] Numeral | String
          | '_'
          | Name
          | Name '.' Name ['(' [pattern {',' pattern}] ')']
          | '(' [pattern {',' pattern}] ')'

pipeline ::= 'when' '(' exp ')' stage {stage}
stage    ::= ':' Name [typeArgs] args
```

Four parts of this need a word of explanation.

**`^` binds tighter than unary minus**, as in Lua: `-2 ^ 2` is `-(2 ^ 2)`, and
because the right operand is itself `unary`, `2 ^ -1` parses.

**The bitwise rungs sit where Lua 5.3 puts them** — just above comparison, in
the order `|`, `~`, `&`, shifts — so `flags & mask != 0` masks before it
compares, and `..`, `+` and `*` all bind tighter than a shift.

**`as` binds tighter than every binary operator but looser than the postfix
chain.** So `y as integer ?? 0` casts before it coalesces, and
`obj.field() as string` casts the call's result rather than the callee.

**The `do … end` suffix is the trailing-block form**, sugar for passing a
lambda as the final positional argument: `Panel(title: "x") do … end` is
`Panel(title: "x", fn() … end)`. It attaches only to something that is already
a call, and it is suppressed while parsing the header of a `while` or `for`,
where a `do` closes the header instead. Parenthesising re-enables it:
`while (next() do … end) do … end`.

There is no `obj:method()` call form. A method call is `obj.method()`; `:`
appears only in type ascriptions, named arguments, table keys, `catch`
bindings, and pipeline stages.

### Lexical elements

```ebnf
Name    ::= ('_' | letter) {'_' | letter | digit}
Numeral ::= integer | float
integer ::= digit {digit}
          | '0' ('x' | 'X') hexDigit {'_' | hexDigit}
          | '0' ('b' | 'B') binDigit {'_' | binDigit}
float   ::= digit {digit} '.' digit {digit} [fSuffix]
          | '.' digit {digit} [fSuffix]
          | digit {digit} fSuffix
fSuffix ::= 'f' | 'F'
String  ::= '"' {stringChar | escape} '"'
          | "'" {stringChar | escape} "'"
escape  ::= '\n' | '\t' | '\r' | '\0' | '\\' | '\"' | "\'"
comment ::= '--' {any} newline | '--[[' {any} ']]'
```

`Name` is ASCII only, and excludes the keywords:

```
and       as        break     case      catch     class     continue  do
else      elseif    end       enum      export    extends   false     fn
for       from      if        implements import    in        interface local
match     nil       not       or        repeat    return    self      static
super     then      throw     true      try       until     when      while
```

Strings take either quote style, and only the delimiter that opened one closes
it — so `'he said "hi"'` needs no escaping. The seven escapes above are the only
ones recognised; anything else after a backslash is an error, and there is no
hex or unicode escape. `_` groups digits in
hex and binary literals only. There is no exponent notation, no octal form, and
no `f` suffix on a hex literal (`0xFF_80f` is one hexadecimal integer, since
`f` is a hex digit).

Whitespace and comments separate tokens and are otherwise insignificant. `;` is
a separator, never a statement: it is accepted between statements, at the end of
a block, and at the end of a file, and is never required anywhere.
