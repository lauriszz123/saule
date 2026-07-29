---
title: "Interfaces"
description: "Interfaces define a contract — method signatures only, no fields, no bodies."
sidebar:
  order: 7
---

<!-- Generated from README.md by `npm run sync-docs`. Edit that file, not this one. -->

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

Any class implementing `Iterable<T>` works inside a `for-in` loop automatically. The contract is a single method `iter()` that returns a **step closure**: each call returns the next element, or `nil` to signal the end. The loop stops on the first `nil`.

```saule
interface Iterable<T>
    fn iter() -> fn() -> T?
end

export class PlayerQueue implements Iterable<Player>
    local items: table<Player>

    fn init()
        self.items = {}
    end

    fn push(p: Player)
        self.items[#self.items + 1] = p
    end

    fn iter() -> fn() -> Player?
        local cursor: integer = 1

        return fn()
            if cursor > #self.items then
                return nil
            end

            local p: Player = self.items[cursor]
            cursor = cursor + 1
            return p
        end
    end
end

local queue: PlayerQueue = PlayerQueue()
queue.push(Player("Arthur", 100))
queue.push(Player("Merlin", 80))

for player: Player in queue do
    player.greet()
end
```

For iteration that yields **pairs** (key + value, index + value, etc.), implement `Iterable2<K, V>` whose `iter()` returns a closure with two return values:

```saule
interface Iterable2<K, V>
    fn iter() -> fn() -> (K?, V?)
end

for key: string, value: Player in playerMap do
    print(key .. " = " .. value.getName())
end
```

The loop also accepts raw step closures and plain `table` values directly — `Iterable` is just the contract that makes user-defined classes look the same.

---
