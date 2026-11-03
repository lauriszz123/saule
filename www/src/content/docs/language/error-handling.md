---
title: "Error Handling"
description: "Saule uses try / catch for unexpected runtime errors. For expected, recoverable failures — missing data, invalid input, parse errors — prefer nullable…"
sidebar:
  order: 11
---

<!-- Generated from README.md by `npm run sync-docs`. Edit that file, not this one. -->

Saule uses `try / catch` for unexpected runtime errors. For expected, recoverable failures — missing data, invalid input, parse errors — prefer **nullable return types** (`-> T?`) and let null safety carry the failure through the type system. A user-defined `Result<T>` class is trivial to write on top of classes and generics if you want richer error payloads.

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
    local p: Player = Player("Arthur", 100)
    p.damage(-10)
catch e: string
    print("Caught: " .. e)
end
```

The `catch` clause names the thrown value and its expected type. Inside the catch block, code runs as if the `try` block had returned normally.

### Several `catch` clauses

A `try` may carry more than one clause. They are tried **in source order**, and the first whose type matches the thrown value handles it; a value no clause accepts carries on to the enclosing `try`:

```saule
try
    risky()
catch e: string
    println("message: " .. e)
catch e: integer
    println("code: " .. e)
catch e: ParseError
    println("parse: " .. e.detail)
catch e: any
    println("something else")
end
```

Each clause's binding is scoped to its own body, so the clauses are free to reuse a name. `catch e: any` matches everything, which is why it belongs last — a clause after it can never run, and the compiler says so:

```saule
catch e: any
    …
catch e: string      -- ERROR: this `catch string` can never run
```

### `finally`

A `finally` block runs on **every** way out of the `try`: normal completion, a clause that handled a throw, a throw that no clause handled, and a `return`, `break` or `continue` from inside:

```saule
local f: File = Io.open(path, IoMode.Read)!
try
    return parse(f.read("a") ?? "")
catch e: string
    return fallback()
finally
    f.close()            -- runs on all three paths
end
```

When nothing handled the throw, the `finally` runs and then the value carries on outward — cleanup does not swallow the failure. A `try` needs at least one of `catch` and `finally`; with neither it protects nothing and runs nothing.

### Nullable returns for expected failures

```saule
fn findPlayer(id: integer) -> Player?
    if id < 0 then
        return nil
    end
    return self.items[id]
end

local name: string = repo.findPlayer(5)?.getName() ?? "Unknown"
```

Reserve `try / catch` for truly unexpected runtime errors — bad data from an external source, file I/O failures, contract violations. For everyday "this lookup might miss", `T?` plus `?.` / `??` keeps the failure mode visible in the type signature.

---
