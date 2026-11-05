---
title: "Null Safety"
description: "Saule enforces null safety at compile time. A type is only nullable if declared with ?."
sidebar:
  order: 10
---

<!-- Generated from LANGUAGE.md by `npm run sync-docs`. Edit that file, not this one. -->

Saule enforces null safety at compile time. A type is only nullable if declared with `?`.

```saule
local name: string? = nil       -- ok, nullable
local name: string = nil        -- ERROR, string is never nil
```

### Safe Access

Use `?.` to access a member that may be nil. Returns nil instead of crashing:

```saule
class Player
    name: string = ""
    fn init(name: string)
        self.name = name
    end
end

class Repo
    fn findById(id: integer) -> Player?
        if id == 1 then
            return Player("Ada")
        end
        return nil
    end
end

local repo: Repo = Repo()
local id: integer = 2

local player: Player? = repo.findById(id)
local name: string? = player?.name      -- nil if `player` was nil
println(name ?? "<none>")               -- <none>
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
class Grid
    rows: table<table<integer>>? = nil
end

local grid: Grid = Grid()
println(grid.rows?[1]?[2] ?? -1)        -- -1, nothing to index

grid.rows = {{7, 8}}
println(grid.rows?[1]?[2] ?? -1)        -- 8
```

### Null Coalescing

Use `??` to provide a fallback when a value is nil:

```saule
local name: string? = nil
local display: string = name ?? "Unknown"
println(display)                        -- Unknown
```

### Force Unwrap

Use `!` to assert a value is not nil. Crashes at runtime if it is:

```saule
local name: string? = "Arthur"
local forced: string = name!
println(forced)                         -- Arthur
```

### Combining Operators

```saule
class Player
    name: string = ""
    fn init(name: string)
        self.name = name
    end
    fn getName() -> string
        return self.name
    end
end

fn getPlayers() -> table<Player?>
    return {Player("Ada"), nil}
end

local players: table<Player?> = getPlayers()

for player: Player? in players do
    local name: string = player?.getName() ?? "Unknown"

    println(name)                       -- Ada, then Unknown
end
```

---
