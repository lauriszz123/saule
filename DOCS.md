# Saule Standard Library

This is the reference for everything Saule ships out of the box — the prelude
(`print`, `assert`, …) plus the static-class modules (`String`, `Bytes`,
`Math`, `Table`, `Iter`, `Io` / `File`, `Os`, `Saule`, `Project`). Every one of
them is in scope with no `import`.

For language syntax see [README.md](./README.md). For the runtime side of
errors / pattern matching / generics see the corresponding README sections.

> All function and method signatures are written in Saule's own type notation.
> `T?` is nullable, `(A, B)` is a multi-return tuple, `...T` is variadic, and
> `fn(A) -> R` is a callback slot — there is no bare `function` type.

<!--
  Code fences say `luau`, not `saule`: GitHub's Linguist has no Saule entry, so
  a fence tagged `saule` renders unhighlighted there. See the note at the top of
  README.md. `npm run sync-docs` rewrites them back to `saule` for the website.
-->

---

## Table of Contents

- [Prelude (always in scope)](#prelude-always-in-scope)
- [`String`](#string)
- [`Bytes`](#bytes)
- [`Math`](#math)
- [`Table`](#table)
- [`Iter`](#iter)
- [`Io` / `File`](#io--file)
- [`Os`](#os)
- [`Saule`](#saule)
- [`Project`](#project)

---

## Prelude (always in scope)

These names are bound at the top of every module — no import required, for
these or for any of the static classes below them.

### Printing

#### `print(...any) -> nil`

Writes its arguments to stdout separated by **tabs**, with no trailing
newline. Each is rendered as `tostring` would.

```saule
print("a", "b", "c")
print("\n")                   -- a	b	c

-- Any type, not just strings.
print(1, 2.5, true, nil)
print("\n")                   -- 1	2.5	true	nil
```

#### `println(...any) -> nil`

The same, with a trailing newline. This is the one to reach for.

```saule
println("hello")              -- hello
println(1, "two", 3.0)        -- 1	two	3.0
println()                     -- an empty line
```

#### `printf(fmt: string, ...any) -> nil`

Formats like [`String.format`](#string-format) and writes the result to
stdout, with **no** trailing newline.

```saule
printf("%s scored %d\n", "Ada", 98)       -- Ada scored 98
printf("%.2f%%\n", 99.5)                  -- 99.50%

-- No newline unless you write one.
printf("a")
printf("b\n")                             -- ab
```

### Inspecting a value

#### `tostring(v: any) -> string`

A readable rendering of any value. For a class instance implementing
[`OpToString`](#prelude-interfaces) this calls its `toString()`; `print`,
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

#### `type(v: any) -> string`

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

### Failing

#### `assert<T>(v: T?, msg: string?) -> T`

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

#### `error(msg: string) -> nil`

Raises `msg` as a runtime error, ending the program. Present for familiarity
with Lua.

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
no number). See [README §Casting](./README.md#casting).

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

```luau
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
[Custom Iterable](./README.md#custom-iterable) and
[Operator Overloading](./README.md#operator-overloading) for the dispatch
rules, and [Generic Classes](./README.md#generic-classes) for declaring your
own generic types.

---

## `String`

UTF-8 aware text utilities. Every index is **1-based** and counted in
**characters**, not bytes, so `String.sub` can never cut a multi-byte
character in half. A negative index counts from the end, where `-1` is the
last character.

Saule has **no pattern language**. Every function here operates on literal
text: the `.` in `String.replace(s, ".", "-")` is a full stop, not "any
character".

### Characters and codepoints

#### `String.byte(s: string, i: integer?) -> integer?`

The Unicode codepoint of the `i`-th character, or `nil` when `i` is outside
the string. `i` defaults to 1, so a bare `String.byte(s)` reads the first
character.

Nullable on purpose: a scan can walk off the end and stop, rather than
needing a length check in front of every read.

```saule
local word: string = "Saule"

println(String.byte(word) ?? 0)         -- 83, the codepoint of "S"
println(String.byte(word, 2) ?? 0)      -- 97, the codepoint of "a"
println(String.byte(word, 99) == nil)   -- true, past the end

-- Walking a string codepoint by codepoint, stopping at the end.
local i: integer = 1
while String.byte(word, i) != nil do
    println(i .. ": " .. (String.byte(word, i) ?? 0))
    i = i + 1
end
```

#### `String.char(...integer) -> string`

Builds a string from codepoints. The inverse of `String.byte`.

Note what this is **not**: `String.char(0x80)` is the two-byte UTF-8
encoding of U+0080, because the result is text. For the single byte `0x80`,
reach for [`Bytes.of`](#bytes).

```saule
println(String.char(72, 105))           -- Hi
println(String.char(83, 97, 117, 108, 101))  -- Saule

-- Codepoints past ASCII work the same way.
println(String.char(0x160))             -- Š

-- Round trip, through `byte` and back.
local source: string = "Hi"
println(String.char(String.byte(source, 1) ?? 0, String.byte(source, 2) ?? 0))  -- Hi
```

#### `String.iter(s: string) -> fn() -> (string?, integer?)`

A step closure over the characters of `s`, yielding each character and its
1-based index. Written for `for`, which stops when the closure answers `nil`.

```saule
for ch, i in String.iter("hey") do
    println(i .. ": " .. ch)     -- 1: h, then 2: e, then 3: y
end

-- Counting characters that match something.
local vowels: integer = 0
for ch in String.iter("saule") do
    if String.contains("aeiou", ch) then
        vowels = vowels + 1
    end
end
println(vowels)                  -- 3
```

### Length and slicing

#### `String.len(s: string) -> integer`

The number of **characters**, which is not the number of bytes. `#s` is not
defined for a string — use this, and use `#Bytes.fromString(s)` when the
byte count is what you want.

```saule
local word: string = "saulė"

println(String.len(word))                  -- 5 characters
println(#Bytes.fromString(word))           -- 6 bytes: "ė" takes two

-- Which is why slicing is by character, not by byte.
println(String.sub(word, 5, 5))            -- ė
```

#### `String.sub(s: string, from: integer, to: integer?) -> string`

The substring from `from` to `to`, both inclusive and 1-based. `to` defaults
to the end of the string. Either index may be negative to count from the
end, and both clamp, so an out-of-range slice is empty rather than an error.

```saule
local word: string = "Saule"

println(String.sub(word, 2, 4))     -- aul
println(String.sub(word, 3))        -- ule, to the end
println(String.sub(word, -3))       -- ule, the last three
println(String.sub(word, -3, -2))   -- ul
println(String.sub(word, 10, 20))   -- (empty, clamped)

-- Trimming a known prefix.
local path: string = "src/main.sau"
if String.starts(path, "src/") then
    println(String.sub(path, 5))    -- main.sau
end
```

#### `String.rep(s: string, n: integer) -> string`

`s` repeated `n` times, with nothing between the copies. `n` of zero or less
gives the empty string.

```saule
println(String.rep("ab", 3))        -- ababab
println(String.rep("-", 20))        -- --------------------
println(String.rep("x", 0))         -- (empty)

-- A quick ruler under a heading.
local title: string = "Report"
println(title)
println(String.rep("=", String.len(title)))   -- ======
```

### Case

#### `String.lower(s: string) -> string`

Lowercases `s`. Unicode-aware, not ASCII-only: `Š` folds to `š` and `Ė` to
`ė`, so this is correct for text in any language the string can hold.

```saule
println(String.lower("SAULE"))        -- saule
println(String.lower("Mixed Case"))   -- mixed case
println(String.lower("ŠALTIS"))       -- šaltis, past ASCII too

-- Case-insensitive comparison, ASCII only.
local typed: string = "YES"
println(String.lower(typed) == "yes")   -- true
```

#### `String.upper(s: string) -> string`

Uppercases `s`, Unicode-aware in the same way as `String.lower`.

```saule
println(String.upper("saule"))        -- SAULE
println(String.upper("Mixed Case"))   -- MIXED CASE
println(String.upper("saulė"))        -- SAULĖ

-- Initial-capitalising a word.
local name: string = "ada"
println(String.upper(String.sub(name, 1, 1)) .. String.sub(name, 2))   -- Ada
```

### Searching

#### `String.find(s: string, needle: string, from: integer?) -> (integer?, integer?)`

The 1-based start and end indices of the first occurrence of `needle`, or
`nil, nil` when there is none. `from` says where to start looking and
defaults to 1.

Two values, so you can slice around the match without measuring it. For just
the start index, [`String.indexOf`](#string-indexof) answers the same
question with one value.

```saule
local line: string = "name = Saule"

local s: integer?, e: integer? = String.find(line, " = ")
if s != nil and e != nil then
    println(String.sub(line, 1, s! - 1))   -- name
    println(String.sub(line, e! + 1))      -- Saule
end

-- A miss answers nil for both.
local missS: integer?, missE: integer? = String.find(line, "zzz")
println(missS == nil and missE == nil)     -- true

-- `from` walks through every occurrence. `String.indexOf` is the one-value
-- form, which is what a loop like this wants. The match is declared *inside*
-- the loop: a nullable narrowed by `!= nil` stays narrowed for the rest of
-- the block, so it cannot be assigned a nullable again.
local text: string = "a,b,c"
local start: integer = 1
while true do
    local at: integer? = String.indexOf(text, ",", start)
    if at == nil then
        break
    end
    println("comma at " .. at!)            -- 2, then 4
    start = at! + 1
end
```

#### `String.indexOf(s: string, needle: string, from: integer?) -> integer?`

The 1-based character index where `needle` first appears, or `nil` when it
does not. `from` says where to start and defaults to 1.

[`String.find`](#string-find) answers the same question and also gives the
end index.

```saule
println(String.indexOf("hello", "ll") ?? 0)      -- 3
println(String.indexOf("hello", "z") == nil)     -- true
println(String.indexOf("a,b,a", "a", 2) ?? 0)    -- 5, searching from 2

-- Splitting on the first separator only.
local setting: string = "path=/usr/local/bin"
local at: integer? = String.indexOf(setting, "=")
if at != nil then
    println(String.sub(setting, 1, at! - 1))     -- path
    println(String.sub(setting, at! + 1))        -- /usr/local/bin
end
```

#### `String.contains(s: string, needle: string) -> boolean`

Whether `needle` appears anywhere in `s`. An empty needle is always present.

```saule
println(String.contains("Saule", "aul"))   -- true
println(String.contains("Saule", "xyz"))   -- false

-- Filtering a list of names.
local names: table<string> = {"ada.sau", "readme.md", "main.sau"}
for name in names do
    if String.contains(name, ".sau") then
        println(name)                      -- ada.sau, then main.sau
    end
end
```

#### `String.starts(s: string, prefix: string) -> boolean`

Whether `s` begins with `prefix`. An empty prefix is always present.

```saule
println(String.starts("Saule", "Sau"))    -- true
println(String.starts("Saule", "aul"))    -- false, not at the start
println(String.starts("Saule", ""))       -- true

-- Routing on a prefix.
local arg: string = "--verbose"
if String.starts(arg, "--") then
    println("long flag: " .. String.sub(arg, 3))   -- long flag: verbose
end
```

#### `String.ends(s: string, suffix: string) -> boolean`

Whether `s` ends with `suffix`. An empty suffix is always present.

```saule
println(String.ends("main.sau", ".sau"))   -- true
println(String.ends("main.sau", ".lua"))   -- false

-- Picking a handler by file extension.
local file: string = "notes.md"
if String.ends(file, ".md") then
    println("markdown")
elseif String.ends(file, ".sau") then
    println("saule")
end
```

### Splitting

#### `String.split(s: string, sep: string) -> table<string>`

Splits `s` on every occurrence of `sep`. The result always holds one more
piece than there were separators, so splitting the empty string gives one
empty piece rather than none. An empty `sep` splits into single characters.

```saule
local parts: table<string> = String.split("id,name,score", ",")
println(#parts)                       -- 3
println(parts[2] ?? "")               -- name

-- The piece count is always separators + 1.
println(#String.split("a,,b", ","))   -- 3: "a", "", "b"
println(#String.split("", ","))       -- 1, one empty piece

-- An empty separator gives characters.
println(#String.split("abc", ""))     -- 3

-- Parsing a simple key=value line.
local pair: table<string> = String.split("colour=amber", "=")
println((pair[1] ?? "") .. " is " .. (pair[2] ?? ""))   -- colour is amber
```

### Joining

#### `String.join<V>(sep: string, parts: table<V>) -> string`

Joins `parts` with `sep` between them, rendering each element the way
`tostring` would. The same operation as
[`Table.concat`](/saule/stdlib/table/#table-concat), with the arguments the
other way round so the call reads as a sentence.

```saule
println(String.join(", ", {"a", "b", "c"}))   -- a, b, c
println(String.join(" | ", {1, 2, 3}))        -- 1 | 2 | 3
println(String.join("", {"a", "b"}))          -- ab

-- Round-tripping a split.
local csv: string = "x,y,z"
println(String.join(",", String.split(csv, ",")) == csv)   -- true
```

### Trimming

#### `String.trim(s: string) -> string`

Removes whitespace from both ends. Unicode whitespace, not only the ASCII
space — a non-breaking space goes too.

```saule
println("[" .. String.trim("   hi   ") .. "]")    -- [hi]
println("[" .. String.trim("\t hi \n") .. "]")     -- [hi]
println("[" .. String.trim("   ") .. "]")         -- []

-- Cleaning a line of input before testing it.
local typed: string = "  yes  "
println(String.trim(typed) == "yes")              -- true
```

#### `String.trimStart(s: string) -> string`

Removes whitespace from the start only.

```saule
println("[" .. String.trimStart("  hi  ") .. "]")   -- [hi  ]

-- Measuring indentation.
local line: string = "    indented"
println(String.len(line) - String.len(String.trimStart(line)))   -- 4
```

#### `String.trimEnd(s: string) -> string`

Removes whitespace from the end only.

```saule
println("[" .. String.trimEnd("  hi  ") .. "]")   -- [  hi]

-- Dropping a trailing newline without touching leading indentation.
local line: string = "  code\n"
println("[" .. String.trimEnd(line) .. "]")       -- [  code]
```

### Replacing

#### `String.replace(s: string, from: string, to: string, limit: integer?) -> string`

Replaces every occurrence of `from` with `to`, or only the first `limit` of
them when `limit` is given. Literal text, not a pattern. An empty `from`
matches nothing and returns `s` unchanged, so this can never loop forever.

```saule
println(String.replace("a.b.c", ".", "/"))        -- a/b/c
println(String.replace("a.b.c", ".", "/", 1))     -- a/b.c, first only
println(String.replace("aaa", "a", ""))           -- (empty), deleting
println(String.replace("abc", "", "-"))           -- abc, unchanged

-- The `.` is a full stop, not "any character".
println(String.replace("a1b", ".", "!"))          -- a1b, nothing matched
```

### Padding

#### `String.padStart(s: string, width: integer, fill: string?) -> string`

Pads `s` on the **left** until it is `width` characters long. `fill` defaults
to a single space and repeats if it is longer than one character. Never
truncates: a string already at or past `width` comes back unchanged.

```saule
println(String.padStart("7", 3, "0"))        -- 007
println(String.padStart("7", 3))             -- "  7"
println(String.padStart("toolong", 3, "0"))  -- toolong, never cut

-- Right-aligning a column of numbers.
for n in {5, 42, 7} do
    println(String.padStart(tostring(n), 4, " "))   --    5,   42,    7
end
```

#### `String.padEnd(s: string, width: integer, fill: string?) -> string`

The same, padding on the **right**.

```saule
println(String.padEnd("7", 3, "0"))      -- 700
println(String.padEnd("hp", 6) .. "|")   -- hp    |

-- Laying out a two-column table without `String.format`.
local rows: table<string> = {"name", "id"}
for key in rows do
    println(String.padEnd(key, 8) .. ": set")   -- name    : set, id      : set
end
```

### Formatting

#### `String.format(fmt: string, ...any) -> string`

C-style formatting. The specifiers are `%d` and `%i` (integer), `%f` and
`%g` (float), `%s` (any value, rendered as `tostring` would), `%x` and `%X`
(hexadecimal), `%o` (octal), `%c` (a character from a codepoint) and `%%`
for a literal percent sign.

Width, precision and flags work as they do in C: `%-5s` left-aligns in five
columns, `%05.1f` pads with zeros to five columns with one decimal place.

```saule
println(String.format("%d items", 42))              -- 42 items
println(String.format("%.2f", 3.14159))             -- 3.14
println(String.format("%-6s|", "hp"))               -- hp    |
println(String.format("%06.2f", 3.14159))           -- 003.14
println(String.format("%x and %X", 255, 255))       -- ff and FF
println(String.format("%s scored %d%%", "Ada", 98)) -- Ada scored 98%

-- `%s` takes anything, including a class that implements `OpToString`.
println(String.format("[%s] [%s]", true, 1.5))      -- [true] [1.5]
```

## `Bytes`

A `bytes` is a buffer of arbitrary bytes with no encoding promised, measured
and indexed in bytes. It is a primitive type alongside `string`, and the two
are **not interchangeable**: a `string` is text, always valid UTF-8, measured
in codepoints.

Reach for `bytes` whenever the data is not text — an image, a digest, a
compressed blob, a wire protocol. Reach for `string` for everything else.

```saule
local raw: bytes = Bytes.of(0x89, 0x50, 0x4e, 0x47)

println(#raw)                            -- 4, a byte count
println(Bytes.toHex(raw))                -- 89504e47
println(Bytes.toString(raw) == nil)      -- true, not valid UTF-8
```

Lua makes one type do both jobs, which is why `#s` there is a byte count and
`s:sub(1, 1)` can cut a character in half. Keeping them apart lets the
typechecker enforce the difference, so a buffer can never be mistaken for
text.

Two things `bytes` deliberately does **not** do. `..` does not accept it —
concatenation builds a string, and a buffer that is not text has no string
form, so use `Bytes.concat`. And it cannot be a table key: key by
`Bytes.toHex(b)` instead. Equality is by contents, and a `bytes` is never
equal to a `string`.

Indices are **1-based**, as everywhere else in Saule.

### Building

#### `Bytes.of(...byte) -> bytes`

Builds a buffer from byte values. The arguments are typed
[`byte`](../README.md#the-byte-type), so a literal outside `0..=255` is a
**compile** error rather than a runtime surprise, and a computed value has to
narrow with `as byte` first.

Note the difference from `String.char`: `Bytes.of(0x80)` is **one** byte,
where `String.char(0x80)` is the two-byte UTF-8 encoding of U+0080.

```saule
local png: bytes = Bytes.of(0x89, 0x50, 0x4e, 0x47)
println(Bytes.toHex(png))                -- 89504e47
println(#png)                            -- 4

-- One byte, not a character.
println(#Bytes.of(0x80))                 -- 1
println(#Bytes.fromString(String.char(0x80)))   -- 2

-- A computed value narrows explicitly.
local n: integer = 200
local b: byte? = n as byte
println(Bytes.toHex(Bytes.of(b!)))       -- c8
```

#### `Bytes.empty() -> bytes`

The empty buffer. Useful as a starting value for an accumulator, and as the
fallback when a read returns nothing.

```saule
println(#Bytes.empty())                  -- 0
println(Bytes.toHex(Bytes.empty()))      -- (empty)

-- Accumulating.
local acc: bytes = Bytes.empty()
for n in {1, 2, 3} do
    acc = Bytes.concat(acc, Bytes.of((n as byte) ?? 0))
end
println(Bytes.toHex(acc))                -- 010203
```

#### `Bytes.concat(...bytes) -> bytes`

Joins buffers in order. This is what `..` would do if it accepted `bytes` —
it does not, because concatenation builds a string.

```saule
local a: bytes = Bytes.of(0x01, 0x02)
local b: bytes = Bytes.of(0x03)

println(Bytes.toHex(Bytes.concat(a, b)))            -- 010203
println(Bytes.toHex(Bytes.concat(a, b, a)))         -- 0102030102
println(#Bytes.concat(Bytes.empty(), a))            -- 2

-- Building a framed message: one length byte, then the body. The length is
-- parenthesised because `as` binds tighter than `#`.
local body: bytes = Bytes.fromString("hi")
local size: byte = ((#body) as byte) ?? 0
println(Bytes.toHex(Bytes.concat(Bytes.of(size), body)))   -- 026869
```

### Converting to and from `string`

#### `Bytes.fromString(s: string) -> bytes`

The UTF-8 bytes of `s`. Always succeeds, because every Saule string is
already valid UTF-8.

```saule
println(Bytes.toHex(Bytes.fromString("Hi")))   -- 4869
println(#Bytes.fromString("saulė"))            -- 6 bytes
println(String.len("saulė"))                   -- 5 characters

-- The byte count of a string, which `String.len` does not give you.
local text: string = "héllo"
println(#Bytes.fromString(text))               -- 6
```

#### `Bytes.toString(b: bytes) -> string?`

The buffer as text, or `nil` when it is not valid UTF-8.

Nullable on purpose: there is no total function from bytes to text, and
silently substituting U+FFFD would hide the problem at exactly the point it
matters.

```saule
println(Bytes.toString(Bytes.of(0x68, 0x69)) ?? "?")   -- hi
println(Bytes.toString(Bytes.of(0xff)) == nil)         -- true

-- The shape this is written for: decode, or fall back.
local payload: bytes = Bytes.fromString("ok")
local text: string = Bytes.toString(payload) ?? "<binary>"
println(text)                                          -- ok
```

### Inspecting

#### `Bytes.len(b: bytes) -> integer`

The byte count. `#b` is the same thing, as a value you can pass around.

```saule
local raw: bytes = Bytes.of(1, 2, 3)
println(Bytes.len(raw))        -- 3
println(#raw)                  -- 3, the same number
println(Bytes.len(Bytes.empty()))   -- 0
```

#### `Bytes.byte(b: bytes, i: integer) -> byte?`

The byte at `i`, or `nil` when `i` is outside the buffer. The result is a
[`byte`](../README.md#the-byte-type), which is in `0..=255` by construction
and widens to `integer` for free.

```saule
local raw: bytes = Bytes.of(0x7f, 0x80, 0xff)

println(Bytes.byte(raw, 1) ?? 0)     -- 127
println(Bytes.byte(raw, 2) ?? 0)     -- 128
println(Bytes.byte(raw, 9) == nil)   -- true

-- Summing a buffer, stopping cleanly at the end.
local total: integer = 0
local i: integer = 1
while Bytes.byte(raw, i) != nil do
    total = total + (Bytes.byte(raw, i) ?? 0)
    i = i + 1
end
println(total)                       -- 510
```

### Slicing

#### `Bytes.sub(b: bytes, from: integer, to: integer?) -> bytes`

The slice from `from` to `to`, both inclusive and 1-based. `to` defaults to
the end, negatives count from the end, and both ends clamp — so an
out-of-range slice is empty rather than an error.

```saule
local raw: bytes = Bytes.of(1, 2, 3, 4, 5)

println(Bytes.toHex(Bytes.sub(raw, 2, 4)))    -- 020304
println(Bytes.toHex(Bytes.sub(raw, 3)))       -- 030405
println(Bytes.toHex(Bytes.sub(raw, -2)))      -- 0405
println(#Bytes.sub(raw, 99, 120))             -- 0, clamped

-- Splitting a header off a payload.
local frame: bytes = Bytes.of(0x02, 0xaa, 0xbb)
println(Bytes.toHex(Bytes.sub(frame, 1, 1)))  -- 02, the length byte
println(Bytes.toHex(Bytes.sub(frame, 2)))     -- aabb, the body
```

### Searching

#### `Bytes.indexOf(b: bytes, needle: bytes, from: integer?) -> integer?`

The 1-based position where `needle` first appears, or `nil` when it does not.
`from` says where to start. An empty needle matches at `from`.

```saule
local raw: bytes = Bytes.of(0x00, 0x7f, 0x80, 0x7f)

println(Bytes.indexOf(raw, Bytes.of(0x80)) ?? 0)       -- 3
println(Bytes.indexOf(raw, Bytes.of(0x7f)) ?? 0)       -- 2, the first one
println(Bytes.indexOf(raw, Bytes.of(0x7f), 3) ?? 0)    -- 4, searching on
println(Bytes.indexOf(raw, Bytes.of(0xee)) == nil)     -- true

-- Finding a delimiter in a buffer.
local packet: bytes = Bytes.fromString("key=value")
local at: integer? = Bytes.indexOf(packet, Bytes.fromString("="))
if at != nil then
    println(Bytes.toString(Bytes.sub(packet, 1, at! - 1)) ?? "")   -- key
end
```

### Hexadecimal

#### `Bytes.toHex(b: bytes) -> string`

Lowercase hexadecimal, two characters per byte.

This is how a buffer gets looked at — `print(b)` shows only `<bytes len=N>` —
and how one becomes a table key, since `bytes` is not keyable itself.

```saule
println(Bytes.toHex(Bytes.of(0, 255)))        -- 00ff
println(Bytes.toHex(Bytes.of(0xde, 0xad)))    -- dead
println(Bytes.toHex(Bytes.empty()))           -- (empty)

-- Keying a table by a buffer's contents.
local seen: table<string, boolean> = {}
local digest: bytes = Bytes.of(0x01, 0x02)
seen[Bytes.toHex(digest)] = true
println(seen["0102"] ?? false)                -- true
```

#### `Bytes.fromHex(s: string) -> bytes?`

Parses hexadecimal back into a buffer. `nil` for an odd number of digits or
any non-hex character, rather than a buffer that is quietly half right.

```saule
println(Bytes.toHex(Bytes.fromHex("00ff")!))   -- 00ff
println(Bytes.fromHex("0f0") == nil)           -- true, odd length
println(Bytes.fromHex("zz") == nil)            -- true, not hex

-- Round trip.
local raw: bytes = Bytes.of(0xca, 0xfe)
println(Bytes.toHex(Bytes.fromHex(Bytes.toHex(raw))!) == Bytes.toHex(raw))   -- true
```

### Byte arrays

#### `Bytes.toArray(b: bytes) -> byte[]`

The buffer as one element per byte. The expensive direction — every byte
becomes a boxed table element — and explicit about it.

```saule
local raw: bytes = Bytes.of(10, 20, 30)
local list: byte[] = Bytes.toArray(raw)

println(#list)                    -- 3
println(list[2] ?? 0)             -- 20

-- Which makes the whole sequence API available.
println(Iter.reduce(list, 0, (acc, n) => acc + n))   -- 60
```

#### `Bytes.fromArray(a: byte[]) -> bytes?`

Packs elements back into a buffer. `nil` when an element is out of range —
which the typechecker prevents for a `byte[]`, but not for a table that
arrived through `any`.

```saule
println(Bytes.toHex(Bytes.fromArray({1, 2, 3})!))    -- 010203
println(#Bytes.fromArray({})!)                       -- 0

-- Round trip through the elementwise form.
local raw: bytes = Bytes.of(0xaa, 0xbb)
println(Bytes.toHex(Bytes.fromArray(Bytes.toArray(raw))!))   -- aabb
```

### `bytes` and `byte[]`

A `bytes` is the packed representation; a `byte[]` (that is,
`table<integer, byte>`) is the elementwise one. `Bytes.toArray` and
`Bytes.fromArray` convert between them, and they are two explicit functions
rather than an implicit conversion because the costs are not comparable: a
`bytes` is one byte per byte, while a table holds a boxed value per element.

Reach for `bytes` to hold or move bytes — a file, a socket frame, a digest,
anything crossing into a native package. Reach for `byte[]` only to work
through bytes one at a time, where a loop over elements is clearer than
repeated `Bytes.byte` calls.

## `Math`

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

#### `Math.floor<N>(n: N) -> integer`

Rounds toward negative infinity. An `integer` comes back unchanged.

```saule
println(Math.floor(2.9))      -- 2
println(Math.floor(2.1))      -- 2
println(Math.floor(-2.1))     -- -3, toward -inf, not toward zero
println(Math.floor(5))        -- 5, already an integer
```

#### `Math.ceil<N>(n: N) -> integer`

Rounds toward positive infinity.

```saule
println(Math.ceil(2.1))       -- 3
println(Math.ceil(-2.9))      -- -2
println(Math.ceil(5))         -- 5

-- How many pages of ten a list needs.
local items: integer = 23
println(Math.ceil((items as float) / 10.0))   -- 3
```

#### `Math.round<N>(n: N) -> integer`

Rounds to the nearest integer, with a half rounding **away from zero** —
`2.5` goes to `3` and `-2.5` to `-3`. This is not banker's rounding: a `.5`
never rounds toward even.

```saule
println(Math.round(2.4))      -- 2
println(Math.round(2.5))      -- 3, away from zero
println(Math.round(-2.5))     -- -3
println(Math.round(3.5))      -- 4
```

### Sign

#### `Math.sign<N>(n: N) -> integer`

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

### Numeric kind

#### `Math.type(v: any) -> string?`

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

### Arithmetic

#### `Math.abs<N>(n: N) -> N`

Magnitude, in the flavour it was given.

```saule
println(Math.abs(-8))         -- 8, an integer
println(Math.abs(-2.5))       -- 2.5, a float
println(Math.abs(0))          -- 0

-- Distance between two numbers, direction discarded.
println(Math.abs(3 - 10))     -- 7
```

#### `Math.min<N>(...N) -> N`

The smallest of its arguments, which must all share one flavour.

```saule
println(Math.min(3, 1, 7))          -- 1
println(Math.min(2.5, 1.5))         -- 1.5
println(Math.min(5))                -- 5, one argument is fine
```

#### `Math.max<N>(...N) -> N`

The largest of its arguments.

```saule
println(Math.max(3, 1, 7))          -- 7
println(Math.max(2.5, 1.5))         -- 2.5

-- A floor under a computed value, without an `if`.
local damage: integer = 2
println(Math.max(damage - 5, 0))    -- 0
```

#### `Math.clamp<N>(n: N, lo: N, hi: N) -> N`

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

#### `Math.fmod<N>(a: N, b: N) -> N`

The remainder after truncated division, in the flavour it was given. The
sign follows the **left** operand, which is also what the `%` operator does —
unlike Lua, where `%` floors and takes the sign of the right one.

```saule
println(Math.fmod(7, 3))        -- 1
println(Math.fmod(-7, 3))       -- -1, the sign of the left operand
println(-7 % 3)                 -- -1, the operator agrees
println(Math.fmod(7.5, 2.0))    -- 1.5
```

#### `Math.modf<N>(n: N) -> table<float>`

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

#### `Math.pow<N>(a: N, b: N) -> float`

`a` raised to the power `b`. **Always a float**, which is what lets it answer
a negative exponent — `2 ^ -1` is an error on integers, while
`Math.pow(2, -1)` is `0.5`.

```saule
println(Math.pow(2, 10))      -- 1024.0, a float even from integers
println(Math.pow(2, -1))      -- 0.5, where `2 ^ -1` is an error
println(Math.pow(9.0, 0.5))   -- 3.0, both arguments share one flavour
```

#### `Math.sqrt<N>(n: N) -> float`

The square root.

```saule
println(Math.sqrt(9))         -- 3.0
println(Math.sqrt(2))         -- 1.4142135623730951

-- The hypotenuse of a right triangle.
local a: float = 3.0
local b: float = 4.0
println(Math.sqrt(a * a + b * b))   -- 5.0
```

#### `Math.exp<N>(n: N) -> float`

`e` raised to the power `n`.

```saule
println(Math.exp(0))          -- 1.0
println(Math.exp(1))          -- 2.718281828459045
println(Math.exp(1) == Math.e)   -- true
```

#### `Math.log<N>(n: N, base: N?) -> float`

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

### Trigonometry

All of these take an `integer` or a `float` and answer a `float` — an angle
is a real number, so there is no integer-in / integer-out form the way there
is for `abs` or `clamp`.

#### `Math.sin<N>(n: N) -> float`

Sine of `n` **radians**.

```saule
println(Math.sin(0))                      -- 0.0
println(Math.sin(Math.pi / 2.0))          -- 1.0

-- Degrees go through `rad` first.
println(Math.sin(Math.rad(90)))           -- 1.0
```

#### `Math.cos<N>(n: N) -> float`

Cosine of `n` radians.

```saule
println(Math.cos(0))                      -- 1.0
println(Math.cos(Math.pi))                -- -1.0

-- A point on the unit circle, a third of the way round.
local angle: float = Math.rad(120)
println(Math.round(Math.cos(angle) * 100.0))   -- -50
```

#### `Math.tan<N>(n: N) -> float`

Tangent of `n` radians.

```saule
println(Math.tan(0))                      -- 0.0
println(Math.round(Math.tan(Math.rad(45))))   -- 1
```

#### `Math.asin<N>(n: N) -> float`

Arcsine, in radians. `n` outside `[-1, 1]` has no answer and gives `nan`.

```saule
println(Math.asin(0))                     -- 0.0
println(Math.asin(1) == Math.pi / 2.0)    -- true
```

#### `Math.acos<N>(n: N) -> float`

Arccosine, in radians.

```saule
println(Math.acos(1))                     -- 0.0
println(Math.acos(0) == Math.pi / 2.0)    -- true
```

#### `Math.atan<N>(y: N, x: N?) -> float`

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

#### `Math.deg<N>(radians: N) -> float`

Radians to degrees.

```saule
println(Math.deg(Math.pi))                -- 180.0
println(Math.deg(Math.pi / 2.0))          -- 90.0
```

#### `Math.rad<N>(degrees: N) -> float`

Degrees to radians.

```saule
println(Math.rad(180))                    -- 3.141592653589793
println(Math.rad(180) == Math.pi)         -- true

-- Round tripping.
println(Math.deg(Math.rad(45)))           -- 45.0
```

### Randomness

#### `Math.random() -> float`

A uniform float in `[0, 1)` — zero is possible, one is not.

```saule
Math.randomseed(42)
local r: float = Math.random()
println(r >= 0.0 and r < 1.0)     -- true
```

#### `Math.random(n: integer) -> integer`

A uniform integer in `[1, n]`, both ends included.

```saule
Math.randomseed(42)
local roll: integer = Math.random(6)
println(roll >= 1 and roll <= 6)   -- true
```

#### `Math.random(lo: integer, hi: integer) -> integer`

A uniform integer in `[lo, hi]`, both ends included.

```saule
Math.randomseed(42)
local n: integer = Math.random(10, 20)
println(n >= 10 and n <= 20)       -- true

-- Picking an element out of a table.
local names: table<string> = {"ada", "grace", "alan"}
println(names[Math.random(1, #names)] != nil)   -- true
```

#### `Math.randomseed(seed: integer) -> nil`

Resets the generator, so a sequence of `Math.random` calls repeats exactly.
This is what makes a test or a procedurally generated world reproducible.

```saule
Math.randomseed(42)
local first: integer = Math.random(1000)

Math.randomseed(42)
local again: integer = Math.random(1000)

println(first == again)       -- true, the same seed gives the same sequence
```

### Unsigned comparison

#### `Math.ult(a: integer, b: integer) -> boolean`

Compares two integers as **unsigned** 64-bit values. A negative integer has
its top bit set, so it reads as a very large unsigned number.

```saule
println(Math.ult(1, 2))           -- true
println(Math.ult(-1, 1))          -- false: -1 is the largest unsigned value
println(1 < 2)                    -- true, the signed comparison agrees here
println(-1 < 1)                   -- true, where the signed one differs
```

## `Table`

A table is one structure doing two jobs: an **array** keyed by consecutive
integers from 1, and a **map** keyed by strings, integers or booleans. Most
of what is here works on the array side; `keys` and `values` see both, and
map access itself is just `t[k]` and `t[k] = v`.

`Table` is the **mutating** half of the sequence API: it changes the table it
is given, or answers a question about one. For deriving a *new* sequence —
`map`, `filter`, `reduce` — see [`Iter`](#iter). No name means two different
things across the two halves: `Table.reverse` reverses in place,
`Iter.reverse` returns a new table.

```saule
local scores: table<integer> = {10, 20, 30}
local byName: table<string, integer> = {ada: 1, grace: 2}

println(#scores)              -- 3, the array length
println(scores[2] ?? 0)       -- 20, 1-based
println(byName["ada"] ?? 0)   -- 1
```

### Changing a table

#### `Table.insert<V>(t: table<V>, value: V, pos: integer?) -> nil`

Appends `value` to the end of the array part, or inserts it at `pos`,
shifting everything from `pos` onward one place to the right.

```saule
local xs: table<integer> = {1, 2, 3}

Table.insert(xs, 4)
println(Table.concat(xs, ","))      -- 1,2,3,4

Table.insert(xs, 0, 1)
println(Table.concat(xs, ","))      -- 0,1,2,3,4

-- Building a list in a loop.
local squares: table<integer> = {}
for n = 1, 4 do
    Table.insert(squares, n * n)
end
println(Table.concat(squares, " "))  -- 1 4 9 16
```

#### `Table.remove<V>(t: table<V>, pos: integer?) -> V?`

Removes and returns the last element, or the one at `pos`, shifting the rest
left. `nil` when there is nothing to remove, which is what makes a table
usable as a stack without a separate emptiness check.

```saule
local stack: table<integer> = {1, 2, 3}

println(Table.remove(stack) ?? 0)       -- 3, from the end
println(Table.concat(stack, ","))       -- 1,2
println(Table.remove(stack, 1) ?? 0)    -- 1, from the front
println(Table.concat(stack, ","))       -- 2

-- Draining a stack until it is empty.
local queue: table<string> = {"a", "b"}
while #queue > 0 do
    println(Table.remove(queue, 1) ?? "")   -- a, then b
end
println(Table.remove(queue) == nil)         -- true, nothing left
```

#### `Table.sort<V>(t: table<V>, cmp: fn(V, V) -> boolean) -> nil`

Sorts the array part in place. `cmp(a, b)` answers `true` when `a` must come
before `b`.

For ordering by an extracted key rather than a comparator, see
[`Iter.sortBy`](/saule/stdlib/iter/#iter-sortby), which returns a new table.

```saule
local xs: table<integer> = {3, 1, 4, 1, 5}

Table.sort(xs, (a, b) => a < b)
println(Table.concat(xs, ","))       -- 1,1,3,4,5

Table.sort(xs, (a, b) => a > b)
println(Table.concat(xs, ","))       -- 5,4,3,1,1

-- Sorting by something other than the value itself. The comparator's
-- parameters take their types from the table, so they need no annotation.
local words: table<string> = {"pear", "fig", "banana"}
Table.sort(words, (a, b) => String.len(a) < String.len(b))
println(Table.concat(words, ","))    -- fig,pear,banana
```

#### `Table.reverse<V>(t: table<V>) -> nil`

Reverses the array part in place. [`Iter.reverse`](/saule/stdlib/iter/#iter-reverse)
returns a new table instead.

```saule
local xs: table<integer> = {1, 2, 3}
Table.reverse(xs)
println(Table.concat(xs, ","))      -- 3,2,1

-- In place: the original binding changes.
local letters: table<string> = {"a", "b", "c"}
Table.reverse(letters)
println(letters[1] ?? "")           -- c
```

#### `Table.clear<V>(t: table<V>) -> nil`

Removes every entry, array part and map part alike. The table itself stays
the same table, so anything else holding a reference to it sees it empty.

```saule
local named: table<string, integer> = {a: 1, b: 2}
Table.clear(named)
println(named["a"] == nil)  -- true

local xs: table<integer> = {1, 2, 3}
Table.clear(xs)
println(#xs)                -- 0

-- Reusing a buffer rather than allocating a new one.
local batch: table<integer> = {}
for round = 1, 2 do
    Table.clear(batch)
    Table.insert(batch, round)
    println(#batch)         -- 1, both times
end
```

### Reading a table

#### `Table.len<V>(t: table<V>) -> integer`

The array length — the same number `#t` gives, as a value you can pass
around.

```saule
println(Table.len({1, 2, 3}))        -- 3
println(Table.len({}))               -- 0

-- `#` is the operator form; this is the one you can hand to something else.
local sizes: table<integer> = Iter.map({{1}, {1, 2}}, t => Table.len(t))
println(Table.concat(sizes, ","))    -- 1,2
```

#### `Table.contains<V>(t: table<V>, value: V) -> boolean`

Whether `value` appears in the array part. Compares with `==`, so a class
implementing [`OpEq`](#prelude-interfaces) is matched by its own rule rather
than by identity.

```saule
println(Table.contains({1, 2, 3}, 2))        -- true
println(Table.contains({1, 2, 3}, 9))        -- false
println(Table.contains({"a", "b"}, "b"))     -- true

-- Filtering out what is already known.
local seen: table<string> = {"ada"}
for name in {"ada", "grace"} do
    if not Table.contains(seen, name) then
        println("new: " .. name)             -- new: grace
    end
end
```

#### `Table.indexOf<V>(t: table<V>, value: V) -> integer?`

The 1-based position of the first `==` match, or `nil`. For a **predicate**
search, use [`Iter.findIndex`](/saule/stdlib/iter/#iter-findindex).

```saule
println(Table.indexOf({"a", "b", "c"}, "b") ?? 0)    -- 2
println(Table.indexOf({1, 2}, 9) == nil)             -- true

-- Removing a known value.
local xs: table<string> = {"a", "b", "c"}
local at: integer? = Table.indexOf(xs, "b")
if at != nil then
    Table.remove(xs, at!)
end
println(Table.concat(xs, ","))                       -- a,c
```

#### `Table.keys<K, V>(t: table<K, V>) -> table<K>`

Every key: the array indices `1..#t` in order, then the map's keys in no
particular order.

```saule
local t: table<string, integer> = {}
t["ada"] = 1
t["grace"] = 2

local names: table<string> = Table.keys(t)
println(#names)                      -- 2

-- Array indices come out in order.
println(Table.concat(Table.keys({"x", "y", "z"}), ","))   -- 1,2,3
```

#### `Table.values<K, V>(t: table<K, V>) -> table<V>`

Every value, in the same order as `Table.keys`.

```saule
println(Table.concat(Table.values({"x", "y"}), ","))   -- x,y

local ages: table<string, integer> = {}
ages["ada"] = 36
println(Table.values(ages)[1] ?? 0)                    -- 36

-- Summing a map's values, whatever its keys are.
local counts: table<string, integer> = {}
counts["a"] = 2
counts["b"] = 3
println(Iter.reduce(Table.values(counts), 0, (acc, n) => acc + n))   -- 5
```

#### `Table.slice<V>(t: table<V>, from: integer, to: integer?) -> table<V>`

A **new** table over the 1-based range, `to` defaulting to the end. Negative
indices count from the end, and an out-of-range slice is empty rather than an
error.

```saule
local xs: table<integer> = {1, 2, 3, 4, 5}

println(Table.concat(Table.slice(xs, 2, 3), ","))    -- 2,3
println(Table.concat(Table.slice(xs, 3), ","))       -- 3,4,5
println(Table.concat(Table.slice(xs, -2), ","))      -- 4,5
println(#Table.slice(xs, 99, 120))                   -- 0

-- The original is untouched.
println(Table.concat(xs, ","))                       -- 1,2,3,4,5
```

#### `Table.copy<V>(t: table<V>) -> table<V>`

A shallow copy of both halves. Elements are **shared**, so copying a table of
instances gives a new table pointing at the same instances.

```saule
local xs: table<integer> = {1, 2}
local ys: table<integer> = Table.copy(xs)

Table.insert(ys, 3)
println(Table.concat(xs, ","))      -- 1,2, unchanged
println(Table.concat(ys, ","))      -- 1,2,3

-- Shallow: a nested table is the same table in both copies.
local nested: table<table<integer>> = {{1}}
local clone: table<table<integer>> = Table.copy(nested)
Table.insert(clone[1] ?? {}, 2)
println(#(nested[1] ?? {}))         -- 2, the inner table is shared
```

#### `Table.concat<V>(t: table<V>, sep: string?, from: integer?, to: integer?) -> string`

Joins the elements with `sep` between them, rendering each as `tostring`
would. `sep` defaults to the empty string, and `from` / `to` restrict the
range.

[`String.join`](/saule/stdlib/string/#string-join) is the same operation with
the arguments the other way round.

```saule
println(Table.concat({1, 2, 3}, ", "))        -- 1, 2, 3
println(Table.concat({"a", "b"}))             -- ab, no separator
println(Table.concat({1, 2, 3, 4}, "-", 2, 3))   -- 2-3

-- Rendering a row.
local row: table<string> = {"ada", "36"}
println("| " .. Table.concat(row, " | ") .. " |")   -- | ada | 36 |
```

### There is no `Table.unpack`

A function returning "however many values the table holds" has no type Saule
can write — the arity is not known until the call runs — so the table is
passed as a table.

## `Iter`

Combinators over sequences. Where [`Table`](#table) mutates a table or
answers a question about one, `Iter` derives a **new** sequence and never
writes to its argument.

Every combinator is **eager**: `table` in, `table` out. That is what keeps
them typed. `Iter.map<V, U>(t: table<V>, f: fn(V) -> U)` binds `V` from the
receiver, so a lambda written without annotations still gets a real parameter
type, and the result is a real `table<U>` rather than an opaque iterator.

```saule
local nums: table<integer> = {1, 2, 3, 4, 5}

local doubled: table<integer> = Iter.map(nums, n => n * 2)
local evens: table<integer> = Iter.filter(nums, n => n % 2 == 0)
local total: integer = Iter.reduce(nums, 0, (acc, n) => acc + n)

println(Table.concat(doubled, ","))   -- 2,4,6,8,10
println(Table.concat(evens, ","))     -- 2,4
println(total)                        -- 15
```

A **step closure** or an **`Iterable`** reaches these through `Iter.collect`,
which drains it into a table first. That call is not overhead the design
added: an eager `map` has to drain the source before it can run, so `collect`
only makes visible a cost that was always there.

### Collecting

#### `Iter.collect<V>(step: fn() -> V?) -> table<V>`

Drains a step closure until it answers `nil`, collecting what it produced.
This is the bridge from anything iterable — a `String.iter`, a `File.lines`,
a class implementing `Iterable` — into the rest of this module.

A closure that never answers `nil` never terminates, exactly as a
hand-written `for` loop over it would not.

```saule
-- A step closure answers one value at a time, and `nil` when it is done.
local n: integer = 0
local countdown: fn() -> integer? = fn() -> integer?
    n = n + 1
    if n > 3 then
        return nil
    end
    return n
end

local collected: table<integer> = Iter.collect(countdown)
println(Table.concat(collected, ","))     -- 1,2,3

-- Which is what puts the combinators within reach of anything iterable: a
-- class with an `iter()` method reaches them as `Iter.collect(x.iter())`.
println(Iter.reduce(collected, 0, (acc, v) => acc + v))   -- 6
```

Note that a closure yielding **two** values — `String.iter`, or an
`Iterable2` — collects into a table of pairs, not a table of its first
value. Those are for `for k, v in ... do` rather than for this.

### Transforming

#### `Iter.map<V, U>(t: table<V>, f: fn(V) -> U) -> table<U>`

Applies `f` to every element, giving a new table of the results. The result
type follows `f`'s return type, so mapping to a different type is ordinary.

```saule
println(Table.concat(Iter.map({1, 2, 3}, n => n * 2), ","))        -- 2,4,6

-- The element type can change.
local lengths: table<integer> = Iter.map({"a", "bbb"}, s => String.len(s))
println(Table.concat(lengths, ","))                                -- 1,3

-- The original is untouched.
local xs: table<integer> = {1, 2}
local ys: table<integer> = Iter.map(xs, n => n + 10)
println(Table.concat(xs, ",") .. " / " .. Table.concat(ys, ","))   -- 1,2 / 11,12
```

#### `Iter.filter<V>(t: table<V>, pred: fn(V) -> boolean) -> table<V>`

Keeps the elements `pred` accepts, in their original order.

```saule
println(Table.concat(Iter.filter({1, 2, 3, 4}, n => n % 2 == 0), ","))   -- 2,4
println(#Iter.filter({1, 2, 3}, n => n > 9))                             -- 0

-- Chaining with `map`: filter first, so `map` does less work.
local names: table<string> = {"ada.sau", "readme.md", "main.sau"}
local saule: table<string> = Iter.filter(names, n => String.ends(n, ".sau"))
println(Table.concat(saule, ", "))                                       -- ada.sau, main.sau
```

#### `Iter.reduce<V, A>(t: table<V>, init: A, f: fn(A, V) -> A) -> A`

Folds left from `init`, threading an accumulator through every element. The
accumulator may be any type, not only the element type.

```saule
println(Iter.reduce({1, 2, 3}, 0, (acc, n) => acc + n))        -- 6
println(Iter.reduce({1, 2, 3}, 1, (acc, n) => acc * n))        -- 6

-- The accumulator can be a different type from the elements.
local joined: string = Iter.reduce({"a", "b"}, "", (acc, s) => acc .. s)
println(joined)                                                -- ab

-- An empty table gives back `init` untouched.
println(Iter.reduce({}, 42, (acc, n) => acc + 1))              -- 42
```

#### `Iter.forEach<V>(t: table<V>, f: fn(V) -> nil) -> nil`

Runs `f` for its effect on every element and answers nothing. Use it when the
point is the side effect; use `map` when the point is the result.

```saule
Iter.forEach({1, 2, 3}, n => println(n))     -- 1, then 2, then 3

-- Accumulating into something outside the loop.
local total: integer = 0
Iter.forEach({10, 20}, n => bump(n))
println(total)                               -- 30

fn bump(n: integer)
    total = total + n
end
```

### Search

#### `Iter.find<V>(t: table<V>, pred: fn(V) -> boolean) -> V?`

The first element `pred` accepts, or `nil` when none does.

```saule
println(Iter.find({1, 5, 9}, n => n > 4) ?? 0)        -- 5
println(Iter.find({1, 2}, n => n > 9) == nil)         -- true

-- Looking something up by a property of it.
local names: table<string> = {"ada", "grace", "alan"}
println(Iter.find(names, n => String.starts(n, "g")) ?? "none")   -- grace
```

#### `Iter.findIndex<V>(t: table<V>, pred: fn(V) -> boolean) -> integer?`

The 1-based position of the first element `pred` accepts, or `nil`.

Named apart from [`Table.indexOf`](/saule/stdlib/table/#table-indexof), which
searches for a **value** rather than with a predicate.

```saule
println(Iter.findIndex({1, 5, 9}, n => n > 4) ?? 0)       -- 2
println(Iter.findIndex({1, 2}, n => n > 9) == nil)        -- true

-- Finding a position in order to change it.
local xs: table<integer> = {3, 8, 2}
local at: integer? = Iter.findIndex(xs, n => n > 5)
if at != nil then
    Table.remove(xs, at!)
end
println(Table.concat(xs, ","))                            -- 3,2
```

#### `Iter.any<V>(t: table<V>, pred: fn(V) -> boolean) -> boolean`

Whether **any** element matches. `false` for an empty table.

```saule
println(Iter.any({1, 2}, n => n > 1))      -- true
println(Iter.any({1, 2}, n => n > 9))      -- false
println(Iter.any({}, n => true))           -- false

-- Guarding on a property of a whole list.
local files: table<string> = {"a.md", "b.sau"}
if Iter.any(files, f => String.ends(f, ".sau")) then
    println("has Saule sources")           -- has Saule sources
end
```

#### `Iter.all<V>(t: table<V>, pred: fn(V) -> boolean) -> boolean`

Whether **every** element matches. `true` for an empty table — there is no
element that fails.

```saule
println(Iter.all({1, 2}, n => n > 0))      -- true
println(Iter.all({1, 2}, n => n > 1))      -- false
println(Iter.all({}, n => false))          -- true, vacuously

-- Validating every entry before acting on any.
local ages: table<integer> = {36, 45}
println(Iter.all(ages, a => a >= 18))      -- true
```

#### `Iter.count<V>(t: table<V>, pred: fn(V) -> boolean) -> integer`

How many elements match.

```saule
println(Iter.count({1, 2, 3}, n => n % 2 == 1))   -- 2
println(Iter.count({}, n => true))                -- 0

-- Cheaper than building the filtered table when only the number is wanted.
local scores: table<integer> = {84, 17, 96, 42}
println(Iter.count(scores, s => s >= 50))         -- 2
```

### Slicing

#### `Iter.take<V>(t: table<V>, n: integer) -> table<V>`

The first `n` elements, or all of them when there are fewer. Never an error.

```saule
println(Table.concat(Iter.take({1, 2, 3}, 2), ","))    -- 1,2
println(Table.concat(Iter.take({1, 2}, 9), ","))       -- 1,2, all of them
println(#Iter.take({1, 2}, 0))                         -- 0

-- A top-three, after sorting.
local scores: table<integer> = {84, 17, 96, 42}
Table.sort(scores, (a, b) => a > b)
println(Table.concat(Iter.take(scores, 3), ","))       -- 96,84,42
```

#### `Iter.skip<V>(t: table<V>, n: integer) -> table<V>`

Everything after the first `n` elements. Empty when `n` is at or past the
length.

```saule
println(Table.concat(Iter.skip({1, 2, 3}, 2), ","))   -- 3
println(#Iter.skip({1, 2}, 9))                        -- 0

-- Dropping a header row.
local rows: table<string> = {"name,age", "ada,36"}
println(Table.concat(Iter.skip(rows, 1), ""))         -- ada,36
```

#### `Iter.first<V>(t: table<V>) -> V?`

The first element, or `nil` when the table is empty.

```saule
println(Iter.first({1, 2}) ?? 0)      -- 1
println(Iter.first({}) == nil)        -- true

-- Safer than `t[1]` when the table might be empty, because the nullability
-- is in the type rather than in your head.
local queue: table<string> = {}
println(Iter.first(queue) ?? "<empty>")   -- <empty>
```

#### `Iter.last<V>(t: table<V>) -> V?`

The last element, or `nil` when the table is empty.

```saule
println(Iter.last({1, 2}) ?? 0)       -- 2
println(Iter.last({}) == nil)         -- true

-- The most recent entry in a log.
local log: table<string> = {"start", "step", "done"}
println(Iter.last(log) ?? "")         -- done
```

#### `Iter.chunk<V>(t: table<V>, size: integer) -> table<table<V>>`

Fixed-size groups, in order. The last group is short when the length does not
divide evenly.

```saule
local groups: table<table<integer>> = Iter.chunk({1, 2, 3, 4, 5}, 2)

println(#groups)                                  -- 3
println(Table.concat(groups[1] ?? {}, ","))       -- 1,2
println(Table.concat(groups[3] ?? {}, ","))       -- 5, the short one

-- Paging through a list.
for page in Iter.chunk({1, 2, 3, 4}, 2) do
    println("[" .. Table.concat(page, ",") .. "]")   -- [1,2], then [3,4]
end
```

### Shaping

#### `Iter.zipWith<V, U, A>(a: table<V>, b: table<U>, f: fn(V, U) -> A) -> table<A>`

Pairs the two tables up and combines each pair with `f`, stopping at the
shorter one.

There is no plain `zip`, because a pair has no representation here: a
two-element table holding an `integer` and a `V` types as `table<any>`, which
loses both. Combining at the point the two elements meet keeps the types.

```saule
println(Table.concat(Iter.zipWith({1, 2}, {10, 20}, (a, b) => a + b), ","))   -- 11,22

-- Stops at the shorter one.
println(#Iter.zipWith({1, 2, 3}, {10}, (a, b) => a + b))                      -- 1

-- Combining two columns into labelled rows.
local names: table<string> = {"ada", "alan"}
local ages: table<integer> = {36, 41}
local rows: table<string> = Iter.zipWith(names, ages, (n, a) => n .. " (" .. a .. ")")
println(Table.concat(rows, ", "))        -- ada (36), alan (41)
```

#### `Iter.flatten<V>(t: table<table<V>>) -> table<V>`

Concatenates the inner tables into one. **One level only** — nest the call to
go deeper.

```saule
println(Table.concat(Iter.flatten({{1, 2}, {3}}), ","))   -- 1,2,3
println(#Iter.flatten({{}, {}}))                          -- 0

-- Pairs naturally with `chunk`, which is its inverse.
local chunked: table<table<integer>> = Iter.chunk({1, 2, 3, 4}, 2)
println(Table.concat(Iter.flatten(chunked), ","))         -- 1,2,3,4
```

#### `Iter.reverse<V>(t: table<V>) -> table<V>`

A **new** table in reverse order. [`Table.reverse`](/saule/stdlib/table/#table-reverse)
reverses in place instead.

```saule
local xs: table<integer> = {1, 2, 3}
println(Table.concat(Iter.reverse(xs), ","))   -- 3,2,1
println(Table.concat(xs, ","))                 -- 1,2,3, untouched
```

#### `Iter.unique<V>(t: table<V>) -> table<V>`

Drops later duplicates, keeping the first occurrence and the original order.
Compares with `==`, so a class implementing [`OpEq`](#prelude-interfaces) is
deduplicated by its own rule.

```saule
println(Table.concat(Iter.unique({1, 2, 1, 3, 2}), ","))   -- 1,2,3
println(Table.concat(Iter.unique({"a", "a"}), ","))        -- a

-- Order is the order of first appearance, not sorted order.
println(Table.concat(Iter.unique({3, 1, 3}), ","))         -- 3,1
```

#### `Iter.sortBy<V, K>(t: table<V>, key: fn(V) -> K) -> table<V>`

A **new** table sorted ascending by the extracted key, using `<` — so a class
implementing [`OpCompare`](#prelude-interfaces) sorts by its own rule. Stable:
elements with equal keys keep their original order.

For a bespoke ordering, [`Table.sort`](/saule/stdlib/table/#table-sort) takes
a comparator directly.

```saule
local words: table<string> = {"pear", "fig", "banana"}
println(Table.concat(Iter.sortBy(words, w => String.len(w)), ","))   -- fig,pear,banana

-- Sorting by the element itself needs no annotation.
println(Table.concat(Iter.sortBy({3, 1, 2}, n => n), ","))                     -- 1,2,3

-- The original is untouched, unlike `Table.sort`.
println(Table.concat(words, ","))                                             -- pear,fig,banana
```

#### `Iter.groupBy<V, K>(t: table<V>, key: fn(V) -> K) -> table<K, table<V>>`

Buckets the elements by key. The key must be a `string`, `integer` or
`boolean` — the types a table can be keyed by.

```saule
local words: table<string> = {"fig", "pear", "kiwi", "a"}
local byLength: table<integer, table<string>> = Iter.groupBy(words, w => String.len(w))

println(Table.concat(byLength[3] ?? {}, ","))    -- fig
println(Table.concat(byLength[4] ?? {}, ","))    -- pear,kiwi
println(#(byLength[9] ?? {}))                    -- 0, no bucket

-- Grouping by a boolean splits a list in two.
local nums: table<integer> = {1, 2, 3, 4}
local parity: table<boolean, table<integer>> = Iter.groupBy(nums, n => n % 2 == 0)
println(Table.concat(parity[true] ?? {}, ","))   -- 2,4
println(Table.concat(parity[false] ?? {}, ","))  -- 1,3
```

### There is no `Iter.zip` or `Iter.enumerate`

Both would have to return pairs, and a pair has no representation here —
`zipWith` combines at the point the two elements meet instead, and a
`for i, v in t do` loop already gives indices alongside values.

## `Io` / `File`

`Io` is the static entry point and holds the standard streams; a `File` is a
reference-counted handle to an open file. Both the mode a file is opened in
and the whence of a seek are real Saule enums — `IoMode` and `IoSeek` — so a
typo is a compile error rather than a silent failure at run time.

The examples below write under `Os.tmpname()` so they run anywhere without
leaving anything behind.

```saule
local path: string = Os.tmpname()

local out: File = Io.open(path, IoMode.Write)!
out.write("first\n")
out.write("second\n")
out.close()

for line in Io.lines(path) do
    println(line)              -- first, then second
end

Os.remove(path)
```

### Streams

`Io.stdin`, `Io.stdout` and `Io.stderr` are `File` values that are already
open. They need no `Io.open` and should not be closed.

```saule
Io.stdout.write("to stdout\n")
Io.stderr.write("to stderr\n")

-- `println` is `Io.stdout.write` with a newline and `tostring` applied, so
-- the two interleave exactly as written.
Io.stdout.write("a")
println("b")                   -- ab
```

### `IoMode` variants

| Variant | Mode | What it does |
| --- | --- | --- |
| `Read` | `r` | Read only; `Io.open` answers `nil` when the file does not exist |
| `Write` | `w` | Write only, **truncating** an existing file to empty |
| `Append` | `a` | Write only, every write going to the end |
| `ReadWrite` | `r+` | Read and write, file must exist, not truncated |
| `WriteRead` | `w+` | Read and write, truncating |
| `AppendRead` | `a+` | Read anywhere, writes always at the end |
| `ReadBinary` | `rb` | As `Read`, in binary mode |
| `WriteBinary` | `wb` | As `Write`, in binary mode |
| `AppendBinary` | `ab` | As `Append`, in binary mode |

The binary modes matter on Windows, where a text-mode handle translates
`\n` to `\r\n` on the way out and back on the way in. For `bytes` that is
corruption, so anything that is not text wants a binary mode.

### `IoSeek` variants

`Set` counts from the start of the file, `Cur` from the current position and
`End` from the end (where a negative offset moves backwards).

### Static functions

#### `Io.open(path: string, mode: IoMode) -> File?`

Opens `path` and answers a handle, or `nil` when it cannot — the file is
missing, the directory is not writable, the path is a directory.

Nullable rather than throwing, because a file that is not there is an
ordinary condition rather than an exceptional one.

```saule
local path: string = Os.tmpname()

-- Writing creates the file.
local out: File = Io.open(path, IoMode.Write)!
out.write("hello")
out.close()

-- Reading finds it.
local input: File = Io.open(path, IoMode.Read)!
println(input.read("a") ?? "")        -- hello
input.close()

-- A file that is not there is `nil`, not an error.
println(Io.open("/no/such/file", IoMode.Read) == nil)   -- true

Os.remove(path)
```

#### `Io.lines(path: string?) -> fn() -> string?`

A step closure over the lines of `path`, written for `for`. With no argument
it reads standard input instead.

The line separator is not included in what comes back, and the closure
answers `nil` at end of file, which is what ends the loop.

```saule
local path: string = Os.tmpname()
local out: File = Io.open(path, IoMode.Write)!
out.write("alpha\nbeta\ngamma\n")
out.close()

for line in Io.lines(path) do
    println("[" .. line .. "]")    -- [alpha], [beta], [gamma]
end

-- It is an ordinary step closure, so `Iter.collect` takes it too.
local all: table<string> = Iter.collect(Io.lines(path))
println(#all)                      -- 3

Os.remove(path)
```

#### `Io.read(...string) -> string?`

Reads from standard input in the given formats:

| Format | Reads |
| --- | --- |
| `"l"` | One line, without its newline |
| `"L"` | One line, keeping its newline |
| `"a"` | Everything remaining, as one string |
| `"n"` | One line, parsed as an `integer`; `nil` when it does not parse |

A leading `*` is accepted on any of them (`"*l"`), for familiarity with Lua.
There is **no** numeric byte-count format — for a fixed number of bytes, use
[`f.readBytes(count)`](#f-readbytes).

`nil` at end of input, which is how a read loop knows to stop.

```saule
-- Reading a line from stdin, with a fallback when there is none.
local answer: string = Io.read("l") ?? "<no input>"
println(answer)
```

#### `Io.write(...string) -> nil`

Writes its arguments to standard output with nothing between them and no
trailing newline. `Io.stdout.write(...)` is the same thing.

```saule
Io.write("no", " ", "newline")
Io.write("\n")
println("done")                -- no newline / done
```

### `File` methods

These are called with a dot, like every other method in Saule:
`handle.read("a")`, never `handle:read("a")`.

#### `f.read(...string) -> string?`

Reads from the handle in the same formats [`Io.read`](#io-read) takes —
`"l"`, `"L"`, `"a"` and `"n"`. The result is a `string`, so this is for
**text**: a file that is not valid UTF-8 needs `readBytes`, and so does a
fixed number of bytes.

```saule
local path: string = Os.tmpname()
local out: File = Io.open(path, IoMode.Write)!
out.write("line one\nline two\n")
out.close()

local f: File = Io.open(path, IoMode.Read)!
println(f.read("l") ?? "")       -- line one, newline stripped
println(f.read("L") ?? "")       -- line two, newline kept, so this prints blank after
println(f.read("l") == nil)      -- true, at end of file
f.close()

Os.remove(path)
```

#### `f.readBytes(count: integer?) -> bytes?`

Reads raw [`bytes`](#bytes), assuming no encoding. With no argument it reads
to end of file; with a `count` it reads up to that many and answers `nil`
only at end of file — so a read loop terminates.

```saule
local path: string = Os.tmpname()
local out: File = Io.open(path, IoMode.WriteBinary)!
out.writeBytes(Bytes.of(0x00, 0x7f, 0x80, 0xff))
out.close()

local f: File = Io.open(path, IoMode.ReadBinary)!
local raw: bytes = f.readBytes() ?? Bytes.empty()
println(Bytes.toHex(raw))        -- 007f80ff
println(Bytes.toString(raw) == nil)   -- true, not text
f.close()

-- Reading in fixed-size chunks until the end.
local g: File = Io.open(path, IoMode.ReadBinary)!
local total: integer = 0
while true do
    local chunk: bytes? = g.readBytes(2)
    if chunk == nil then
        break
    end
    total = total + #chunk!
end
println(total)                   -- 4
g.close()

Os.remove(path)
```

#### `f.write(...string) -> nil`

Appends text at the handle's current position. A `bytes` passed here would be
rendered as `<bytes len=N>` — use `writeBytes`.

```saule
local path: string = Os.tmpname()
local f: File = Io.open(path, IoMode.Write)!
f.write("a", "b", "c")           -- nothing between the arguments
f.write("\n")
f.close()

println(Io.open(path, IoMode.Read)!.read("a") ?? "")   -- abc
Os.remove(path)
```

#### `f.writeBytes(data: bytes) -> nil`

Appends the bytes themselves, with no encoding applied. Writing non-UTF-8 to
stdout or stderr is an error rather than mojibake.

```saule
local path: string = Os.tmpname()
local f: File = Io.open(path, IoMode.WriteBinary)!
f.writeBytes(Bytes.fromString("PNG"))
f.writeBytes(Bytes.of(0x0d, 0x0a))
f.close()

local back: File = Io.open(path, IoMode.ReadBinary)!
println(Bytes.toHex(back.readBytes() ?? Bytes.empty()))   -- 504e470d0a
back.close()

Os.remove(path)
```

#### `f.lines() -> fn() -> string?`

A step closure over the handle's remaining lines. The same as `Io.lines`, for
a file you have already opened — which is what you want when the handle was
opened in a particular mode, or when some of it has already been read.

```saule
local path: string = Os.tmpname()
local out: File = Io.open(path, IoMode.Write)!
out.write("one\ntwo\nthree\n")
out.close()

local f: File = Io.open(path, IoMode.Read)!
println(f.read("l") ?? "")       -- one, read directly

for line in f.lines() do
    println(line)                -- two, then three: it carries on
end
f.close()

Os.remove(path)
```

#### `f.seek(whence: IoSeek?, offset: integer?) -> integer?`

Moves the read/write position and answers the new position, counted in bytes
from the start. Defaults to `(IoSeek.Cur, 0)`, which moves nothing and so
answers where you are.

`nil` when the handle cannot seek — a pipe, or a closed file.

```saule
local path: string = Os.tmpname()
local out: File = Io.open(path, IoMode.Write)!
out.write("0123456789")
out.close()

local f: File = Io.open(path, IoMode.ReadBinary)!
println(f.seek() ?? -1)                       -- 0, where we are
println(Bytes.toString(f.readBytes(2) ?? Bytes.empty()) ?? "")   -- 01
println(f.seek() ?? -1)                       -- 2, two bytes in

f.seek(IoSeek.Set, 5)
println(Bytes.toString(f.readBytes(2) ?? Bytes.empty()) ?? "")   -- 56

-- From the end, with a negative offset.
f.seek(IoSeek.End, -3)
println(f.read("a") ?? "")                    -- 789

-- The size of a file, without reading it.
println(f.seek(IoSeek.End, 0) ?? -1)          -- 10
f.close()

Os.remove(path)
```

#### `f.flush() -> nil`

Forces buffered writes out to the file. Writes are buffered, so another
reader — a different handle, another process — does not see them until the
buffer is flushed or the handle is closed.

```saule
local path: string = Os.tmpname()
local f: File = Io.open(path, IoMode.Write)!
f.write("buffered")
f.flush()

-- Now a second handle can see it, without closing the first.
println(Io.open(path, IoMode.Read)!.read("a") ?? "")   -- buffered

f.close()
Os.remove(path)
```

#### `f.close() -> nil`

Releases the handle, flushing whatever is buffered. Using a handle after
closing it is an error, so close last.

```saule
local path: string = Os.tmpname()
local f: File = Io.open(path, IoMode.Write)!
f.write("done")
f.close()

-- The data is on disk, without an explicit flush.
println(Io.open(path, IoMode.Read)!.read("a") ?? "")   -- done
Os.remove(path)
```

## `Os`

Time, environment, filesystem and processes. This is the module that touches
the world outside the program, so it is also where failure is most ordinary —
see [Failure conventions](#failure-conventions) for which call reports it
which way.

### Constants

| Name | Type | Value |
| --- | --- | --- |
| `Os.sep` | `string` | The path separator: `"/"`, or `"\\"` on Windows |
| `Os.lineSep` | `string` | The line ending: `"\n"`, or `"\r\n"` on Windows |

```saule
-- Joining a path without hard-coding a separator.
local parts: table<string> = {"src", "main.sau"}
println(String.join(Os.sep, parts))      -- src/main.sau on Unix

println(#Os.lineSep >= 1)                -- true
```

### Time

#### `Os.time() -> integer`

The current Unix epoch in **seconds**. Comparable across runs and across
machines, which is what makes it the right thing to record.

```saule
local now: integer = Os.time()
println(now > 1700000000)       -- true, well past 2023

-- A timestamp to store or print.
println(Os.date("%Y-%m-%d", now) != "")   -- true
```

#### `Os.timeMillis() -> integer`

The same instant in **milliseconds** — the resolution needed to time a
request, a frame or a test.

```saule
local started: integer = Os.timeMillis()
Os.sleep(0.05)
local elapsed: integer = Os.timeMillis() - started

println(elapsed >= 40)          -- true, about 50ms
println(elapsed < 5000)         -- true
```

#### `Os.clock() -> float`

Seconds from a **monotonic** clock: it never goes backwards, even if the
system clock is adjusted, but its origin is arbitrary — only *differences*
mean anything.

This is not Lua's `os.clock`, which reports CPU time. Use `Os.time` for
*when* something happened and `Os.clock` for *how long* it took.

```saule
local started: float = Os.clock()
Os.sleep(0.05)
local took: float = Os.clock() - started

println(took >= 0.04)           -- true
println(took < 5.0)             -- true
```

#### `Os.difftime(t2: integer, t1: integer) -> integer`

`t2 - t1`. Present for familiarity with Lua; plain subtraction does the same
thing here, because both arguments are already integers.

```saule
local earlier: integer = Os.time()
Os.sleep(0.05)
local later: integer = Os.time()

println(Os.difftime(later, earlier) >= 0)      -- true
println(Os.difftime(100, 40))                  -- 60
```

#### `Os.date(format: string?, time: integer?) -> string`

Formats a Unix epoch using `strftime`-style specifiers. `format` defaults to
`"%c"` and `time` to now.

These are the specifiers that are implemented:

| Specifier | Gives | Example |
| --- | --- | --- |
| `%Y` | Four-digit year | `2021` |
| `%y` | Two-digit year | `21` |
| `%m` | Month, zero-padded | `01` |
| `%d` | Day of month, zero-padded | `01` |
| `%j` | Day of year, zero-padded | `001` |
| `%H` `%M` `%S` | Hours, minutes, seconds | `00` |
| `%c` | Date and time | `2021-01-01 00:00:00` |
| `%x` | Date only | `2021-01-01` |
| `%X` | Time only | `00:00:00` |

Anything else is **passed through literally** rather than reported — the
name-based specifiers (`%A`, `%B`, `%a`, `%b`, `%p`) are not implemented, so
`Os.date("%B")` answers the string `"%B"`.

```saule
-- A fixed instant, so the output is the same every run:
-- 2021-01-01 00:00:00 UTC.
local stamp: integer = 1609459200

println(Os.date("%Y-%m-%d", stamp))           -- 2021-01-01
println(Os.date("%Y-%m-%dT%H:%M:%S", stamp))  -- 2021-01-01T00:00:00
println(Os.date("%x", stamp))                 -- 2021-01-01
println(Os.date("%B", stamp))                 -- %B, not implemented

-- No arguments at all is "now, in the system's preferred format".
println(#Os.date() > 0)                       -- true
```

#### `Os.parsedate(text: string, format: string?) -> integer?`

The inverse of `Os.date`: reads `text` and answers the Unix epoch, or `nil`
when it does not match. `format` defaults to `"%Y-%m-%d"`.

```saule
println(Os.parsedate("2021-01-01") ?? 0)                    -- 1609459200
println(Os.parsedate("not a date") == nil)                  -- true

-- A format other than the default.
local at: integer? = Os.parsedate("01/01/2021", "%d/%m/%Y")
println(at ?? 0)                                            -- 1609459200

-- Round trip.
local stamp: integer = 1609459200
println(Os.parsedate(Os.date("%Y-%m-%d", stamp)) == stamp)  -- true
```

#### `Os.sleep(seconds: float) -> nil`

Pauses the whole program. Fractions are allowed, so sub-second waits are
ordinary.

```saule
local started: float = Os.clock()
Os.sleep(0.1)
println(Os.clock() - started >= 0.09)    -- true

-- A retry loop that backs off.
for attempt = 1, 3 do
    Os.sleep(0.01 * (attempt as float))
end
println("done")                          -- done
```

### Environment

#### `Os.getenv(name: string) -> string?`

The value of an environment variable, or `nil` when it is not set.

```saule
Os.setenv("SAULE_DOC_EXAMPLE", "yes")
println(Os.getenv("SAULE_DOC_EXAMPLE") ?? "")      -- yes

-- Unset is `nil`, which `??` turns into a default.
println(Os.getenv("SAULE_DEFINITELY_UNSET") ?? "fallback")   -- fallback

-- The shape configuration usually wants.
local level: string = Os.getenv("LOG_LEVEL") ?? "info"
println(level != "")                               -- true
```

#### `Os.setenv(name: string, value: string) -> nil`

Sets a variable **for this process** and anything it spawns afterwards. It
does not reach the shell that started the program.

```saule
Os.setenv("SAULE_GREETING", "hello")
println(Os.getenv("SAULE_GREETING") ?? "")         -- hello

-- Which a child process then inherits.
local code: integer, out: string, err: string = Os.capture("echo $SAULE_GREETING")
println(String.trim(out))                          -- hello
```

#### `Os.cwd() -> string`

The absolute path of the current working directory.

```saule
local here: string = Os.cwd()
println(String.len(here) > 0)      -- true
println(String.starts(here, Os.sep) or String.contains(here, ":"))   -- true

-- Resolving a relative path against it.
println(Os.exists(here))           -- true
```

#### `Os.chdir(path: string) -> boolean`

Changes the working directory, answering `false` when it cannot — the path
does not exist, or is not a directory.

```saule
local original: string = Os.cwd()

println(Os.chdir("/"))                  -- true
println(Os.cwd() != original)           -- true

println(Os.chdir("/no/such/dir"))       -- false, and nothing moved

Os.chdir(original)
println(Os.cwd() == original)           -- true
```

### Filesystem

#### `Os.exists(path: string) -> boolean`

Whether anything exists at `path` — a file, a directory or a symlink.

```saule
local path: string = Os.tmpname()
println(Os.exists(path))             -- false, not created yet

Io.open(path, IoMode.Write)!.close()
println(Os.exists(path))             -- true

Os.remove(path)
println(Os.exists(path))             -- false again
```

#### `Os.list(path: string) -> table<string>`

The names of a directory's children — names only, not full paths.

**It is the one call here that fails hard.** When the path cannot be read it
raises a runtime error that ends the program; `try` / `catch` does not
intercept it, because this is not a `throw` of a value. Its neighbours answer
`false` or `nil` instead, so check with `Os.exists` or `Os.fsInfo` first when
the path might not be there. That inconsistency is a known rough edge — see
[Failure conventions](#failure-conventions).

```saule
local dir: string = Os.tmpname()
Os.mkdir(dir)
Io.open(dir .. Os.sep .. "a.txt", IoMode.Write)!.close()
Io.open(dir .. Os.sep .. "b.txt", IoMode.Write)!.close()

local names: table<string> = Os.list(dir)
Table.sort(names, (a, b) => a < b)
println(Table.concat(names, ","))     -- a.txt,b.txt

-- Names, so join them back onto the directory to use them.
for name in names do
    println(Os.exists(dir .. Os.sep .. name))   -- true, twice
end

-- Guard rather than catch: an unreadable path ends the program.
if Os.exists("/no/such/dir") then
    println(#Os.list("/no/such/dir"))
else
    println("not a readable directory")   -- not a readable directory
end

Os.remove(dir .. Os.sep .. "a.txt")
Os.remove(dir .. Os.sep .. "b.txt")
Os.remove(dir)
```

#### `Os.mkdir(path: string, recursive: boolean?) -> boolean`

Creates a directory, answering `false` when it cannot. With `recursive` set,
it creates the missing parents too; without it, a missing parent is a
failure.

```saule
local base: string = Os.tmpname()

println(Os.mkdir(base))                            -- true
println(Os.mkdir(base))                            -- false, already there

-- A nested path needs `recursive`.
local nested: string = base .. Os.sep .. "a" .. Os.sep .. "b"
println(Os.mkdir(nested))                          -- false, parent missing
println(Os.mkdir(nested, true))                    -- true
println(Os.exists(nested))                         -- true

Os.remove(nested)
Os.remove(base .. Os.sep .. "a")
Os.remove(base)
```

#### `Os.remove(path: string) -> boolean`

Removes a file, or an **empty** directory. `false` when it cannot — the path
is missing, or the directory still has something in it.

```saule
local path: string = Os.tmpname()
Io.open(path, IoMode.Write)!.close()

println(Os.remove(path))              -- true
println(Os.remove(path))              -- false, already gone

-- A directory has to be empty first.
local dir: string = Os.tmpname()
Os.mkdir(dir)
local inside: string = dir .. Os.sep .. "f.txt"
Io.open(inside, IoMode.Write)!.close()

println(Os.remove(dir))               -- false, not empty
println(Os.remove(inside))            -- true
println(Os.remove(dir))               -- true, now it is
```

#### `Os.rename(from: string, to: string) -> boolean`

Moves or renames, answering `false` when it cannot. Within one filesystem
this is atomic, which is what makes write-then-rename the safe way to replace
a file.

```saule
local a: string = Os.tmpname()
local b: string = a .. ".renamed"

local f: File = Io.open(a, IoMode.Write)!
f.write("contents")
f.close()

println(Os.rename(a, b))                  -- true
println(Os.exists(a))                     -- false
println(Io.open(b, IoMode.Read)!.read("a") ?? "")   -- contents

println(Os.rename("/no/such/file", b))    -- false

Os.remove(b)
```

#### `Os.tmpname() -> string`

A path under the system temp directory that nothing is using yet. It is a
**name**, not a file: nothing is created until you create it.

```saule
local path: string = Os.tmpname()
println(Os.exists(path))          -- false, just a name

local f: File = Io.open(path, IoMode.Write)!
f.write("x")
f.close()
println(Os.exists(path))          -- true

Os.remove(path)

-- Two calls never collide.
println(Os.tmpname() != Os.tmpname())   -- true
```

#### `Os.fsInfo(path: string?) -> FsInfo?`

Metadata for one path, or `nil` when nothing is there. `path` defaults to the
current directory.

The result is an `FsInfo` with these fields:

| Field | Type | What it holds |
| --- | --- | --- |
| `path` | `string` | The path this describes |
| `kind` | `FsKind` | `File`, `Dir`, `Symlink` or `Other` |
| `size` | `integer` | Size in bytes |
| `modifiedAt` | `integer` | Last-modified time, as a Unix epoch for `Os.date` |
| `readOnly` | `boolean` | Whether the permissions forbid writing |

```saule
local path: string = Os.tmpname()
local f: File = Io.open(path, IoMode.Write)!
f.write("12345")
f.close()

local info: FsInfo = Os.fsInfo(path)!
println(info.size)                      -- 5
println(info.kind.value)                -- file
println(info.readOnly)                  -- false
println(info.modifiedAt > 0)            -- true
println(info.path == path)              -- true

-- A directory reports its own kind.
println(Os.fsInfo(Os.cwd())!.kind.value)   -- dir

-- Nothing there is `nil`, not an error.
println(Os.fsInfo("/no/such/path") == nil) -- true

Os.remove(path)
```

### `FsKind` variants

`File` (`"file"`), `Dir` (`"dir"`), `Symlink` (`"symlink"`) and `Other`
(`"other"`). Match on the variant rather than comparing `.value` when you
want the compiler to check you have covered every case.

```saule
local info: FsInfo = Os.fsInfo(Os.cwd())!

match info.kind
case FsKind.Dir then
    println("a directory")      -- a directory
case FsKind.File then
    println("a file")
case _ then
    println("something else")
end
```

### Process

#### `Os.pid() -> integer`

This process's own process id.

```saule
println(Os.pid() > 0)            -- true

-- Stable within a run.
println(Os.pid() == Os.pid())    -- true
```

#### `Os.args() -> table<string>`

The command-line arguments passed to the script, without the interpreter or
the script path.

```saule
local args: table<string> = Os.args()
println(#args >= 0)              -- true

-- The shape a CLI usually wants.
local first: string = args[1] ?? "<none>"
println(first != "")             -- true
```

#### `Os.execute(cmd: string) -> integer`

Runs `cmd` **through the shell** and answers its exit code. The child's
output goes to this process's own stdout and stderr — nothing is captured.

`-1` when the command could not be run at all.

```saule
println(Os.execute("true"))             -- 0
println(Os.execute("false") != 0)       -- true

-- The shell is real, so redirection and variables work.
println(Os.execute("test -d /") == 0)   -- true
```

#### `Os.capture(cmd: string) -> (integer, string, string)`

Runs `cmd` through the shell and captures it: the exit code, everything it
wrote to stdout, and everything it wrote to stderr. Nothing reaches the
terminal.

`-1` with two empty strings when the command could not be run.

```saule
local code: integer, out: string, err: string = Os.capture("echo hello")

println(code)                           -- 0
println(String.trim(out))               -- hello
println(err == "")                      -- true

-- stderr comes back separately from stdout.
local c2: integer, o2: string, e2: string = Os.capture("echo oops 1>&2")
println(o2 == "")                       -- true
println(String.trim(e2))                -- oops

-- A failing command reports its code rather than throwing.
local c3: integer, o3: string, e3: string = Os.capture("exit 3")
println(c3)                             -- 3
```

#### `Os.spawn(program: string, args: table<string>?, cwd: string?, env: table<string, string>?) -> Process?`

Starts `program` and answers a handle **while it runs**, so the program can
write to it, read what came back, and carry on in between. `nil` when it
cannot be started.

There is **no shell**: `args` is passed as a vector, so nothing re-parses
quoting and a path with a space in it stays one argument. `cwd` sets the
child's working directory, and `env` *adds to* the environment it inherits
rather than replacing it.

See [`Process` handles](#process-handles) for what the handle can do.

```saule
-- A child that finishes on its own.
local p: Process = Os.spawn("echo", {"hello"})!
p.wait()
println(String.trim(Bytes.toString(p.read()) ?? ""))   -- hello

-- No shell: the argument arrives exactly as written.
local literal: Process = Os.spawn("echo", {"$HOME not expanded"})!
literal.wait()
println(String.trim(Bytes.toString(literal.read()) ?? ""))  -- $HOME not expanded

-- `cwd` places the child.
local inDir: Process = Os.spawn("pwd", {}, "/")!
inDir.wait()
println(String.trim(Bytes.toString(inDir.read()) ?? ""))    -- /

-- A program that is not there is `nil`.
println(Os.spawn("saule-no-such-program") == nil)           -- true
```

#### `Os.exit(code: integer?) -> nil`

Ends the process immediately with `code`, which defaults to 0. Nothing after
it runs.

```saule
-- Guarded so this example still prints, rather than exiting the page.
local shouldStop: boolean = false
if shouldStop then
    Os.exit(1)
end
println("still running")       -- still running
```

#### `Os.platform() -> OsPlatform`

Which operating system this is, as an enum: `Linux`, `Macos`, `Windows` or
`Other`, whose `.value` is `"linux"`, `"macos"`, `"windows"` or `"other"`.

```saule
println(#Os.platform().value > 0)      -- true

-- Branching on it, checked for exhaustiveness.
match Os.platform()
case OsPlatform.Windows then
    println("backslashes")
case _ then
    println("slashes")                 -- slashes, on Unix
end

-- Which is also what `Os.sep` already answers.
println(Os.sep == "/" or Os.sep == "\\")   -- true
```

### `Process` handles

`Os.execute` and `Os.capture` both block until the child is finished, so
neither can be used to *talk to* one. `Os.spawn` is for a child with a
conversation in it — a language server, a formatter fed on stdin, an editor
embedded in a window: it returns while the child runs, and the program
writes, reads what came back, and carries on in between.

Two properties shape everything below. **Reads never block**: each captured
stream is drained on a thread of its own into a buffer, and `read` takes
whatever has arrived so far. And **reads are `bytes`, not `string`**: a
child's output is not promised to be text, and a `string` is always valid
UTF-8 — use `Bytes.toString` where you expect text, which returns `string?`
precisely because that is where it can fail.

```saule
local p: Process = Os.spawn("cat")!

p.write(Bytes.fromString("round trip\n"))
p.closeStdin()                -- `cat` reads until end of input
println(p.wait())             -- 0, its exit code

println(String.trim(Bytes.toString(p.read()) ?? ""))   -- round trip
```

#### `p.read(max: integer?) -> bytes`

Whatever has arrived on stdout, up to `max` bytes (64 KiB by default).
**Never blocks**: an empty buffer means "nothing yet", which is not the same
as "nothing more" — see [`p.atEof()`](#p-ateof).

```saule
local p: Process = Os.spawn("echo", {"abcdefgh"})!
p.wait()

println(Bytes.toString(p.read(3)) ?? "")     -- abc, capped
println(#p.read(0))                          -- 0, reads nothing
println(String.trim(Bytes.toString(p.read()) ?? ""))   -- defgh, the rest
```

#### `p.readErr(max: integer?) -> bytes`

The same, for stderr. The two streams are separate, so output written to one
never appears in the other.

```saule
local p: Process = Os.spawn("sh", {"-c", "echo out; echo err 1>&2"})!
p.wait()

println(String.trim(Bytes.toString(p.read()) ?? ""))      -- out
println(String.trim(Bytes.toString(p.readErr()) ?? ""))   -- err
```

#### `p.write(data: bytes) -> integer`

Writes to the child's stdin and answers how many bytes went in. An error once
stdin has been closed.

```saule
local p: Process = Os.spawn("cat")!

println(p.write(Bytes.fromString("hello")))   -- 5, the byte count
p.closeStdin()
p.wait()
println(Bytes.toString(p.read()) ?? "")       -- hello
```

#### `p.closeStdin() -> nil`

Drops the child's stdin so it sees end of input. Idempotent — closing twice
has expressed one intention.

Anything that reads until EOF needs this, or both ends wait forever.

```saule
local p: Process = Os.spawn("cat")!
p.write(Bytes.fromString("done\n"))

-- Without this, `cat` keeps waiting for more and `wait` never returns.
p.closeStdin()
println(p.wait())                             -- 0
println(String.trim(Bytes.toString(p.read()) ?? ""))   -- done
```

#### `p.poll() -> integer?`

The exit code if the child has finished, `nil` while it is still running.
Remembered, so asking twice is fine.

```saule
local p: Process = Os.spawn("sleep", {"30"})!
println(p.poll() == nil)        -- true, still going

p.kill()
p.wait()
println(p.poll() != nil)        -- true, and it stays answered
```

#### `p.wait() -> integer`

Blocks until the child exits **and its output has arrived**, then answers its
exit code. Closes stdin first — a child waiting for input and a parent
waiting for the child is a deadlock, and this is the end that can tell.

Both halves matter: the exit status alone does not mean the output has been
collected, so `wait` then `read` sees all of it rather than however much
happened to arrive first.

```saule
local p: Process = Os.spawn("echo", {"complete"})!
println(p.wait())                                       -- 0
println(String.trim(Bytes.toString(p.read()) ?? ""))    -- complete

-- A non-zero code comes back as itself.
local failed: Process = Os.spawn("sh", {"-c", "exit 3"})!
println(failed.wait())                                  -- 3
```

#### `p.kill() -> boolean`

Ends the child. `true` when the signal was delivered, `false` when it had
already exited — which is not an error.

```saule
local p: Process = Os.spawn("sleep", {"30"})!
println(p.running())            -- true
println(p.kill())               -- true

-- Signalled rather than exited, which `Os.execute` also reports as -1.
println(p.wait())               -- -1
println(p.running())            -- false
println(p.kill())               -- false, already gone
```

#### `p.running() -> boolean`

Whether the child is still going. Asks the operating system rather than
trusting a flag, so it is accurate without a prior `poll`.

```saule
local p: Process = Os.spawn("sleep", {"30"})!
println(p.running())      -- true
p.kill()
p.wait()
println(p.running())      -- false
```

#### `p.pid() -> integer`

The child's process id.

```saule
local p: Process = Os.spawn("true")!
println(p.pid() > 0)            -- true
println(p.pid() != Os.pid())    -- true, a different process
p.wait()
```

#### `p.available() -> integer`

How many bytes are waiting on stdout, without taking them — how far behind a
reader is.

Each stream buffers at most 8 MiB of unread output. At the limit the reader
stops taking from the pipe, so the operating system's own buffer fills and
the child blocks on its next write: nothing is ever dropped, which for a
binary protocol would be unrecoverable.

```saule
local p: Process = Os.spawn("echo", {"hello"})!
p.wait()

println(p.available())          -- 6, "hello" and a newline
p.read()
println(p.available())          -- 0, taken
```

#### `p.atEof() -> boolean`

Whether stdout has **ended and been drained**. Both halves matter: a child
can exit with output still sitting in the buffer, and reporting end of file
then would lose it.

This is what makes a non-blocking read loop terminate correctly.

```saule
local p: Process = Os.spawn("sh", {"-c", "for i in 1 2 3; do echo $i; done"})!

local text: string = ""
local deadline: float = Os.clock() + 20.0
while Os.clock() < deadline do
    text = text .. (Bytes.toString(p.read()) ?? "")
    -- Done only when it has exited *and* the buffer is drained; either
    -- alone would lose the tail.
    if p.poll() != nil and p.atEof() then
        break
    end
end

println(String.replace(String.trim(text), "\n", ","))   -- 1,2,3
println(p.atEof())                                      -- true
```

**The child outlives the handle.** Dropping the last reference does not kill
it — call `kill` or `wait`.

### Failure conventions

The filesystem calls disagree about how they report trouble, and it is worth
knowing which is which before you write the error handling:

- `Os.remove` / `Os.rename` / `Os.chdir` / `Os.mkdir` return `boolean`.
- `Os.fsInfo` and `Os.getenv` return `T?`.
- `Os.list` **throws**.

None of the first two carry the operating system's reason, so a failed call
can't tell "not found" from "permission denied". That is a known rough edge,
not a design.

---

## `Saule`

The version of the **toolchain** running your code. Distinct from
[`Project`](#project), which is the version of the code being run: upgrade
the compiler and `Saule.version` moves, while `Project.version` only moves
when you edit your own config.

Saule versions are `<two-digit year>.<build number>` — `26.7` is the seventh
release cut in 2026. There is no patch component; a fix is simply the next
build number. Build numbers restart each year, and comparisons still work
because the year leads: `27.1` is newer than `26.412`.

| Name | Type | What it holds |
| --- | --- | --- |
| `Saule.version` | `string` | The version as a version — `"26.7"`. Compare against this |
| `Saule.full` | `string` | Display only: `"26.7"`, or `"26.8-dev+1a2b3c4"` for a development build. Never parse it |
| `Saule.year` | `integer` | The year part — `26` |
| `Saule.build` | `integer` | The build number within the year, counting from 1. `0` when it could not be determined |
| `Saule.isDev` | `boolean` | `false` only when built from a clean release tag |
| `Saule.commit` | `string` | Short commit hash, or `""` when built without git available |

```saule
println(#Saule.version > 0)            -- true
println(Saule.year >= 26)              -- true
println(Saule.build >= 0)              -- true

-- `full` is for showing a human; `version` is for comparing.
println(String.starts(Saule.full, Saule.version))   -- true
```

#### `Saule.atLeast(version: string) -> boolean`

Whether this toolchain is `version` or newer. Compares dotted numeric
components, so `"26.7"` satisfies `"26"` and `"26.7"` but not `"26.8"`.

A development build reports the version it is *heading toward*, not the last
one released — so `26.8-dev` satisfies `atLeast("26.8")`. That is deliberate:
it lets you write and test code against a feature before its release exists.

```saule
println(Saule.atLeast("26"))           -- true
println(Saule.atLeast("1.0"))          -- true, long past it
println(Saule.atLeast("99.1"))         -- false

-- Using a newer facility when it is there, and degrading when it is not.
if Saule.atLeast("26.4") then
    println("running on " .. Saule.version)
else
    println("older toolchain")
end
```

`atLeast` is the runtime counterpart to `min_saule_version` in `saule.config`.
The two use the same comparison but answer different questions:
`min_saule_version` refuses to run the project at all on an old toolchain,
whereas `atLeast` lets code degrade. Reach for `min_saule_version` when your
project simply cannot work without a version, and `atLeast` when it can.

## `Project`

The `saule.config` of the code being run, as the loader read it at startup.

| Name | Type | What it holds |
| --- | --- | --- |
| `Project.name` | `string` | The project's `name:` |
| `Project.version` | `string` | Its `version:` — the version of *your* code |
| `Project.root` | `string` | Absolute path of the directory holding `saule.config` |
| `Project.srcDirs` | `table<string>` | The `src_dirs:` roots as absolute paths, resolved against `root` — not the relative spellings the config used |

```saule
-- In a project whose saule.config reads:
--   name: "bf"   version: "0.1.0"   src_dirs: ["src"]
--
-- Project.name      is "bf"
-- Project.version   is "0.1.0"
-- Project.root      is /home/you/code/bf
-- #Project.srcDirs  is 1
--
-- Run as a single file, there is no project, so every field is empty:
println(Project.name == "")          -- true
println(Project.version == "")       -- true
println(#Project.srcDirs)            -- 0
```

Three things worth knowing.

**They are snapshots, taken at startup.** There is no setter, and writing to
the `srcDirs` table changes your copy rather than anything the module loader
will consult.

**Every field is a plain `string`, never `string?`.** In **single-file mode**
— `saule run script.sau`, with no `saule.config` above it — there is no
project, and each field is the **empty string**, with `srcDirs` an empty
table. A nullable type would push a `??` onto every use of a value that is
almost always there; an empty string is falsy enough to test directly:

```saule
if Project.name == "" then
    println("running as a single file")   -- running as a single file
end
```

Project mode is what `saule run .`, `saule run <dir>` and a bare `saule run`
select; naming a file directly is single-file mode even when that file sits
inside a project.

**It needs no import.** Like every other stdlib module, `Project` is in scope
everywhere. For the config file's own keys, and which of them are required,
see [README §Project Configuration](./README.md#project-configuration).

## Conventions

A handful of patterns repeat across the stdlib:

- **1-based indexing everywhere.** `String.sub`, `String.iter`, `Table.*`,
  array literals — all 1-based. Negative indices count from the end where
  it makes sense.
- **`?` means "may not happen".** Lookups that can miss (`String.find`,
  `Io.open`, `Os.getenv`) return nullable types so the typechecker forces
  you to handle the absence with `?.`, `??`, `!`, or a `match`.
- **Multi-return is a tuple.** Functions like `String.find` and `Os.capture`
  return `(integer?, integer?)` / `(integer, string, string)`. Destructure
  with `local s, e = ...` or pattern-match.
- **Conversion is a cast, not a call.** `as` converts between `integer` and
  `float`, renders either (and `boolean`) as a `string`, and parses a
  `string` back — and on an `any` it is the checked type test instead. It
  refuses any pair with no obvious answer rather than inventing one. The one
  pair that is deliberately a *call* is `bytes` ↔ `string`: `Bytes.toString`
  can fail, and a cast has nowhere to put the failure, so it is
  [`Bytes`](#bytes)' job and its return type is `string?`.
- **Numeric flavour never mixes.** `integer` and `float` are separate types
  and nothing promotes between them silently — not the operators, and not the
  stdlib. `Math.*` is generic over the flavour (`<N>`), so a call takes either
  and answers in kind, but one call may not take both.
- **No exceptions for "expected" failures.** Filesystem helpers return
  `boolean` rather than throwing. Use `throw` / `try` / `catch` for genuinely
  exceptional cases — see [README §Error Handling](./README.md#error-handling).
  The exception is `Os.list`, which throws; see
  [Failure conventions](#failure-conventions).
- **Mutate with `Table`, transform with `Iter`.** `Table.*` writes to the
  table it is given or answers a question about it; `Iter.*` builds a new
  sequence and never writes. Where both could claim a name they are named
  apart — `Table.reverse` / `Iter.reverse`, `Table.indexOf` /
  `Iter.findIndex`.

Anything not in this reference is not in the toolchain. Libraries — JSON among
them — ship as ordinary Saule packages: a git repo with a `saule.config`,
added to `dependencies:`. See
[README §Importing from a Dependency](./README.md#importing-from-a-dependency).

