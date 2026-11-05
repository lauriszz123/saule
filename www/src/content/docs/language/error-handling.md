---
title: "Error Handling"
description: "Saule uses try / catch for unexpected runtime errors. For expected, recoverable failures — missing data, invalid input, parse errors — prefer nullable…"
sidebar:
  order: 11
---

<!-- Generated from LANGUAGE.md by `npm run sync-docs`. Edit that file, not this one. -->

Saule uses `try / catch` for unexpected runtime errors. For expected, recoverable failures — missing data, invalid input, parse errors — prefer **nullable return types** (`-> T?`) and let null safety carry the failure through the type system. A user-defined `Result<T>` class is trivial to write on top of classes and generics if you want richer error payloads.

### Throwing Errors

```saule
class Player
    health: integer = 100

    fn damage(amount: integer)
        if amount < 0 then
            throw "Damage cannot be negative"
        end

        self.health = self.health - amount
    end
end

local p: Player = Player()
p.damage(10)
println(p.health)                       -- 90
```

### Try / Catch

```saule
class Player
    health: integer = 0

    fn init(health: integer)
        self.health = health
    end

    fn damage(amount: integer)
        if amount < 0 then
            throw "Damage cannot be negative"
        end

        self.health = self.health - amount
    end
end

try
    local p: Player = Player(100)
    p.damage(-10)
catch e: string
    println("Caught: " .. e)    -- Caught: Damage cannot be negative
end
```

The `catch` clause names the thrown value and its expected type. Inside the catch block, code runs as if the `try` block had returned normally.

### Several `catch` clauses

A `try` may carry more than one clause. They are tried **in source order**, and the first whose type matches the thrown value handles it; a value no clause accepts carries on to the enclosing `try`:

```saule
class ParseError
    detail: string = ""

    fn init(detail: string)
        self.detail = detail
    end
end

fn risky()
    throw ParseError("unexpected `}` on line 3")
end

try
    risky()
catch e: string
    println("message: " .. e)
catch e: integer
    println("code: " .. e)
catch e: ParseError
    println("parse: " .. e.detail)      -- parse: unexpected `}` on line 3
catch e: any
    println("something else")
end
```

Each clause's binding is scoped to its own body, so the clauses are free to reuse a name. `catch e: any` matches everything, which is why it belongs last — a clause after it can never run, and the compiler says so:

```saule
try
    throw "boom"
catch e: any
    println("anything")
catch e: string      -- ERROR: this `catch string` can never run
    println("never")
end
```

### `finally`

A `finally` block runs on **every** way out of the `try`: normal completion, a clause that handled a throw, a throw that no clause handled, and a `return`, `break` or `continue` from inside:

```saule
local path: string = Os.tmpname()

local out: File = Io.open(path, IoMode.Write)!
out.write("port=8080")
out.close()

fn parse(text: string) -> string
    local eq: integer?, _: integer? = String.find(text, "=")
    if eq == nil then
        throw "no `=` in `" .. text .. "`"
    end
    return String.sub(text, eq + 1)
end

fn fallback() -> string
    return "80"
end

fn readPort() -> string
    local f: File = Io.open(path, IoMode.Read)!
    try
        return parse(f.read("a") ?? "")
    catch e: string
        return fallback()
    finally
        f.close()            -- runs on all three paths
    end
end

println(readPort())              -- 8080
Os.remove(path)
```

When nothing handled the throw, the `finally` runs and then the value carries on outward — cleanup does not swallow the failure. A `try` needs at least one of `catch` and `finally`; with neither it protects nothing and runs nothing.

### Nullable returns for expected failures

```saule
class Player
    local name: string

    fn init(name: string)
        self.name = name
    end

    fn getName() -> string
        return self.name
    end
end

class Repo
    local items: table<Player>

    fn init(items: table<Player>)
        self.items = items
    end

    fn findPlayer(id: integer) -> Player?
        if id < 0 or id > #self.items then
            return nil
        end
        return self.items[id]
    end
end

local repo: Repo = Repo({Player("Arthur")})

local name: string = repo.findPlayer(5)?.getName() ?? "Unknown"
println(name)                   -- Unknown
println(repo.findPlayer(1)?.getName() ?? "Unknown")     -- Arthur
```

Reserve `try / catch` for truly unexpected runtime errors — bad data from an external source, file I/O failures, contract violations. For everyday "this lookup might miss", `T?` plus `?.` / `??` keeps the failure mode visible in the type signature.

---
