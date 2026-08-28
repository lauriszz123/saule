---
title: "Math"
description: "Numeric utilities. integer and float are distinct in Saule, so several functions return integer even when given a float (e.g. floor, ceil)."
sidebar:
  order: 3
---

<!-- Generated from DOCS.md by `npm run sync-docs`. Edit that file, not this one. -->

Numeric utilities. `integer` and `float` are distinct in Saule, so several
functions return `integer` even when given a `float` (e.g. `floor`, `ceil`).

### Constants

| Name | Value |
| --- | --- |
| `Math.pi` | π |
| `Math.e` | Euler's number |
| `Math.huge` | `+inf` |
| `Math.maxinteger` | i64 max |
| `Math.mininteger` | i64 min |

### Rounding & conversion

| Signature | Description |
| --- | --- |
| `Math.floor(n: float) -> integer` | Round toward `-inf`. |
| `Math.ceil(n: float) -> integer` | Round toward `+inf`. |
| `Math.round(n: float) -> integer` | Banker's-friendly round. |
| `Math.tointeger(v: any) -> integer?` | Lossless integer conversion (`3.0` → `3`, `3.5` → `nil`). |
| `Math.type(v: any) -> string?` | `"integer"` / `"float"` for numbers, `nil` for non-numbers. |
| `Math.sign(n: float) -> integer` | `-1`, `0`, or `1`. |

### Arithmetic

| Signature | Description |
| --- | --- |
| `Math.abs(n: integer) -> integer`<br />`Math.abs(n: float) -> float` | Magnitude, in the same kind as the input. |
| `Math.min(...float) -> float` | Minimum of all arguments. |
| `Math.max(...float) -> float` | Maximum of all arguments. |
| `Math.clamp(n: float, lo: float, hi: float) -> float` | Pin `n` into `[lo, hi]`. |
| `Math.fmod(a: integer, b: integer) -> integer`<br />`Math.fmod(a: float, b: float) -> float` | Truncated remainder, in the kind it was given. |
| `Math.modf(n: float) -> table<float>` | `[int_part, frac_part]`. |
| `Math.pow(a: float, b: float) -> float` | `a^b`. |
| `Math.sqrt(n: float) -> float` | √n. |
| `Math.exp(n: float) -> float` | `e^n`. |
| `Math.log(n: float, base: float?) -> float` | Natural log; with `base`, log in that base. |

### Trigonometry

`sin`, `cos`, `tan`, `asin`, `acos`, `atan(y, x?)` (`atan2`-style),
`deg(rad)`, `rad(deg)`. All take/return floats.

### Random & bit-ish

| Signature | Description |
| --- | --- |
| `Math.random() -> float` | Uniform `[0, 1)`. |
| `Math.random(n: integer) -> integer` | Uniform `[1, n]`. |
| `Math.random(lo: integer, hi: integer) -> integer` | Uniform `[lo, hi]`. |
| `Math.randomseed(seed: integer) -> nil` | Reset the PRNG. |
| `Math.ult(a: integer, b: integer) -> boolean` | Unsigned less-than. |

```saule
Math.randomseed(42)
local roll: integer = Math.random(1, 6)
println(Math.sqrt(2.0), Math.log(8.0, 2.0))     -- 1.414…  3.0
```

---
