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

---
