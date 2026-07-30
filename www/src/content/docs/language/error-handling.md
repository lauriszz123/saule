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
