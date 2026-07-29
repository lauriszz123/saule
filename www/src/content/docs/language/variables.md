---
title: "Variables"
description: "Saule follows Lua's scoping model: local makes a binding lexically scoped (visible only inside the block / function / file where it was declared), and…"
sidebar:
  order: 2
---

<!-- Generated from README.md by `npm run sync-docs`. Edit that file, not this one. -->

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

> Class **fields** are a separate thing — they live on instances or the class itself, not in the surrounding scope. They use `name: T = expr` for public and `local name: T = expr` for private. See [Classes → Access Modifiers](/saule/language/classes/#access-modifiers).

---
