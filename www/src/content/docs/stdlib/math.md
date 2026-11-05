---
title: "Math"
description: "Numeric utilities. integer and float are distinct types in Saule and nothing promotes between them silently, so Math is written to respect that rather…"
sidebar:
  order: 4
---

<!-- Generated from DOCS.md by `npm run sync-docs`. Edit that file, not this one. -->

Numeric utilities. `integer` and `float` are distinct types in Saule and
nothing promotes between them silently, so `Math` is written to respect that
rather than to paper over it.

Most of it is **generic over numeric flavour**, written `<N>` below, where
`N` is `integer` or `float` and nothing else. The flavour of the answer
follows from what the function does:

- the ones that **pick** one of their arguments — `abs`, `min`, `max`,
  `clamp`, `fmod` — answer in the flavour they were given;
- the ones that **compute a real number** — `sqrt`, `pow`, `log`, `exp` and
  the whole trigonometry family — always answer `float`;
- the **rounding** functions always answer `integer`.

```saule
-- integer in, integer out: no cast either way.
local hp: integer = Math.clamp(137, 0, 100)
println(hp)                          -- 100

-- A real number is always a float, even from integer arguments.
local root: float = Math.sqrt(9)
println(root)                        -- 3.0
```

Flavour still cannot be **mixed inside one call**. `Math.max(1, 2.5)` is a
type error for the same reason `1 + 2.5` is — cast one side with `as float`
first.

### Constants

| Name | Type | Value |
| --- | --- | --- |
| `Math.pi` | `float` | π |
| `Math.e` | `float` | Euler's number |
| `Math.huge` | `float` | positive infinity |
| `Math.maxinteger` | `integer` | the largest `integer`, 2⁶³−1 |
| `Math.mininteger` | `integer` | the smallest `integer`, −2⁶³ |

```saule
println(Math.pi)                         -- 3.141592653589793
println(Math.maxinteger)                 -- 9223372036854775807

-- `huge` is what a comparison against "no limit yet" starts from.
local best: float = Math.huge
for score in {4.5, 2.0, 9.0} do
    if score < best then
        best = score
    end
end
println(best)                            -- 2.0

-- Integer arithmetic wraps at the ends of the range rather than trapping.
println(Math.maxinteger + 1 == Math.mininteger)   -- true
```

### Rounding

<div class="api-list">
<details class="api-entry" id="math-floor">
<summary><code class="api-sig"><span class="sig-fn">Math.floor</span><span class="sig-punct">&lt;</span><span class="sig-type">N</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">n</span><span class="sig-punct">:</span> <span class="sig-type">N</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">integer</span></code></summary>


Rounds toward negative infinity. An `integer` comes back unchanged.

```saule
println(Math.floor(2.9))      -- 2
println(Math.floor(2.1))      -- 2
println(Math.floor(-2.1))     -- -3, toward -inf, not toward zero
println(Math.floor(5))        -- 5, already an integer
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="math-ceil">
<summary><code class="api-sig"><span class="sig-fn">Math.ceil</span><span class="sig-punct">&lt;</span><span class="sig-type">N</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">n</span><span class="sig-punct">:</span> <span class="sig-type">N</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">integer</span></code></summary>


Rounds toward positive infinity.

```saule
println(Math.ceil(2.1))       -- 3
println(Math.ceil(-2.9))      -- -2
println(Math.ceil(5))         -- 5

-- How many pages of ten a list needs.
local items: integer = 23
println(Math.ceil((items as float) / 10.0))   -- 3
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="math-round">
<summary><code class="api-sig"><span class="sig-fn">Math.round</span><span class="sig-punct">&lt;</span><span class="sig-type">N</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">n</span><span class="sig-punct">:</span> <span class="sig-type">N</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">integer</span></code></summary>


Rounds to the nearest integer, with a half rounding **away from zero** —
`2.5` goes to `3` and `-2.5` to `-3`. This is not banker's rounding: a `.5`
never rounds toward even.

