---
title: "Enums"
description: "An enum takes type parameters the same way, and a variant's payload may be typed by one. This is what makes a Result worth writing: the arm that…"
sidebar:
  order: 8
---

<!-- Generated from README.md by `npm run sync-docs`. Edit that file, not this one. -->

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

### Generic Enums

An enum takes type parameters the same way, and a variant's payload may be typed by one. This is what makes a `Result` worth writing: the arm that matches `Ok` binds a real `T`, not an `any` you have to cast:

```saule
enum Result<T>
    Ok(value: T),
    Err(message: string)
end

local r: Result<integer> = Result.Ok(5)

local n: integer = match r
    case Result.Ok(v) then v + 1    -- `v` is an `integer`
    case Result.Err(m) then 0
end
```

The instantiation comes from the construction where the payload pins it down, and from the annotation where it doesn't — `Result.Err("boom")` says nothing about `T`, so it fits any `Result`:

```saule
local inferred = Result.Ok("hi")            -- Result<string>
local failed: Result<integer> = Result.Err("boom")
```

Exhaustiveness is unaffected: type arguments say what the payloads hold, never which variants exist.

---
