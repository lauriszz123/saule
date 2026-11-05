---
title: "Pattern Matching"
description: "match selects one of several branches by structurally inspecting a value. Unlike a C-style switch, every arm is independent (no fall-through), patterns…"
sidebar:
  order: 9
---

<!-- Generated from LANGUAGE.md by `npm run sync-docs`. Edit that file, not this one. -->

`match` selects one of several branches by structurally inspecting a value. Unlike a C-style `switch`, every arm is independent (no fall-through), patterns can **bind** parts of the value, and the typechecker requires the arms to be **exhaustive** — so a forgotten enum variant or unhandled `nil` is a compile error, not a production incident.

`match` is also an **expression**: every arm produces a value of the same type, and the whole `match` evaluates to that value.

### Basic Match

```saule
enum Status
    Ok
    Warn
    Error
end

local status: Status = Status.Warn

local label: string = match status
    case Status.Ok then "fine"
    case Status.Warn then "watch out"
    case Status.Error then "oops"
end

println(label)                  -- watch out
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
class User
    name: string = ""
    fn init(name: string)
        self.name = name
    end
end

class Repo
    fn findById(id: integer) -> User?
        if id == 1 then
            return User("Ada")
        end
        return nil
    end
end

local repo: Repo = Repo()
local id: integer = 1

match repo.findById(id)
    case nil then println("not found")
    case user then println("hi " .. user.name)    -- hi Ada
end
```

The bare name `user` here is a **binding pattern**: it captures the non-nil value. Inside that arm `user` has type `Player`, not `Player?`.

### Wildcard and Bindings

Use `_` to ignore a value and any identifier to bind it:

```saule
local n: integer = 7

match n
    case 0 then println("zero")
    case 1 then println("one")
    case other then println("something else: " .. other)   -- something else: 7
end

fn pair() -> (integer, integer)
    return 0, 5
end

match pair()
    case (0, _) then println("x is zero")      -- x is zero
    case (_, 0) then println("y is zero")
    case _ then println("neither")
end
```

A pattern that starts with a lowercase identifier (and isn't an enum variant) binds; `_` matches anything without binding.

### Guards with `when`

Add a condition that must also hold for the arm to fire:

```saule
local n: integer = 5

match n
    case x when x < 0 then println("negative")
    case 0 then println("zero")
    case x then println("positive " .. x)      -- positive 5
end
```

Guards are evaluated **after** the pattern matches. If the guard is false, matching continues with the next arm.

### Literal Patterns

Numbers, strings, booleans, and `nil` are all valid patterns:

```saule
fn showHelp()
    println("help")
end

local command: string = "help"

match command
    case "quit" then println("bye")
    case "help" then showHelp()                -- help
    case other then println("unknown: " .. other)
end
```

### Multiple Return Values

Match destructures tuples returned by multi-value functions:

```saule
fn divmod(a: integer, b: integer) -> (integer, integer)
    return a / b, a % b
end

local a: integer = 17
local b: integer = 5

match divmod(a, b)
    case (q, 0) then println("clean: " .. q)
    case (q, r) then println(q .. " rem " .. r)   -- 3 rem 2
end
```

### Exhaustiveness

The typechecker verifies that **every possible value** of the scrutinee is covered:

```saule
enum Color
    Red, Green, Blue
end

local c: Color = Color.Red

local name: string = match c   -- ERROR: non-exhaustive, missing `Color.Blue`
    case Color.Red then "red"
    case Color.Green then "green"
end
```

Add the missing variant or a wildcard arm to fix it. For nullables, both `nil` and a binding (or `_`) must be covered. Guards never count toward exhaustiveness — if every arm has a guard, you still need a wildcard fallback.

### As an Expression

Because `match` produces a value, it composes naturally with `local`, `return`, and arguments:

```saule
enum Tier
    Free
    Pro
    Enterprise
end

fn priceFor(tier: Tier) -> integer
    return match tier
        case Tier.Free then 0
        case Tier.Pro then 9
        case Tier.Enterprise then 99
    end
end

println(priceFor(Tier.Pro))     -- 9
```

If used as a statement, the result is simply discarded — same rule as any other expression.

---
