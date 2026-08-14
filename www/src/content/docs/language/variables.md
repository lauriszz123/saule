---
title: "Variables"
description: "Saule follows Lua's scoping model with one deliberate departure: local makes a binding lexically scoped (visible only inside the block / function /…"
sidebar:
  order: 2
---

<!-- Generated from README.md by `npm run sync-docs`. Edit that file, not this one. -->

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

> Class **fields** are a separate thing — they live on instances or the class itself, not in the surrounding scope. They use `name: T = expr` for public and `local name: T = expr` for private. See [Classes → Access Modifiers](/saule/language/classes/#access-modifiers).

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