```saule
println(Math.round(2.4))      -- 2
println(Math.round(2.5))      -- 3, away from zero
println(Math.round(-2.5))     -- -3
println(Math.round(3.5))      -- 4
```


</details>
</div>

### Sign

<div class="api-list">
<details class="api-entry" id="math-sign">
<summary><code class="api-sig"><span class="sig-fn">Math.sign</span><span class="sig-punct">&lt;</span><span class="sig-type">N</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">n</span><span class="sig-punct">:</span> <span class="sig-type">N</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">integer</span></code></summary>


`-1`, `0` or `1`. Always an `integer`, whatever flavour went in.

```saule
println(Math.sign(-8))        -- -1
println(Math.sign(0))         -- 0
println(Math.sign(2.5))       -- 1

-- Stepping toward a target without branching on direction.
local current: integer = 3
local target: integer = 10
println(current + Math.sign(target - current))   -- 4
```


</details>
</div>

### Numeric kind

<div class="api-list">
<details class="api-entry" id="math-type">
<summary><code class="api-sig"><span class="sig-fn">Math.type</span><span class="sig-punct">(</span><span class="sig-param">v</span><span class="sig-punct">:</span> <span class="sig-type">any</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">string</span><span class="sig-punct">?</span></code></summary>


`"integer"` or `"float"` for a number, `nil` for anything else. The way to
ask which flavour a value that arrived through `any` actually has.

```saule
println(Math.type(1) ?? "?")        -- integer
println(Math.type(1.0) ?? "?")      -- float
println(Math.type("1") == nil)      -- true, not a number

-- Which flavour arrived.
local value: any = 2.5
if (Math.type(value) ?? "") == "float" then
    println("a float: " .. ((value as float) ?? 0.0))   -- a float: 2.5
end
```


</details>
</div>

### Arithmetic

<div class="api-list">
<details class="api-entry" id="math-abs">
<summary><code class="api-sig"><span class="sig-fn">Math.abs</span><span class="sig-punct">&lt;</span><span class="sig-type">N</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">n</span><span class="sig-punct">:</span> <span class="sig-type">N</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">N</span></code></summary>


Magnitude, in the flavour it was given.

```saule
println(Math.abs(-8))         -- 8, an integer
println(Math.abs(-2.5))       -- 2.5, a float
println(Math.abs(0))          -- 0

-- Distance between two numbers, direction discarded.
println(Math.abs(3 - 10))     -- 7
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="math-min">
<summary><code class="api-sig"><span class="sig-fn">Math.min</span><span class="sig-punct">&lt;</span><span class="sig-type">N</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-punct">...</span><span class="sig-type">N</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">N</span></code></summary>


The smallest of its arguments, which must all share one flavour.

```saule
println(Math.min(3, 1, 7))          -- 1
println(Math.min(2.5, 1.5))         -- 1.5
println(Math.min(5))                -- 5, one argument is fine
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="math-max">
<summary><code class="api-sig"><span class="sig-fn">Math.max</span><span class="sig-punct">&lt;</span><span class="sig-type">N</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-punct">...</span><span class="sig-type">N</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">N</span></code></summary>


The largest of its arguments.

```saule
println(Math.max(3, 1, 7))          -- 7
println(Math.max(2.5, 1.5))         -- 2.5

-- A floor under a computed value, without an `if`.
local damage: integer = 2
println(Math.max(damage - 5, 0))    -- 0
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="math-clamp">
<summary><code class="api-sig"><span class="sig-fn">Math.clamp</span><span class="sig-punct">&lt;</span><span class="sig-type">N</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">n</span><span class="sig-punct">:</span> <span class="sig-type">N</span><span class="sig-punct">,</span> <span class="sig-param">lo</span><span class="sig-punct">:</span> <span class="sig-type">N</span><span class="sig-punct">,</span> <span class="sig-param">hi</span><span class="sig-punct">:</span> <span class="sig-type">N</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">N</span></code></summary>


Pins `n` into the inclusive range `[lo, hi]`.

```saule
println(Math.clamp(137, 0, 100))      -- 100
println(Math.clamp(-5, 0, 100))       -- 0
println(Math.clamp(42, 0, 100))       -- 42, already inside

-- Keeping a value in range as it changes.
local health: integer = 95
health = Math.clamp(health + 20, 0, 100)
println(health)                       -- 100
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="math-fmod">
<summary><code class="api-sig"><span class="sig-fn">Math.fmod</span><span class="sig-punct">&lt;</span><span class="sig-type">N</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">a</span><span class="sig-punct">:</span> <span class="sig-type">N</span><span class="sig-punct">,</span> <span class="sig-param">b</span><span class="sig-punct">:</span> <span class="sig-type">N</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">N</span></code></summary>


The remainder after truncated division, in the flavour it was given. The
sign follows the **left** operand, which is also what the `%` operator does —
unlike Lua, where `%` floors and takes the sign of the right one.

```saule
println(Math.fmod(7, 3))        -- 1
println(Math.fmod(-7, 3))       -- -1, the sign of the left operand
println(-7 % 3)                 -- -1, the operator agrees
println(Math.fmod(7.5, 2.0))    -- 1.5
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="math-modf">
<summary><code class="api-sig"><span class="sig-fn">Math.modf</span><span class="sig-punct">&lt;</span><span class="sig-type">N</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">n</span><span class="sig-punct">:</span> <span class="sig-type">N</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">float</span><span class="sig-punct">&gt;</span></code></summary>


Splits a number into its integer and fractional parts, as a two-element
table of floats.

```saule
local parts: table<float> = Math.modf(2.75)
println(parts[1] ?? 0.0)        -- 2.0, the integer part
println(parts[2] ?? 0.0)        -- 0.75, the fraction

-- Both parts carry the sign.
local neg: table<float> = Math.modf(-2.75)
println(neg[1] ?? 0.0)          -- -2.0
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="math-pow">
<summary><code class="api-sig"><span class="sig-fn">Math.pow</span><span class="sig-punct">&lt;</span><span class="sig-type">N</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">base</span><span class="sig-punct">:</span> <span class="sig-type">N</span><span class="sig-punct">,</span> <span class="sig-param">exp</span><span class="sig-punct">:</span> <span class="sig-type">N</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">float</span></code></summary>


`base` raised to the power `exp`. **Always a float**, which is what lets it
answer a negative exponent — `2 ^ -1` is an error on integers, while
`Math.pow(2, -1)` is `0.5`.

```saule
println(Math.pow(2, 10))      -- 1024.0, a float even from integers
println(Math.pow(2, -1))      -- 0.5, where `2 ^ -1` is an error
println(Math.pow(9.0, 0.5))   -- 3.0, both arguments share one flavour
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="math-sqrt">
<summary><code class="api-sig"><span class="sig-fn">Math.sqrt</span><span class="sig-punct">&lt;</span><span class="sig-type">N</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">n</span><span class="sig-punct">:</span> <span class="sig-type">N</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">float</span></code></summary>


The square root.

```saule
println(Math.sqrt(9))         -- 3.0
println(Math.sqrt(2))         -- 1.4142135623730951

-- The hypotenuse of a right triangle.
local a: float = 3.0
local b: float = 4.0
println(Math.sqrt(a * a + b * b))   -- 5.0
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="math-exp">
<summary><code class="api-sig"><span class="sig-fn">Math.exp</span><span class="sig-punct">&lt;</span><span class="sig-type">N</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">n</span><span class="sig-punct">:</span> <span class="sig-type">N</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">float</span></code></summary>


`e` raised to the power `n`.

```saule
println(Math.exp(0))          -- 1.0
println(Math.exp(1))          -- 2.718281828459045
println(Math.exp(1) == Math.e)   -- true
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="math-log">
<summary><code class="api-sig"><span class="sig-fn">Math.log</span><span class="sig-punct">&lt;</span><span class="sig-type">N</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">n</span><span class="sig-punct">:</span> <span class="sig-type">N</span><span class="sig-punct">,</span> <span class="sig-param">base</span><span class="sig-punct">:</span> <span class="sig-type">N</span><span class="sig-punct">?</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">float</span></code></summary>


The natural logarithm, or the logarithm in `base` when one is given.

```saule
println(Math.log(Math.e))     -- 1.0, natural by default
println(Math.log(8, 2))       -- 3.0, base 2

-- Floating point: a logarithm that "should" be exact often is not, so
-- compare with a tolerance rather than with `==`.
println(Math.log(1000, 10))   -- 2.9999999999999996

-- How many bits a value needs.
println(Math.ceil(Math.log(300, 2)))   -- 9
```


</details>
</div>

### Trigonometry

All of these take an `integer` or a `float` and answer a `float` — an angle
is a real number, so there is no integer-in / integer-out form the way there
is for `abs` or `clamp`.

<div class="api-list">
<details class="api-entry" id="math-sin">
<summary><code class="api-sig"><span class="sig-fn">Math.sin</span><span class="sig-punct">&lt;</span><span class="sig-type">N</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">n</span><span class="sig-punct">:</span> <span class="sig-type">N</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">float</span></code></summary>


Sine of `n` **radians**.

```saule
println(Math.sin(0))                      -- 0.0
println(Math.sin(Math.pi / 2.0))          -- 1.0

-- Degrees go through `rad` first.
println(Math.sin(Math.rad(90)))           -- 1.0
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="math-cos">
<summary><code class="api-sig"><span class="sig-fn">Math.cos</span><span class="sig-punct">&lt;</span><span class="sig-type">N</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">n</span><span class="sig-punct">:</span> <span class="sig-type">N</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">float</span></code></summary>


Cosine of `n` radians.

```saule
println(Math.cos(0))                      -- 1.0
println(Math.cos(Math.pi))                -- -1.0

-- A point on the unit circle, a third of the way round.
local angle: float = Math.rad(120)
println(Math.round(Math.cos(angle) * 100.0))   -- -50
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="math-tan">
<summary><code class="api-sig"><span class="sig-fn">Math.tan</span><span class="sig-punct">&lt;</span><span class="sig-type">N</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">n</span><span class="sig-punct">:</span> <span class="sig-type">N</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">float</span></code></summary>


Tangent of `n` radians.

```saule
println(Math.tan(0))                      -- 0.0
println(Math.round(Math.tan(Math.rad(45))))   -- 1
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="math-asin">
<summary><code class="api-sig"><span class="sig-fn">Math.asin</span><span class="sig-punct">&lt;</span><span class="sig-type">N</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">n</span><span class="sig-punct">:</span> <span class="sig-type">N</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">float</span></code></summary>


Arcsine, in radians. `n` outside `[-1, 1]` has no answer and gives `nan`.

```saule
println(Math.asin(0))                     -- 0.0
println(Math.asin(1) == Math.pi / 2.0)    -- true
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="math-acos">
<summary><code class="api-sig"><span class="sig-fn">Math.acos</span><span class="sig-punct">&lt;</span><span class="sig-type">N</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">n</span><span class="sig-punct">:</span> <span class="sig-type">N</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">float</span></code></summary>


Arccosine, in radians.

```saule
println(Math.acos(1))                     -- 0.0
println(Math.acos(0) == Math.pi / 2.0)    -- true
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="math-atan">
<summary><code class="api-sig"><span class="sig-fn">Math.atan</span><span class="sig-punct">&lt;</span><span class="sig-type">N</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">y</span><span class="sig-punct">:</span> <span class="sig-type">N</span><span class="sig-punct">,</span> <span class="sig-param">x</span><span class="sig-punct">:</span> <span class="sig-type">N</span><span class="sig-punct">?</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">float</span></code></summary>


Arctangent, in radians. With one argument it is the arctangent of `y`; with
two it is the `atan2` form — the angle to the point `(x, y)`, with the
quadrant resolved correctly, which a single-argument arctangent cannot do.

```saule
println(Math.atan(0))                     -- 0.0
println(Math.atan(1, 1) == Math.pi / 4.0) -- true, 45°

-- The quadrant is what the two-argument form is for.
println(Math.round(Math.deg(Math.atan(1, -1))))    -- 135
println(Math.round(Math.deg(Math.atan(-1, -1))))   -- -135
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="math-deg">
<summary><code class="api-sig"><span class="sig-fn">Math.deg</span><span class="sig-punct">&lt;</span><span class="sig-type">N</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">radians</span><span class="sig-punct">:</span> <span class="sig-type">N</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">float</span></code></summary>


Radians to degrees.

```saule
println(Math.deg(Math.pi))                -- 180.0
println(Math.deg(Math.pi / 2.0))          -- 90.0
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="math-rad">
<summary><code class="api-sig"><span class="sig-fn">Math.rad</span><span class="sig-punct">&lt;</span><span class="sig-type">N</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">degrees</span><span class="sig-punct">:</span> <span class="sig-type">N</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">float</span></code></summary>


Degrees to radians.

```saule
println(Math.rad(180))                    -- 3.141592653589793
println(Math.rad(180) == Math.pi)         -- true

-- Round tripping.
println(Math.deg(Math.rad(45)))           -- 45.0
```


</details>
</div>

### Randomness

<div class="api-list">
<details class="api-entry" id="math-random">
<summary><code class="api-sig"><span class="sig-fn">Math.random</span><span class="sig-punct">(</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">float</span></code></summary>


A uniform float in `[0, 1)` — zero is possible, one is not.

```saule
Math.randomseed(42)
local r: float = Math.random()
println(r >= 0.0 and r < 1.0)     -- true
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="math-random-2">
<summary><code class="api-sig"><span class="sig-fn">Math.random</span><span class="sig-punct">(</span><span class="sig-param">n</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">integer</span></code></summary>


A uniform integer in `[1, n]`, both ends included.

```saule
Math.randomseed(42)
local roll: integer = Math.random(6)
println(roll >= 1 and roll <= 6)   -- true
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="math-random-3">
<summary><code class="api-sig"><span class="sig-fn">Math.random</span><span class="sig-punct">(</span><span class="sig-param">lo</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">,</span> <span class="sig-param">hi</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">integer</span></code></summary>


A uniform integer in `[lo, hi]`, both ends included.

```saule
Math.randomseed(42)
local n: integer = Math.random(10, 20)
println(n >= 10 and n <= 20)       -- true

-- Picking an element out of a table.
local names: table<string> = {"ada", "grace", "alan"}
println(names[Math.random(1, #names)] != nil)   -- true
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="math-randomseed">
<summary><code class="api-sig"><span class="sig-fn">Math.randomseed</span><span class="sig-punct">(</span><span class="sig-param">seed</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-kw">nil</span></code></summary>


Resets the generator, so a sequence of `Math.random` calls repeats exactly.
This is what makes a test or a procedurally generated world reproducible.

```saule
Math.randomseed(42)
local first: integer = Math.random(1000)

Math.randomseed(42)
local again: integer = Math.random(1000)

println(first == again)       -- true, the same seed gives the same sequence
```


</details>
</div>

### Unsigned comparison

<div class="api-list">
<details class="api-entry" id="math-ult">
<summary><code class="api-sig"><span class="sig-fn">Math.ult</span><span class="sig-punct">(</span><span class="sig-param">a</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">,</span> <span class="sig-param">b</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">boolean</span></code></summary>


Compares two integers as **unsigned** 64-bit values. A negative integer has
its top bit set, so it reads as a very large unsigned number.

```saule
println(Math.ult(1, 2))           -- true
println(Math.ult(-1, 1))          -- false: -1 is the largest unsigned value
println(1 < 2)                    -- true, the signed comparison agrees here
println(-1 < 1)                   -- true, where the signed one differs
```

</details>
</div>

