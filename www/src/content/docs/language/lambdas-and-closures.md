---
title: "Lambdas and Closures"
description: "Single expression, most common form:"
sidebar:
  order: 5
---

<!-- Generated from README.md by `npm run sync-docs`. Edit that file, not this one. -->

### Arrow Lambda

Single expression, most common form:

```saule
local double: fn(integer) -> integer = x => x * 2
local square: fn(integer) -> integer = x => x * x
local greet: fn(string) -> nil       = name => print("Hi " .. name)
local half: fn(float) -> float       = x => x / 2.0
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

---
