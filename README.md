# Saule Programming Language

Saule is a statically typed, class-oriented language inspired by Lua's simplicity and runtime model, designed to be minimal to write but powerful to use. It compiles to clean, readable code and is structured around files, classes, interfaces, and scripts.

---

## Table of Contents

- [Types](#types)
- [Variables](#variables)
- [Functions](#functions)
- [Lambdas and Closures](#lambdas-and-closures)
- [Classes](#classes)
- [Interfaces](#interfaces)
- [Enums](#enums)
- [Null Safety](#null-safety)
- [Error Handling](#error-handling)
- [Loops](#loops)
- [Imports and File Structure](#imports-and-file-structure)
- [Project Configuration](#project-configuration)

---

## Types

Saule has 9 primitive types, inherited from Lua's type system but statically declared. Lua's single `number` type is split into two distinct types in Saule:

| Type | Description |
|---|---|
| `integer` | Whole numbers, no decimal component |
| `float` | Decimal numbers, 64-bit precision |
| `string` | Immutable sequences of characters |
| `boolean` | `true` or `false` |
| `nil` | Absence of value |
| `function` | First-class function values |
| `table<T>` | The only data structure, typed generically |
| `userdata` | Raw memory for native integrations |
| `thread` | Coroutines |

All types must be declared. A variable cannot be `nil` unless its type is marked nullable with `?`.

### Integer vs Float

Use `integer` for whole values like counts, indices, and health. Use `float` for precision values like position, speed, and ratios:

```saule
local health: integer = 100
local speed: float = 3.14
local index: integer = 1
local ratio: float = 0.75
```

Mixing `integer` and `float` directly is a **compile error**:

```saule
local health: integer = 100
local dmg: float = 10.5
local result = health - dmg    -- ERROR: cannot mix integer and float
```

### Casting

Use `int()` and `float()` to explicitly convert between the two:

```saule
local health: integer = 100
local dmg: float = 10.5

local result: integer = health - int(dmg)       -- dmg truncated to 10
local precise: float = float(health) - dmg      -- health promoted to 100.0
```

Casting rules:
- `int(float)` — truncates toward zero, no rounding
- `float(integer)` — always safe, no precision loss
- Both are explicit — Saule **never** casts silently

```saule
local x: float = 7.9
print(int(x))    -- 7, not 8, truncation not rounding
```

---

## Variables

```saule
local name: string = "Arthur"
local health: integer = 100
local speed: float = 1.5
local alive: boolean = true
local nothing: nil = nil
```

Variables are declared with `local` followed by a name, type annotation, and value.

---

## Functions

### Basic Functions

```saule
fn add(a: integer, b: integer) -> integer
    return a + b
end

fn average(a: float, b: float) -> float
    return (a + b) / 2.0
end

fn greet(name: string) -> nil
    print("Hello, " .. name)
end
```

### Default Parameters

```saule
fn createPlayer(name: string, health: integer = 100, score: integer = 0) -> Player
    return Player(name, health, score)
end

local p: Player = createPlayer("Arthur")         -- health=100, score=0
local p: Player = createPlayer("Arthur", 50)     -- health=50, score=0
```

### Named Parameters

```saule
fn setupGame(width: integer, height: integer, title: string, fullscreen: boolean = false) -> nil
    -- ...
end

setupGame(width: 1920, height: 1080, title: "My Game", fullscreen: true)
```

### Multiple Return Values

```saule
fn minMax(items: table<integer>) -> (integer, integer)
    local min: integer = items[1]
    local max: integer = items[1]
    for val: integer in items do
        if val < min then min = val end
        if val > max then max = val end
    end
    return min, max
end

local min: integer, max: integer = minMax({3, 1, 7, 2, 9})
```

### Variadic Functions

```saule
fn sum(...values: integer) -> integer
    local total: integer = 0

    for v: integer in values do
        total = total + v
    end

    return total
end

print(sum(1, 2, 3, 4, 5))    -- 15
```

### Generic Functions

```saule
fn filter<T>(items: table<T>, predicate: fn(T) -> boolean) -> table<T>
    local result: table<T> = {}

    for item: T in items do
        if predicate(item) then
            result[#result + 1] = item
        end
    end

    return result
end

local nums: table<integer> = {1, 2, 3, 4, 5, 6}
local evens: table<integer> = filter<integer>(nums, x => x % 2 == 0)
```

### Piping with `then`

The `then` keyword passes the result of the left side as the first argument of the right side, enabling clean data transformation chains that read top to bottom:

```saule
local result: table<integer> = {1, 2, 3, 4, 5, 6, 7, 8, 9, 10}
    then filter<integer>(x => x % 2 == 0)
    then map(x => x * x)
    then map(x => x + 1)
```

Each `then` takes the result of the previous step and passes it as the first argument of the next function. It reads like a sentence — take this, then do that, then do this:

```saule
local name: string = getRawInput()
    then trim()
    then toUpperCase()
    then format("Player: %s")

print(name)    -- "Player: ARTHUR"
```

---

## Lambdas and Closures

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
local double: fn(integer) -> integer = fn => (x)
    return x * 2
end
```

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

    return fn => ()
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

## Classes

### Declaring a Class

Each class lives in its own `.saule` file. Fields are declared at the top, followed by an `init` method (the constructor) and the rest of the methods. Methods **do not declare `self`** — it is bound implicitly inside every method body.

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

`init` is the canonical constructor name.

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

        self.count = count + 1
        self.report()
    end

    fn report()
        print("Count is " .. count)
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

A class with **no** constructor (no `init` and no `constructor`) promotes every `local field = expr` to a static, evaluated once at class-declaration time. This makes a class usable as a small module:

```saule
class Main
    static local lauris: Person = Person("Laurynas")

    static fn main()
        lauris.introduce()     -- `lauris` resolves via the class
    end
end
```

### Inheritance

Use `extends` to inherit from another class. Call the parent constructor with `self.super(...)` from inside `init`:

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

---

## Interfaces

Interfaces define a contract — method signatures only, no fields, no bodies.

### Declaring an Interface

```saule
interface Greetable
    fn greet() -> nil
end

interface Damageable
    fn damage(amount: integer) -> nil
    fn isAlive() -> boolean
end
```

### Implementing an Interface

A class can implement multiple interfaces:

```saule
export class Player extends Entity implements Greetable, Damageable
    local health: integer

    fn init(name: string, health: integer)
        self.super(name)

        self.health = health
    end

    fn greet()
        print("Hi, I am " .. self.getName())
    end

    fn damage(amount: integer)
        self.health = self.health - amount
    end

    fn isAlive() -> boolean
        return self.health > 0
    end
end
```

### Interface Composition

Interfaces can extend other interfaces:

```saule
interface Combatant extends Damageable
    fn attack(target: Damageable) -> nil
end
```

### Interfaces as Types

This is the main power — use interfaces as parameter and variable types:

```saule
fn processEntity(target: Damageable, amount: integer) -> nil
    if target.isAlive() then
        target.damage(amount)
    end
end

local p: Player = Player("Arthur", 100)
processEntity(p, 30)    -- works, Player implements Damageable
```

### Generic Interfaces

```saule
interface Repository<T>
    fn save(item: T) -> nil
    fn findById(id: integer) -> T
    fn delete(id: integer) -> nil
end

export class PlayerRepository implements Repository<Player>
    local items: table<Player>

    fn init()
        self.items = {}
    end

    fn save(item: Player)
        self.items[#self.items + 1] = item
    end

    fn findById(id: integer) -> Player
        return self.items[id]
    end

    fn delete(id: integer)
        self.items[id] = nil
    end
end
```

### Custom Iterable

Any class implementing `Iterable<T>` works inside a `for-in` loop automatically:

```saule
interface Iterable<T>
    fn hasNext() -> boolean
    fn next() -> T
end

export class PlayerQueue implements Iterable<Player>
    local items: table<Player>
    local cursor: integer

    fn init()
        self.items = {}
        self.cursor = 1
    end

    fn hasNext() -> boolean
        return self.cursor <= #self.items
    end

    fn next() -> Player
        local p: Player = self.items[self.cursor]
        self.cursor = self.cursor + 1
        return p
    end
end

local queue: PlayerQueue = PlayerQueue()

for player: Player in queue do
    player.greet()
end
```

---

## Enums

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

## Null Safety

Saule enforces null safety at compile time. A type is only nullable if declared with `?`.

```saule
local name: string? = nil       -- ok, nullable
local name: string = nil        -- ERROR, string is never nil
```

### Safe Access

Use `?.` to access a member that may be nil. Returns nil instead of crashing:

```saule
local len: integer? = name?.length
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

## Error Handling

Saule has two layers of error handling: `try/catch` for unexpected crashes, and `Result<T>` for expected, recoverable failures.

### Throwing Errors

```saule
fn damage(amount: integer)
    if amount < 0 then
        throw "Damage cannot be negative"
    end

    self.health = self.health - amount
end
```

### Try / Catch

```saule
try
    local p: Player = new Player("Arthur", 100)
    p:damage(-10)
catch e: string
    print("Caught: " .. e)
end
```

### Result\<T\>

For functions that can fail gracefully without throwing. Returns either a value or an error message:

```saule
fn findPlayer(id: integer) -> Result<Player>
    if id < 0 then
        return Result.err("Invalid ID")
    end
    return Result.ok(self.items[id])
end

local result: Result<Player> = repo.findPlayer(5)

if result.ok then
    print(result.value.getName())
else
    print("Error: " .. result.error)
end
```

### Combining Result with Null Safety

```saule
local result: Result<Player> = repo.findPlayer(5)
local name: string = result.value?.getName() ?? "Unknown"
```

Use `Result<T>` for expected failures like missing data or invalid input. Use `try/catch` for truly unexpected runtime errors.

---

## Loops

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

## Imports and File Structure

### Importing

```saule
-- single import
import Player from "entities/Player"

-- multiple from same folder
import Player, Enemy from "entities"

-- import everything from a folder
import * from "entities"

-- import with alias
import PlayerRepository as PlayerRepo from "data/PlayerRepository"

-- import a utility module
import Math from "utils/Math"
```

### Exporting

Add `export` before a class, interface, enum, or function to make it accessible from other files:

```saule
export class Player
    -- ...
end

export fn clamp(value: integer, min: integer, max: integer) -> integer
    if value < min then return min end
    if value > max then return max end

    return value
end
```

A file without `export` is private to its folder.

### Utility Modules

Not everything needs a class. Export standalone functions from a utility file:

```saule
-- utils/Math.saule

export fn clamp(value: integer, min: integer, max: integer) -> integer
    if value < min then return min end
    if value > max then return max end
    return value
end

export fn lerp(a: float, b: float, t: float) -> float
    return a + (b - a) * t
end
```

```saule
import Math from "utils/Math"

local clamped: integer = Math.clamp(150, 0, 100)   -- 100
local smooth: float = Math.lerp(0.0, 1.0, 0.5)    -- 0.5
```

### Visibility Rules

| Situation | Accessible from |
|---|---|
| `export class Foo` | anywhere that imports it |
| `class Foo` without export | only within the same folder |
| `local` field or method | only within the class |
| `static` field or method | via `ClassName.x` anywhere |

### Circular Imports

Saule forbids circular imports at compile time:

```
ERROR: Circular import detected
  Player.saule → Inventory.saule → Player.saule

  Hint: Extract shared types into a separate file
```

---

## Project Configuration

Every Saule project has a `saule.config` file at the root:

```
name: "myproject"
version: "1.0.0"
entry: "main.saule"
author: "Arthur"
```

### Recommended Project Structure

```
myproject/
├── saule.config
├── main.saule
├── entities/
│   ├── Entity.saule
│   ├── Player.saule
│   └── Enemy.saule
├── data/
│   ├── Repository.saule
│   └── PlayerRepository.saule
├── utils/
│   ├── Math.saule
│   └── Logger.saule
└── enums/
    ├── Direction.saule
    └── Status.saule
```

### Entry Point

`main.saule` is a script that runs top to bottom, like a Lua script. No class needed:

```saule
import Player from "entities/Player"
import Math from "utils/Math"
import Direction from "enums/Direction"

local p: Player = Player("Arthur", 100, 1.5)
p:greet()

local dmg: integer = Math.clamp(50, 0, 100)
p.damage(dmg)
```

---

## Quick Reference

### Keywords

| Keyword | Purpose |
|---|---|
| `class` | Declare a class |
| `interface` | Declare an interface |
| `enum` | Declare an enum |
| `fn` | Declare a function or method |
| `extends` | Inherit from a class |
| `implements` | Fulfill one or more interfaces |
| `super` | Call the parent constructor |
| `self` | Reference the current instance |
| `static` | Declare a class-level member |
| `local` | Declare a private member or variable |
| `export` | Make a file member publicly importable |
| `import` | Import from another file |
| `return` | Return a value from a function |
| `throw` | Raise an error |
| `try` | Begin an error-handled block |
| `catch` | Handle a thrown error |
| `for` | Begin a loop |
| `while` | Begin a while loop |
| `repeat` | Begin a repeat-until loop |
| `until` | End condition for repeat loop |
| `break` | Exit a loop |
| `continue` | Skip to the next iteration |
| `if / else / end` | Conditional logic |
| `then` | Pipe result into next function |
| `nil` | Absence of value |
| `true / false` | Boolean literals |

### Operators

| Operator | Meaning |
|---|---|
| `?.` | Safe member access |
| `??` | Null coalescing fallback |
| `!` | Force unwrap nullable |
| `..` | String concatenation |
| `#` | Length of table or string |
| `==`, `!=` | Equality checks |
| `>`, `<`, `>=`, `<=` | Comparisons |
| `and`, `or`, `not` | Boolean logic |
| `+`, `-`, `*`, `/`, `%` | Arithmetic |
| `int()` | Cast float to integer, truncates toward zero |
| `float()` | Cast integer to float, always safe |
