---
title: Your First Program
description: From a one-line script to a typed, multi-file Saule project in a few minutes.
sidebar:
  order: 3
---

## Hello, world

Put this in `hello.sau`:

```saule
println("Hello, world!")
```

Run it:

```sh
saule run hello.sau
```

Naming a **file** puts the compiler in single-file mode: the script runs
top-to-bottom like a Lua script, and no `class Main` is required.

## Adding types

Types are checked before a single line executes. On a local the annotation is
optional — the initializer implies it — but writing it out is the clearer
habit, and parameters and fields have to state theirs.

```saule
local name: string = "Arthur"
local health: integer = 100
local speed: float = 1.5

println(name .. " has " .. health .. " HP")
```

Try changing `health` to `100.0` and running it again — Saule rejects the
program rather than quietly converting between
[`integer` and `float`](/saule/language/types/#integer-vs-float).

## A class

```saule
class Greeter
    local greeting: string

    fn init(greeting: string)
        self.greeting = greeting
    end

    fn greet(who: string)
        println(self.greeting .. ", " .. who .. "!")
    end
end

local g = Greeter("Hello")
g.greet("world")
```

Fields are declared at the top, `fn init` is the constructor, and `self` refers
to the instance. There is no `new` keyword — calling the class name constructs
an instance.

## Handling nothing

A variable cannot be `nil` unless its type says so. Mark it with `?` and the
compiler makes you handle the empty case:

```saule
local nickname: string? = nil

-- `??` supplies a fallback
local display: string = nickname ?? "no nickname"
println(display)

-- `#` needs a definite value, so coalesce before measuring
local len: integer = #(nickname ?? "")
println(len)
```

Drop the `?? "no nickname"` and the program stops compiling — a `string?`
cannot be assigned to a `string`.

## Starting a real project

For anything larger than a script, scaffold a project:

```sh
saule init myproject
cd myproject
saule run
```

`saule init` writes a `saule.config` and an entry file. Running `saule run` with
no argument — or naming a **directory** — puts the compiler in project mode,
where the entry file must declare a `Main` class:

```saule
class Main
    static fn main()
        println("Hello from a project!")
    end
end
```

Whether the target is a directory is the *only* thing that chooses between
project mode and single-file mode.

### Multiple files

Project mode resolves imports across your source directories:

```saule
-- entities/Player.sau
export class Player
    local name: string

    fn init(name: string)
        self.name = name
    end

    fn greet()
        println("I am " .. self.name)
    end
end
```

```saule
-- main.sau
import Player from "entities/Player"

class Main
    static fn main()
        local p: Player = Player("Arthur")
        p.greet()
    end
end
```

Only `export`ed declarations are visible to other files. See
[Imports and File Structure](/saule/language/imports-and-file-structure/) for
folder modules, dependencies, and visibility rules.

### Passing arguments

Anything after `--` goes to your program untouched, and shows up in `Os.args()`:

```sh
saule run -- input.txt --verbose
```

## Next steps

- **[Language Guide](/saule/language/types/)** — the full tour.
- **[Project Configuration](/saule/language/project-configuration/)** — what `saule.config` accepts.
- **[CLI Reference](/saule/reference/cli/)** — `run`, `fmt`, `init`.
