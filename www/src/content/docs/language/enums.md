---
title: "Enums"
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

---
