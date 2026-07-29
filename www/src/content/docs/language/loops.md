---
title: "Loops"
description: "Runs at least once, checks the condition at the end:"
sidebar:
  order: 12
---

<!-- Generated from README.md by `npm run sync-docs`. Edit that file, not this one. -->

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
