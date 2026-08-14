# wrapper-types

Two ways to make a class feel built in, without giving up static checking: a
`Text` built straight from a string literal, and a `Settings` with full
control of every read and write.

Run with:

```sh
saule run
```

## The contracts

| Interface | Fires on | Method |
|---|---|---|
| `Assignable<T>` | `local x: C = value` | `static fn of(value: T) -> C` |
| `OpIndex<K, V>` | `obj[key]` | `fn index(key: K) -> V` |
| `OpNewIndex<K, V>` | `obj[key] = value` | `fn newIndex(key: K, value: V) -> nil` |

| File | Uses |
|---|---|
| `src/text.sau` | `Assignable<string>` + `OpToString` |
| `src/settings.sau` | `OpIndex` + `OpNewIndex` |
| `src/main.sau` | all of them, plus the boundaries |

## Nothing is injected

`Text` wraps a `string` and exposes `length`, `upper`, `slice`, `startsWith`
and `words`. It **declares every one of them**, and implements each by calling
the `String` class explicitly:

```saule
fn upper() -> string
    return String.upper(self.raw)
end
```

That is not boilerplate the language could have saved you — it is the whole
point. `string` is a *type* and has no members; `String` is a separate *class*
of static functions. There is no mapping between the two anywhere in Saule, so
a wrapper chooses its own surface: `Text` has `words()`, which `String` does
not, and deliberately does not expose `String.rep`.

## `Assignable<T>` — build from a value

```saule
local greeting: Text = "hello wrapped world"
println(describe("two words"))    -- and at a parameter
```

The annotation picks the target, so this is *target-typed*: there is never a
question of which class a bare value should become, only whether the one asked
for accepts it.

It applies at exactly two kinds of site — an annotated `local` or module
variable, and a user function's or method's parameters. Everywhere else the
ordinary rule stands:

```saule
local all: table<Text> = {"a"}    -- ERROR
local t: Text = "a"
t = "b"                            -- ERROR: only the declaration converts
```

That boundary is soundness, not an unfinished edge. The interpreter converts
at those sites and only those, so relaxing the checker anywhere else would
typecheck a value that never gets built.

## `OpIndex` / `OpNewIndex` — full control of get and set

Saule's `__index` / `__newindex`, with one deliberate difference from Lua's:
they are **not** miss handlers over a stored key space. A class instance has no
keys of its own, so the method *is* the lookup and runs on every access.

That is what makes `Settings` reliable — there is no "already present" path
that would skip the normalising:

```saule
s["theme"] = "SOLARIZED"
println(s["theme"])        -- solarized
println(s["editor"])       -- vim — a default, never stored
```

`obj.name` is deliberately **not** routed here. Field and method names are
resolved statically, and sending their misses to a hook would give up
"unknown member" diagnostics for the whole class. Dynamic access is
`obj[key]`; a fixed surface is declared as ordinary methods.

A hook that indexes `self` re-enters itself. Lua answers that with `rawget` /
`rawset`; Saule caps the depth and reports it, so the mistake is a diagnostic
naming the class rather than a hang.

## Files

- `src/text.sau` — built from a value, with a surface it declares itself
- `src/settings.sau` — full control of reading and writing
- `src/main.sau` — all of it in use, and where it stops
