---
title: "Prelude (always in scope)"
description: "These names are bound at the top of every module — no import required."
sidebar:
  order: 1
---

<!-- Generated from DOCS.md by `npm run sync-docs`. Edit that file, not this one. -->

These names are bound at the top of every module — no import required.

| Signature | Description |
| --- | --- |
| `print(...any) -> nil` | Write arguments to stdout separated by tabs, no trailing newline. |
| `println(...any) -> nil` | Same as `print`, with a trailing newline. |
| `printf(fmt: string, ...any) -> nil` | Format like `String.format` and write to stdout (no newline). |
| `tostring(v: any) -> string` | Human-readable rendering of any value. For a class instance implementing `OpToString`, calls its `toString()`; `print` / `println` / `..` render the same way. |
| `type(v: any) -> string` | Returns the runtime type name: `"integer"`, `"float"`, `"string"`, `"boolean"`, `"nil"`, `"function"`, `"table"`, or the class name for instances. `"function"` is a runtime tag covering every callable — it is not a type you can write, since a function's type is its signature (`fn(A) -> R`). |
| `assert<T>(v: T?, msg: string?) -> T` | If `v` is truthy, returns it with its nullability stripped; otherwise throws `msg` (default `"assertion failed"`). |
| `error(msg: string) -> nil` | Throws `msg` as a runtime error. Equivalent to `throw msg`. |

```saule
local n: integer = "42" as integer ?? 0
printf("got %d\n", n)
```

Conversion between types is the `as` cast, not a function — `3.9 as
integer`, `n as string`, `"42" as integer` (which is `integer?`, since the
text may hold no number). See [README §Casting](/saule/language/types/#casting).

### Prelude interfaces

These interfaces are in scope everywhere too, so a class can implement them
without an import. They carry no behaviour of their own — each one is a
contract the language itself looks for.

| Interface | What it enables |
| --- | --- |
| `Iterable<T>` | `for v in instance do` — `fn iter() -> fn() -> T?` returns the step closure. |
| `Iterable2<K, V>` | `for k, v in instance do` — the step closure returns two values. |
| `OpAdd<T, R>` `OpSub<T, R>` `OpMul<T, R>` `OpDiv<T, R>` `OpMod<T, R>` `OpPow<T, R>` | `+` `-` `*` `/` `%` `^` — `fn add(other: T) -> R`, and so on. |
| `OpBAnd<T, R>` `OpBOr<T, R>` `OpBXor<T, R>` `OpShl<T, R>` `OpShr<T, R>` | `&` `\|` `~` `<<` `>>` — `fn band(other: T) -> R`, and so on. |
| `OpNeg<R>` | `-a` — `fn neg() -> R`. |
| `OpBNot<R>` | `~a` — `fn bnot() -> R`. |
| `OpLen` | `#a` — `fn len() -> integer`. |
| `OpConcat<T, R>` | `a .. b` — `fn concat(other: T) -> R`. |
| `OpEq<T>` | `a == b` and `a != b` — `fn equals(other: T) -> boolean`. |
| `OpCompare<T>` | `<` `<=` `>` `>=` — `fn compare(other: T) -> integer`, negative / zero / positive. |
| `OpToString` | `tostring(a)`, `print(a)`, and `..` — `fn toString() -> string`. |
| `OpIndex<K, V>` | `a[k]` — `fn index(key: K) -> V`. Saule's `__index`. |
| `OpNewIndex<K, V>` | `a[k] = v` — `fn newIndex(key: K, value: V) -> nil`. Saule's `__newindex`. |
| `Assignable<T>` | `local a: C = t` — `static fn of(value: T) -> C`. A bare `T` may fill a `C` slot; `C.of` builds it. |

The `<T>` on these is a real type argument, checked like any other: `OpAdd`
takes two (the operand and the result), `OpEq` one, `OpLen` none — supplying
the wrong number is an error rather than being ignored. See
[Custom Iterable](/saule/language/interfaces/#custom-iterable) and
[Operator Overloading](/saule/language/interfaces/#operator-overloading) for the rules and
worked examples, and [Generic Classes](/saule/language/classes/#generic-classes) for
declaring your own.

---
