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
| `tostring(v: any) -> string` | Human-readable rendering of any value. |
| `type(v: any) -> string` | Returns the runtime type name: `"integer"`, `"float"`, `"string"`, `"boolean"`, `"nil"`, `"function"`, `"table"`, or the class name for instances. |
| `int(n: integer \| float) -> integer` | Truncating conversion (`int(3.9) == 3`). |
| `float(n: integer \| float) -> float` | Lossless widening (`float(3) == 3.0`). |
| `assert<T>(v: T?, msg: string?) -> T` | If `v` is truthy, returns it with its nullability stripped; otherwise throws `msg` (default `"assertion failed"`). |
| `error(msg: string) -> nil` | Throws `msg` as a runtime error. Equivalent to `throw msg`. |

```saule
local n: integer = assert(int("42"!), "expected an integer")
printf("got %d\n", n)
```

---
