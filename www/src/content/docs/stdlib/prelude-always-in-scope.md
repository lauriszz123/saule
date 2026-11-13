---
title: "Prelude (always in scope)"
description: "These names are bound at the top of every module — no import required, for these or for any of the static classes below them."
sidebar:
  order: 1
---

<!-- Generated from DOCS.md by `npm run sync-docs`. Edit that file, not this one. -->

These names are bound at the top of every module — no import required, for
these or for any of the static classes below them.

### Printing

<div class="api-list">
<details class="api-entry" id="print">
<summary><code class="api-sig"><span class="sig-fn">print</span><span class="sig-punct">(</span><span class="sig-punct">...</span><span class="sig-type">any</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-kw">nil</span></code></summary>


Writes its arguments to stdout separated by **tabs**, with no trailing
newline. Each is rendered as `tostring` would.

```saule
print("a", "b", "c")
print("\n")                   -- a	b	c

-- Any type, not just strings.
print(1, 2.5, true, nil)
print("\n")                   -- 1	2.5	true	nil
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="println">
<summary><code class="api-sig"><span class="sig-fn">println</span><span class="sig-punct">(</span><span class="sig-punct">...</span><span class="sig-type">any</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-kw">nil</span></code></summary>


The same, with a trailing newline. This is the one to reach for.

```saule
println("hello")              -- hello
println(1, "two", 3.0)        -- 1	two	3.0
println()                     -- an empty line
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="printf">
<summary><code class="api-sig"><span class="sig-fn">printf</span><span class="sig-punct">(</span><span class="sig-param">fmt</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">,</span> <span class="sig-punct">...</span><span class="sig-type">any</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-kw">nil</span></code></summary>


Formats like [`String.format`](/saule/stdlib/string/#string-format) and writes the result to
stdout, with **no** trailing newline.

```saule
printf("%s scored %d\n", "Ada", 98)       -- Ada scored 98
printf("%.2f%%\n", 99.5)                  -- 99.50%

-- No newline unless you write one.
printf("a")
printf("b\n")                             -- ab
```


</details>
</div>

### Inspecting a value

<div class="api-list">
<details class="api-entry" id="tostring">
<summary><code class="api-sig"><span class="sig-fn">tostring</span><span class="sig-punct">(</span><span class="sig-param">v</span><span class="sig-punct">:</span> <span class="sig-type">any</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">string</span></code></summary>


A readable rendering of any value. For a class instance implementing
[`OpToString`](/saule/stdlib/prelude-always-in-scope/#prelude-interfaces) this calls its `toString()`; `print`,
`println` and `..` all render the same way.

```saule
println(tostring(42))             -- 42
println(tostring(2.5))            -- 2.5
println(tostring(true))           -- true
println(tostring(nil))            -- nil
println(tostring({1, 2}))         -- {1, 2}

-- Which is what `..` uses, so this is rarely needed explicitly.
println("n = " .. 42)             -- n = 42
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="type">
<summary><code class="api-sig"><span class="sig-fn">type</span><span class="sig-punct">(</span><span class="sig-param">v</span><span class="sig-punct">:</span> <span class="sig-type">any</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">string</span></code></summary>


The runtime type name: `"integer"`, `"float"`, `"string"`, `"boolean"`,
`"nil"`, `"function"`, `"table"`, or the class name for an instance.

`"function"` is a runtime tag covering every callable — it is not a type you
can write, since a function's type is its signature, `fn(A) -> R`.

```saule
println(type(42))             -- integer
println(type(2.5))            -- float
println(type("hi"))           -- string
println(type(true))           -- boolean
println(type(nil))            -- nil
println(type({1}))            -- table
println(type(println))        -- function

-- An instance reports its class.
class Point
    x: integer = 0
end
println(type(Point()))        -- Point
```


</details>
</div>

### Failing

<div class="api-list">
<details class="api-entry" id="assert">
<summary><code class="api-sig"><span class="sig-param">assert</span><span class="sig-punct">&lt;</span><span class="sig-type">T</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">v</span><span class="sig-punct">:</span> <span class="sig-type">T</span><span class="sig-punct">?</span><span class="sig-punct">,</span> <span class="sig-param">msg</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">?</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">T</span></code></summary>


If `v` is truthy, answers it with the nullability stripped. Otherwise throws
`msg`, which defaults to `"assertion failed"`.

The stripping is the point: `assert` is how a `T?` becomes a `T` when you are
prepared to fail rather than handle the absence.

```saule
local maybe: string? = "present"
local definite: string = assert(maybe)
println(definite)                     -- present

-- It answers the value, so it composes inline.
println(String.upper(assert(maybe)))  -- PRESENT

-- A failure ends the program. It is *not* a catchable `throw`: `try` /
-- `catch` does not intercept it, so use `assert` where failing is the
-- intended outcome, and `??` or a `nil` check where it is not.
local fallback: string = maybe ?? "default"
println(fallback)                     -- present
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="error">
<summary><code class="api-sig"><span class="sig-fn">error</span><span class="sig-punct">(</span><span class="sig-param">msg</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">,</span> <span class="sig-param">level</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">?</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-kw">nil</span></code></summary>


Raises `msg` as a runtime error, ending the program. The message is shown
exactly as written, followed by the stack trace. Present for familiarity
with Lua.

`level` says whose line the error points at, as in Lua: `1` (the default)
is the `error` call itself, `2` the call into the function that raised it,
`3` the call one further out. A function that checks its arguments raises
at level 2, so the error lands on the line that passed the bad value rather
than on the check. A level deeper than the stack points at the outermost
call. A level below 1 is itself an error. The stack trace is the same at any
level.

```saule
fn sqrtOf(n: float) -> float
    if n < 0.0 then
        error("sqrtOf needs a non-negative number, got " .. n, 2)
    end
    return Math.sqrt(n)
end

println(sqrtOf(4.0))             -- 2.0
```

**It is not the same as `throw`.** A value you `throw` can be caught by a
`catch` clause of the matching type; an `error` cannot be caught at all. Use
`throw` for a failure the program is meant to handle, and `error` only where
stopping is the point.

```saule
-- `throw` is catchable.
try
    throw "handled"
catch e: string
    println(e)                   -- handled
end

-- `error` is not, so this guard is the only thing standing between the
-- example and an ended program.
local shouldFail: boolean = false
if shouldFail then
    error("stops everything")
end
println("still running")         -- still running
```

Conversion between types is the `as` cast, not a function — `3.9 as integer`,
`n as string`, `"42" as integer` (which is `integer?`, since the text may hold
no number). See [Language guide §Casting](/saule/language/types/#casting).


</details>
</div>

### Prelude interfaces

These interfaces are in scope everywhere too, so a class can implement them
without an import. They carry no behaviour of their own — each one is a
contract the language itself looks for, and implementing it is how a class
opts into a piece of syntax that would otherwise be refused.

#### Operators

One row per operator. The method is what you write; the interface is what you
list in `implements` so the compiler knows to look for it.

| Operator | Interface | Method to define |
| --- | --- | --- |
| `a + b` | `OpAdd<T, R>` | `fn add(other: T) -> R` |
| `a - b` | `OpSub<T, R>` | `fn sub(other: T) -> R` |
| `a * b` | `OpMul<T, R>` | `fn mul(other: T) -> R` |
| `a / b` | `OpDiv<T, R>` | `fn div(other: T) -> R` |
| `a % b` | `OpMod<T, R>` | `fn mod(other: T) -> R` |
| `a ^ b` | `OpPow<T, R>` | `fn pow(other: T) -> R` |
| `a & b` | `OpBAnd<T, R>` | `fn band(other: T) -> R` |
| `a \| b` | `OpBOr<T, R>` | `fn bor(other: T) -> R` |
| `a ~ b` | `OpBXor<T, R>` | `fn bxor(other: T) -> R` |
| `a << b` | `OpShl<T, R>` | `fn shl(other: T) -> R` |
| `a >> b` | `OpShr<T, R>` | `fn shr(other: T) -> R` |
| `-a` | `OpNeg<R>` | `fn neg() -> R` |
| `~a` | `OpBNot<R>` | `fn bnot() -> R` |
| `#a` | `OpLen` | `fn len() -> integer` |
| `a .. b` | `OpConcat<T, R>` | `fn concat(other: T) -> R` |
| `a == b`, `a != b` | `OpEq<T>` | `fn equals(other: T) -> boolean` |
| `a < b`, `a <= b`, `a > b`, `a >= b` | `OpCompare<T>` | `fn compare(other: T) -> integer` |
| `a[k]` | `OpIndex<K, V>` | `fn index(key: K) -> V` |
| `a[k] = v` | `OpNewIndex<K, V>` | `fn newIndex(key: K, value: V) -> nil` |

`compare` answers the way C's `strcmp` does: negative when `self` sorts first,
zero when they tie, positive when `other` does. `OpIndex` / `OpNewIndex` are
Saule's `__index` / `__newindex`.

#### Everything else

| Syntax it enables | Interface | Method to define |
| --- | --- | --- |
| `for v in instance do` | `Iterable<T>` | `fn iter() -> fn() -> T?` |
| `for k, v in instance do` | `Iterable2<K, V>` | `fn iter() -> fn() -> (K?, V?)` |
| `tostring(a)`, `print(a)`, `a .. b` | `OpToString` | `fn toString() -> string` |
| `local a: C = value` | `Assignable<T>` | `static fn of(value: T) -> C` |

`Assignable` is the odd one: it lets a bare `T` fill a `C`-typed slot, with
`C.of` doing the conversion at the point of assignment.

#### Where the standard library looks for them

Implementing one of these does not only change what *your* code can write —
parts of the library consult them too, which is usually the reason to bother:

| Interface | What starts working |
| --- | --- |
| `OpEq<T>` | `Table.contains`, `Table.indexOf` and `Iter.unique` compare with `==`, so they match by your rule rather than by identity |
| `OpCompare<T>` | `Iter.sortBy` orders with `<`; `Table.sort` takes a comparator directly and needs no interface |
| `OpToString` | `tostring`, `print`, `println`, `String.join` and `Table.concat` all render through it |
| `Iterable<T>` | `Iter.collect(instance.iter())` drains it into a table, which is how an instance reaches the `Iter` combinators |
| `OpLen` | `#instance` |

#### A worked example

A money type that adds, sorts and prints — three interfaces, three methods:

```saule
class Money implements OpAdd<Money, Money>, OpCompare<Money>, OpToString
    local cents: integer

    fn init(cents: integer)
        self.cents = cents
    end

    fn add(other: Money) -> Money
        return Money(self.cents + other.cents)
    end

    fn compare(other: Money) -> integer
        return self.cents - other.cents
    end

    fn toString() -> string
        return "$" .. (self.cents / 100) .. "." .. String.padStart(tostring(self.cents % 100), 2, "0")
    end
end

local prices: table<Money> = {Money(1250), Money(399), Money(8000)}

-- `+` is `add`, and `tostring` is `toString`.
println(tostring(prices[1]! + prices[2]!))      -- $16.49

-- `Iter.sortBy` orders with `<`, which is `compare`.
local cheapest: Money = Iter.sortBy(prices, p => p)[1]!
println(tostring(cheapest))                     -- $3.99
```

The `<T>` on these is a real type argument, checked like any other: `OpAdd`
takes two (the operand and the result), `OpEq` one, `OpLen` none — supplying
the wrong number is an error rather than being ignored. See
[Custom Iterable](/saule/language/interfaces/#custom-iterable) and
[Operator Overloading](/saule/language/interfaces/#operator-overloading) for the dispatch
rules, and [Generic Classes](/saule/language/classes/#generic-classes) for declaring your
own generic types.

---
