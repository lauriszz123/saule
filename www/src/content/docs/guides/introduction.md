---
title: Introduction
description: Saule is a statically typed, class-oriented scripting language with Lua's runtime model and a type system that catches mistakes before you run anything.
sidebar:
  order: 1
---

Saule is a statically typed, class-oriented scripting language. It borrows Lua's
simplicity and runtime model — tables, closures, coroutines, multiple return
values — and puts a real type system in front of them.

The goal is a language that stays small enough to hold in your head while
refusing to let a `nil` reach production.

## What it looks like

```saule
export class Player
    local name: string
    local health: integer

    static maxHealth: integer = 100

    fn init(name: string, health: integer)
        self.name = name
        self.health = health
    end

    fn damage(amount: integer)
        self.health = self.health - amount

        if self.health <= 0 then
            self.health = 0
            println(self.name .. " has fallen")
        end
    end
end
```

## Why it exists

**Lua's ergonomics, without the runtime surprises.** Lua's single `number` type
is split into [`integer` and `float`](/saule/language/types/#integer-vs-float),
and mixing them is a compile error rather than a silent coercion. Tables are
[generically typed](/saule/language/tables/), so `table<string>` cannot quietly
acquire an integer.

**`nil` is opt-in.** A variable cannot hold `nil` unless its type says so with
`?`. The compiler then makes you deal with it — through
[safe access](/saule/language/null-safety/#safe-access), a
[coalescing default](/saule/language/null-safety/#null-coalescing), or an
explicit [force unwrap](/saule/language/null-safety/#force-unwrap) you had to
type out on purpose.

**Classes, not table metaprogramming.** Inheritance, interfaces, access
modifiers and static members are language constructs with real declarations
instead of metatable conventions you rediscover in every codebase.

**Exhaustive pattern matching.** [`match`](/saule/language/pattern-matching/)
knows your enum's variants and its payloads, and it will not compile if you
forgot one.

## Where to go next

- **[Installation](/saule/guides/installation/)** — build the toolchain and put it on your PATH.
- **[Your First Program](/saule/guides/first-program/)** — hello world, then a real project.
- **[Playground](/saule/play/)** — run Saule in your browser, nothing to install.
- **[Language Guide](/saule/language/types/)** — the full tour, starting with the type system.
- **[Standard Library](/saule/stdlib/prelude-always-in-scope/)** — `String`, `Math`, `Table`, `Io`, `Os`.
