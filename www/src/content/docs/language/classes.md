---
title: "Classes"
description: "Each class lives in its own .sau file. Fields are declared at the top, followed by an fn init method (the constructor) and the rest of the methods."
sidebar:
  order: 6
---

<!-- Generated from README.md by `npm run sync-docs`. Edit that file, not this one. -->

### Declaring a Class

Each class lives in its own `.sau` file. Fields are declared at the top, followed by an `fn init` method (the constructor) and the rest of the methods.

```saule
export class Player
    local name: string
    local health: integer
    local speed: float

    static maxHealth: integer = 100

    fn init(name: string, health: integer, speed: float)
        self.name = name
        self.health = health
        self.speed = speed
    end

    fn greet()
        print("Hi, I am " .. self.name)
    end

    fn damage(amount: integer)
        self.health = self.health - amount
    end

    fn isAlive() -> boolean
        return self.health > 0
    end

    local fn secret()
        print("This is private")
    end

    static fn getMaxHealth() -> integer
        return maxHealth
    end
end
```

`fn init` is the **only** constructor — there is no `constructor` keyword.

### Instantiation

Call the class as if it were a function:
```saule
local p: Player = Player("Arthur", 100, 5)

p.greet()
```

### Implicit `self`

Inside a method body, `self` is always in scope if it is a non `static` and in an **instance method** (and `init`), so `self` is the instance.

In addition, every class member — static fields, static methods, instance methods — is reachable by its **bare name** from inside any method of the same class. Local variables and parameters can shadow them, which is what you want.

```saule
class Counter
    local count: integer = 0

    static local cap: integer = 10

    fn tick()
        if self.count >= cap then
            return
        end

        self.count = self.count + 1
        self.report()
    end

    fn report()
        print("Count is " .. self.count)
    end
end
```

### Access Modifiers

| Syntax | Access |
|---|---|
| `name: string` | public field |
| `local name: string` | private field |
| `fn method()` | public method |
| `local fn method()` | private method |
| `static field: T = value` | class-level, shared, public |
| `static local field: T = value` | class-level, shared, private |

### Field Initialization

The rule for [locals](/saule/language/variables/#nullable-without-initializer) holds for fields too: a non-nullable field is never allowed to start out `nil`. Every field must therefore get its value from one of three places — a default in the declaration, an assignment in `init`, or a `?` on its type:

```saule
class Player
    local name: string = "anon"     -- ok: default
    local level: integer            -- ok: `init` assigns it
    local clan: string?             -- ok: nullable, starts nil

    fn init(level: integer)
        self.level = level
    end
end
```

Leave all three off and the field is reported at compile time:

```saule
class Player
    local level: integer            -- ERROR: never initialized
end
```

That covers a class with no `init` at all (there is nowhere to assign the field) as well as an `init` that forgets one.

Static fields are stricter — nothing runs before the first read of a static, so `init` is not an option and the value has to be in the declaration:

```saule
static local scores: table<integer> = {}    -- ok
static local scores: table<integer>?        -- ok, starts nil
static local scores: table<integer>         -- ERROR: never initialized
```

### Static Members

Static fields and methods belong to the class itself, not to instances. They are accessed via the class name from the outside, or by bare name (or `self.name` in a `static fn`) from inside:

```saule
print(Player.maxHealth)         -- 100
print(Player.getMaxHealth())    -- 100
```

Static fields are shared across all instances. Modifying them affects the class globally:

```saule
Player.maxHealth = 200
```

A class with **no** `fn init` promotes every `local field = expr` to a static, evaluated once at class-declaration time. This makes a class usable as a small module:

```saule
class Main
    static local lauris: Person = Person("Laurynas")

    static fn main()
        lauris.introduce()     -- `lauris` resolves via the class
    end
end
```

### Inheritance

Use `extends` to inherit from another class. Call the parent's `init` with `self.super(...)` from inside `init`:

```saule
export class Entity
    name: string

    fn init(name: string)
        self.name = name
    end

    fn getName() -> string
        return self.name
    end
end

export class Player extends Entity
    local health: integer
    local speed: float

    fn init(name: string, health: integer, speed: float)
        self.super(name)

        self.health = health
        self.speed = speed
    end

    fn greet()
        print("Hi, I am " .. self.getName())
    end
end
```

Rules:
- A class can only extend **one** parent
- Private members from the parent are **not accessible** in the child
- Public and static members are inherited

### Overriding

Redeclaring an inherited method overrides it. The override has to be usable
everywhere the parent's version was, because a caller holding the parent type
cannot know which one it will get. The checker enforces:

- **Same parameter count and types.** Widening or renaming the shape of the
  call is a compile error.
- **A return type the parent's callers accept.** Narrowing is fine — an
  override may return a *subclass* of what the parent declared. Returning
  something unrelated is a compile error.
- **Instance stays instance, static stays static.**

```saule
class Base
    fn get() -> integer
        return 1
    end
end

class Derived extends Base
    fn get() -> string        -- ERROR: the parent returns `integer`
        return "oops"
    end
end
```

`fn init` is exempt — constructors aren't dispatched through a parent
reference, so a subclass constructor may take whatever parameters it needs
and forward what the parent wants via `self.super(...)`.

If a method wasn't meant to override anything, give it a different name.

### Generic Classes

A class takes type parameters after its name, and they are in scope for every field, method signature and body inside it:

```saule
class Box<T>
    value: T

    fn init(value: T)
        self.value = value
    end

    fn get() -> T
        return self.value
    end
end

local ints: Box<integer> = Box(5)
local n: integer = ints.get()       -- `T` is `integer` here

local words: Box<string> = Box("hi")
local s: string = words.get()       -- and `string` here
```

The argument is **inferred from the constructor** when you don't write it, so `local b = Box(5)` gives a `Box<integer>` and `b.get() + 1` type-checks. Several parameters bind independently, each from the position it appears in:

```saule
class Pair<A, B>
    left: A
    right: B

    fn init(left: A, right: B)
        self.left = left
        self.right = right
    end

    fn first() -> A
        return self.left
    end
end

local p = Pair(7, "seven")          -- Pair<integer, string>
local n: integer = p.first()
```

Type arguments are **invariant**, the same rule [table elements](/saule/language/tables/#element-types-are-invariant) follow and for the same reason: a `Box<string>` accepted into a `Box<integer>` slot would be an alias through which the wrong type could be written back.

```saule
local b: Box<integer> = Box("no")   -- ERROR: Box<string> is not Box<integer>
local c: Box<integer, string> = ... -- ERROR: `Box` expects 1 type argument
```

Naming the class **without** its arguments means "some instantiation, unknown which", and is accepted against any of them:

```saule
local any: Box = Box(1)             -- ok
local back: Box<integer> = any      -- ok
```

Like a function's, a class's type parameters are erased at runtime — they constrain the program, they don't reach it.

---
