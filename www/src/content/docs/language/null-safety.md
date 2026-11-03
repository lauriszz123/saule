---
title: "Null Safety"
description: "Saule enforces null safety at compile time. A type is only nullable if declared with ?."
sidebar:
  order: 10
---

<!-- Generated from README.md by `npm run sync-docs`. Edit that file, not this one. -->

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

For lengths of strings, bytes and tables, use `#`:

```saule
local greeting: string? = nil
local len: integer = #(greeting ?? "")     -- 0 when nil
```

### Safe Index

`?[...]` is the index half of the same idea — `nil` when the receiver is nil, the element otherwise:

```saule
local rows: table<integer>? = nil
println(rows?[1] ?? 0)              -- 0, no fault on the nil receiver
```

The index expression is not evaluated at all on the nil path, the same rule `?.` follows for a call's arguments, and the two chain together:

```saule
println(grid.rows?[1]?[2] ?? -1)
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
