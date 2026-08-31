---
title: "Math"
description: "Numeric utilities. integer and float are distinct in Saule, so several functions return integer even when given a float (e.g. floor, ceil)."
sidebar:
  order: 3
---

<!-- Generated from DOCS.md by `npm run sync-docs`. Edit that file, not this one. -->

Numeric utilities. `integer` and `float` are distinct in Saule, so several
functions return `integer` even when given a `float` (e.g. `floor`, `ceil`).

Most of `Math` is **generic over numeric flavour**, written `<N>` below: `N`
is `integer` or `float` and nothing else. So `Math.sqrt(9)` and
`Math.clamp(hp, 0, 100)` are ordinary calls — no cast into them, and none
back out. The functions that *pick* one of their arguments (`abs`, `min`,
`max`, `clamp`, `fmod`) answer in the flavour they were given; the ones that
compute a real number (`sqrt`, `pow`, `log`, the trig family) always answer
`float`; the rounding functions always answer `integer`.

Flavour still cannot be **mixed inside one call**. `Math.max(1, 2.5)` is a
type error for the same reason `1 + 2.5` is — Saule never auto-promotes, and
the stdlib does not get a private exemption. Cast one side with `as float`.

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
| `Math.floor<N>(n: N) -> integer` | Round toward `-inf`. An `integer` comes back unchanged. |
| `Math.ceil<N>(n: N) -> integer` | Round toward `+inf`. |
| `Math.round<N>(n: N) -> integer` | Banker's-friendly round. |
| `Math.type(v: any) -> string?` | `"integer"` / `"float"` for numbers, `nil` for non-numbers. |
| `Math.sign<N>(n: N) -> integer` | `-1`, `0`, or `1`. |

### Arithmetic

| Signature | Description |
| --- | --- |
| `Math.abs<N>(n: N) -> N` | Magnitude, in the flavour it was given. |
| `Math.min<N>(...N) -> N` | Minimum of all arguments, in their shared flavour. |
| `Math.max<N>(...N) -> N` | Maximum of all arguments. |
| `Math.clamp<N>(n: N, lo: N, hi: N) -> N` | Pin `n` into `[lo, hi]`. |
| `Math.fmod<N>(a: N, b: N) -> N` | Truncated remainder, in the flavour it was given. |
| `Math.modf<N>(n: N) -> table<float>` | `[int_part, frac_part]`. |
| `Math.pow<N>(a: N, b: N) -> float` | `a^b`. Always a float — `Math.pow(2, -1)` is `0.5`. |
| `Math.sqrt<N>(n: N) -> float` | √n. |
| `Math.exp<N>(n: N) -> float` | `e^n`. |
| `Math.log<N>(n: N, base: N?) -> float` | Natural log; with `base`, log in that base. |

### Trigonometry

`sin`, `cos`, `tan`, `asin`, `acos`, `atan(y, x?)` (`atan2`-style),
`deg(rad)`, `rad(deg)`. All take an `integer` or a `float` and return a
`float`.

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
println(Math.sqrt(2), Math.log(8, 2))           -- 1.414…  3.0

-- integer in, integer out: no cast either way
local hp: integer = Math.clamp(137, 0, 100)     -- 100
local span: integer = Math.max(3, 7, 2)         -- 7
```

---
