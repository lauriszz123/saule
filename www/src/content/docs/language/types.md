---
title: "Types"
description: "Saule has 10 primitive types, inherited from Lua's type system but statically declared. Lua's single number type is split into two distinct types in…"
sidebar:
  order: 1
---

<!-- Generated from README.md by `npm run sync-docs`. Edit that file, not this one. -->

Saule has 10 primitive types, inherited from Lua's type system but statically declared. Lua's single `number` type is split into two distinct types in Saule, and Lua's single `string` type is likewise split into text and bytes:

| Type | Description |
|---|---|
| `integer` | Whole numbers, no decimal component. 64-bit signed; [overflow wraps](/saule/language/types/#integer-overflow) |
| `float` | Decimal numbers, 64-bit precision |
| `byte` | An `integer` restricted to `0..=255`. Widens to `integer` for free; narrowing needs a checked [`as byte`](/saule/language/types/#the-byte-type) |
| `string` | Immutable text, always valid UTF-8. Measured and indexed in **codepoints** |
| `bytes` | Immutable buffer of arbitrary bytes, no encoding promised. Measured and indexed in **bytes** — see [`bytes` vs `string`](/saule/language/types/#bytes-vs-string) |
| `boolean` | `true` or `false` |
| `nil` | Absence of value |
| `fn(A, B) -> R` | First-class function values, typed by [signature](/saule/language/types/#function-types) |
| `table<T>` | The only data structure, typed generically. Also written `T[]` — see [Array Suffixes](/saule/language/tables/#array-suffixes) |
| `any` | A value of unknown type. Anything may be assigned **to** an `any`; getting a value back **out** requires a checked [`as` cast](/saule/language/types/#escaping-any-with-as) |

Parameters and fields must declare their type; on a local or a return type the annotation is optional and inferred from what you wrote. A variable cannot be `nil` unless its type is marked nullable with `?`.

### `bytes` vs `string`

A `string` is text. It is always valid UTF-8, `String.len` counts codepoints,
and `String.sub` slices on codepoint boundaries. That is what application code
wants, and it is why `String.char(0x80)` gives you the two-byte UTF-8 encoding
of U+0080 rather than the single byte `0x80`.

A `bytes` is everything else: an image, a digest, a compressed blob, a frame off
a socket. It promises no encoding, `#b` counts bytes, and `Bytes.sub` slices on
byte boundaries.

Lua makes one type do both jobs — a Lua string is a counted byte array, with the
`utf8` library layered on top. That is why `#s` in Lua is a byte count and
`s:sub(1, 1)` can cut a character in half. Saule splits them so the typechecker
can keep them apart:

```saule
local text: string = "saulė"
local raw: bytes = Bytes.fromString(text)

println(String.len(text))   -- 5 codepoints
println(#raw)               -- 6 bytes

-- Neither of these compiles, which is the point of keeping them apart:
--   local oops: string = raw    -- ERROR: cannot assign `bytes` to `string`
--   println(String.len(raw))    -- ERROR: expects `string`, got `bytes`
```

Nothing converts between them implicitly, and `as` is not a way across
either — there is no `bytes as string`, for the same reason there is no
`integer as boolean`: the interesting case (a buffer that is not UTF-8) has no
answer a cast could give. The conversion is a function call, and its type says
so. `Bytes.fromString` always succeeds, since every Saule string is already
valid UTF-8; `Bytes.toString` returns `string?`, because not every buffer is
text and substituting replacement characters would hide that.

`bytes` also stays out of two places a `string` is welcome: `..` does not accept
one (concatenation builds text, so use `Bytes.concat`, and `print` shows
`<bytes len=N>`), and it cannot be a table key — key by `Bytes.toHex(b)`
instead. Equality is by contents, and a `bytes` is never equal to a `string`.

Files follow the same split: `f.read()` hands back a `string` and
`f.readBytes()` a `bytes`, with `f.write` / `f.writeBytes` to match. See
[`Bytes`](/saule/stdlib/bytes/) for the full module.

### The `byte` type

`byte` is an `integer` restricted to `0..=255` — the type of one element of a
`bytes`. It is the same value at run time; what the type adds is that nothing
out of range can ever be in one.

The two directions are deliberately not symmetric:

```saule
local b: byte = 200                  -- a literal in range: no cast needed
local n: integer = b                 -- widening is free; a byte *is* an integer

fn readLength() -> integer
    return 300
end

local computed: integer = readLength()
local narrowed: byte? = computed as byte   -- checked: nil when out of range
local oops: byte = computed          -- ERROR: needs `as byte`
```

Widening costs nothing because a `byte` already is an `integer`. Narrowing is
the direction that can fail, so it goes through `as byte`, which yields
`byte?` — `nil` for anything outside the range, rather than a wrap or a clamp
that would silently corrupt the value. A `float` truncates toward zero first
and then has to fit.

A literal needs no cast because its range is visible where it is written, so
`local b: byte = 200` is fine and `local b: byte = 300` is a compile error at
the literal. A *computed* integer always needs the cast: nothing at the type
level can know what it holds.

**Arithmetic widens to `integer`.** `200 + 100` is 300, which is not a byte, so
a result that stayed `byte` would be a lie:

```saule
local b: byte = 0x7f

local sum: integer = b + 100         -- integer, not byte
local masked: integer = b & 0x0f     -- bit patterns work; a byte has them

println(sum)                         -- 227
println(masked)                      -- 15
```

That makes the overflow visible in the type instead of hidden in a wrap, and
narrowing back is the `as byte` that checks.

Where it shows up: `Bytes.byte(b, i)` returns `byte?`, so a byte read out of a
buffer needs no re-checking, and `Bytes.of(...byte)` takes them — so
`Bytes.of(0x89, 0x50)` is checked where it is written, and `Bytes.of(300)` is
a compile error rather than a runtime one. `Bytes.toArray` / `Bytes.fromArray` convert
between a `bytes` and a `byte[]` when a loop over elements is what you want —
two functions rather than an implicit conversion, because the representations
cost very differently: a `bytes` is one byte per byte, a `byte[]` a boxed
value per byte. Reach for `bytes` to hold or move bytes, `byte[]` only to work
through them one at a time.

### Function types

A function value's type is its **signature**: the types it is called with and the type it hands back. There is no bare `function` type — a slot that holds a callable has to say which calls are legal against it.

```saule
fn advance(dt: float)
  println("tick " .. dt)
end

local onTick: fn(float) -> nil = fn(dt: float)
  advance(dt)
end

local compare: fn(integer, integer) -> boolean = (a, b) => a < b

onTick(0.5)                  -- tick 0.5
println(compare(1, 2))       -- true
```

The parts are the same ones a declaration writes: a parenthesised parameter-type list, then `->` and the return type. `-> nil` is the "returns nothing" spelling. Nullability wraps the whole type, so an optional callback needs parentheses — `(fn(string) -> nil)?`, not `fn(string) -> nil?`, which is a callback returning a nullable `nil`.

```saule
class TextField
  -- Optional callbacks: absent until the caller supplies one.
  onSubmitted: (fn(string) -> nil)?
  onChanged: (fn(string) -> nil)?
end
```

The payoff is that anonymous functions assigned into such a slot are checked against it — arity, parameter types, and return type — instead of being accepted on the grounds that they are *some* function:

```saule
class TextField
  onChanged: fn(string) -> nil = text => println(text)
end

local field: TextField = TextField()

-- The slot's type is the contract, so this is rejected:
--   field.onChanged = fn(count: integer)   -- ERROR: expected fn(string) -> nil
--     println(count)
--   end
```

It also means the parameters of a lambda written into a typed slot don't need annotations. The signature supplies them, and the body is checked with the real types:

```saule
class TextField
  onChanged: fn(string) -> nil = text => println(text)
end

local field: TextField = TextField()

-- `text` is a `string`, taken from the slot's own type.
field.onChanged = text => println(String.upper(text))
field.onChanged("saule")                        -- SAULE
```

An earlier version of the language accepted a bare `function` annotation. It carried no arity and no parameter types, so every lambda fit every slot and the mistake surfaced at the call — or, for a callback stored now and invoked later, not at all. Widen deliberately with `any` if a slot really does accept anything.

### Reserved: `userdata` and `thread`

Lua has these and Saule keeps the names reserved, but **neither is implemented yet**. The typechecker will accept `local h: userdata` as an annotation and then nothing in the language can produce a value to satisfy it — there is no literal, no constructor, and no standard-library function returning either. Don't reach for them:

- `userdata` — raw memory for native integrations. Native packages currently exchange values through `table` and the native SDK's own types instead.
- `thread` — coroutines. Saule has no concurrency primitives today; there is no `coroutine` module.

They are listed here so the omission is visible rather than surprising.

### Integer vs Float

Use `integer` for whole values like counts, indices, and health. Use `float` for precision values like position, speed, and ratios:

```saule
local health: integer = 100
local speed: float = 3.14
local index: integer = 1
local ratio: float = 0.75
```

Mixing `integer` and `float` directly is a **compile error**:

```saule
local health: integer = 100
local dmg: float = 10.5
local result = health - dmg    -- ERROR: cannot mix integer and float
```

Saule never auto-promotes — the checker catches this at compile time, so a hidden `int / int` truncating into a `float` slot is impossible.

### Float Literals

Because the two types never mix implicitly, a whole number that belongs in a
`float` has to *say* it is one. There are three ways to write it:

```saule
local ratio: float = 0.75
local half: float = .5         -- the integer part may be omitted
local speed: float = 10f       -- `f` / `F` suffix: this is 10.0
local scale: float = 1.0       -- the same thing, written out
local exact: float = 2.5f      -- allowed, though the `.5` already decided it
local budget: float = 1e9      -- exponent: this is 1000000000.0
```

The suffix earns its keep in expressions, where `10f` is considerably easier to
read than `10 as float`:

```saule
local speed: float = 3.5
println(speed * 2f)            -- 7.0
```

A trailing dot is **not** a float: `1.` lexes as `1` followed by `.`, which is
what keeps `1..2` (concatenation) and `1.foo` (member access) unambiguous.
Write `1.0` or `1f` instead.

#### Exponents

`e` (or `E`) followed by an optional sign and at least one digit multiplies the
literal by that power of ten:

```saule
local budget: float = 1e9         -- 1000000000.0
local milli: float = 1.5e3        -- 1500.0
local tiny: float = 2e-7          -- 0.0000002
local avogadro: float = 6.022E+23
local half: float = .5e2          -- 50.0, leading dot and exponent together
```

An exponent always makes a **float**, with or without a fractional part in the
mantissa. `1e9` is `1000000000.0`, not the `integer` `1000000000` — the
notation is for real numbers, and letting the *value* decide the type would
mean `1e9` and `1.0e9` were different types. Lua 5.3 draws the line in the
same place, as Saule already does for `/` and `^`.

The digits after `e` are required, and that is what keeps the near misses
readable: `1e` is the number `1` followed by the name `e`, and `1end` is still
`1` then `end`. A hex literal has no exponent form — `0x1e9` is one
hexadecimal integer, since `e` is a hex digit there.

### Base Prefixes

Integers can be written in hex or binary, with `_` allowed anywhere as a digit
separator:

```saule
local mask: integer = 0xFF        -- 255
local flags: integer = 0b1010     -- 10
local colour: integer = 0xFF_80_00
local glyph: integer = 0xE5CD     -- a font codepoint
```

Both forms produce ordinary `integer` values — there is no separate type. A
prefix with no digits (`0x`) or an invalid digit for the base (`0xGG`, `0b102`)
is a lex error. The `_` separator belongs to these two forms only: in decimal,
`1_000` is the number `1` followed by an identifier.

### String Literals

Strings are written with either `"` or `'`, exactly as in Lua. The two are the
same type and the same syntax — only the quote that opened a literal closes it,
so each style lets the other appear unescaped:

```saule
local plain: string = "hello"
local same: string = 'hello'

local quoted: string = 'he said "hi"'
local possessive: string = "it's fine"
```

These escapes are recognised, and both quote escapes work in either style, so
moving a literal between them never invalidates one:

| Escape | Meaning |
|---|---|
| `\n` | Newline |
| `\t` | Tab |
| `\r` | Carriage return |
| `\0` | NUL |
| `\\` | Backslash |
| `\"` | Double quote |
| `\'` | Single quote |
| `\xHH` | The codepoint with that two-digit hex value: `\x41` is `A` |
| `\u{...}` | The codepoint with that one-to-six-digit hex value: `\u{1F600}` is 😀 |

Anything else after a backslash is a lex error. A literal newline inside a
string is allowed and is kept as-is.

Both numeric escapes name a **codepoint**, not a byte. A `string` is always
valid UTF-8, so `"\x80"` is U+0080 — two bytes once encoded — and not a lone
`0x80`, which is not text at all. `Bytes.of(0x80)` is how you write that byte,
and the distinction is the same one [`bytes` vs `string`](/saule/language/types/#bytes-vs-string)
draws everywhere else. `\u` rejects a value that is not a character: a
surrogate half (`D800`–`DFFF`) or anything past `10FFFF` is an error rather
than a silent U+FFFD.

`saule fmt` keeps an escape wherever the character would otherwise be
invisible in source — a control character, a zero-width or bidi character —
and unescapes the ones that are perfectly readable, so `"\x41"` comes back as
`"A"`.

`saule fmt` leaves the choice alone. It reads the delimiter back out of your
source, so formatting never rewrites `'hello'` into `"hello"` — in a string
literal, a pattern, a table key or an import path.

### Integer Division

`/` on two integers is **integer division** (Lua / C semantics) — the result is the truncated quotient, never a float:

```saule
local q: integer = 7 / 2     -- 3 (truncated, not 3.5)
local r: integer = 7 % 2     -- 1
```

If you want the real-number quotient, convert one operand first:

```saule
local q: float = (7 as float) / 2.0    -- 3.5
```

Because mixing kinds is a compile error, `7 / 2.0` won't silently produce `3.5` — the checker rejects it and forces an explicit `7 as float` (or `2.0 as integer`) so the intent is visible at the call site.

Dividing by zero is a runtime error for both `/` and `%`, not a `nan` or an `inf`:

```saule
local q: integer = 7 / 0     -- ERROR: division by zero
```

### Integer Overflow

`integer` is a signed 64-bit value, spanning `-9223372036854775808` to `9223372036854775807`. Arithmetic that leaves that range **wraps around** rather than trapping — the same rule Lua 5.4 uses:

```saule
local big: integer = 9223372036854775807
println(big + 1)             -- -9223372036854775808, not an error
```

This is the one place Saule does not catch a numeric mistake for you. It is a deliberate trade — a check on every add costs more than it saves for a language used at this scale — but it means a counter or an accumulator that could plausibly reach 2⁶³ needs its own bound. Use `float` when a value's magnitude is genuinely unbounded and precision matters more than exactness.

### Exponentiation

`^` raises a number to a power. It binds tighter than every other arithmetic operator — tighter than unary minus, too — and is right-associative:

```saule
local squared: integer = 5 ^ 2      -- 25
local tower: integer = 2 ^ 3 ^ 2    -- 512, i.e. 2 ^ (3 ^ 2)
local neg: integer = -2 ^ 2         -- -4, i.e. -(2 ^ 2)
local root: float = 2.0 ^ 0.5       -- 1.4142135623730951
```

Like `/`, `^` follows the type of its operands rather than promoting: `integer ^ integer` stays an `integer`. A negative exponent has no integer answer, so it is an error — convert first if you want one:

```saule
local half: float = 2.0 ^ -1.0      -- 0.5
local bad: integer = 2 ^ -1         -- ERROR: negative exponent on integers
```

### Bitwise Operators

Six operators work on the bits of an `integer`, spelled as in Lua 5.3:

| Operator | Meaning |
|---|---|
| `a & b` | and |
| `a \| b` | or |
| `a ~ b` | xor |
| `~a` | complement (one's complement) |
| `a << b` | left shift |
| `a >> b` | right shift |

`~` carries xor because `^` is already exponentiation — the same trade Lua 5.3 made. It is unary complement in prefix position and binary xor in infix position, told apart by where it appears, exactly as `-` already is.

```saule
local flags: integer = 0b1100 | 0b0011   -- 15
local common: integer = 0b1100 & 0b1010  -- 8
local toggled: integer = 0b1100 ~ 0b1010 -- 6
local inverted: integer = ~0             -- -1
local doubled: integer = 1 << 4          -- 16
local halved: integer = 255 >> 4         -- 15
```

**Integers only.** Unlike Lua 5.3, a `float` is rejected rather than converted when it happens to have no fractional part — Saule never mixes the two numeric kinds implicitly, and this is not the place to start:

```saule
local f: float = 6.0
local bad: integer = f & 1               -- ERROR: `&` expects `integer`
local ok: integer = (f as integer) & 1   -- 0
```

**Shifts fill with zeros in both directions**, which is Lua's rule and means `>>` is a *logical* shift, not an arithmetic one — the sign bit is not replicated. A negative shift count shifts the other way, and shifting by 64 or more shifts every bit out:

```saule
local logical: integer = -1 >> 63        -- 1, not -1
local flipped: integer = 16 >> -2        -- 64 — negative count reverses
local gone: integer = 1 << 64            -- 0
```

**Precedence follows Lua**: `|` is loosest, then `~`, then `&`, then the shifts, and all of them bind looser than `..`, `+` and `*` but tighter than any comparison. So the mask-test idiom needs no parentheses:

```saule
local flags: integer = 0b0110

if flags & 0b0100 != 0 then              -- (flags & 0b0100) != 0
    println("bit set")                   -- bit set
end
```

**Compound assignment** exists for four of the five: `&=`, `|=`, `<<=`, `>>=`.

```saule
local bits: integer = 0b0001
bits |= 0b0100                           -- 5
bits <<= 2                               -- 20
```

There is deliberately no `~=`. That spelling is how Lua writes "not equal", which Saule spells `!=`; reading it as xor-assignment would turn a habitual `if a ~= b then` into a silent mutation, so `~=` is left as a syntax error instead. Write `a = a ~ b`.

Classes can overload all six through `OpBAnd`, `OpBOr`, `OpBXor`, `OpShl`, `OpShr` and `OpBNot` — see [Operator Overloading](/saule/language/interfaces/#operator-overloading).

### `nil` Is a Value, Not a Binding Type

`nil` exists only as the **value** that inhabits a nullable slot. Writing `: nil` as a binding type is rejected so the meaning of the type system stays "every variable has a real type, and `?` says whether it can be empty":

```saule
local nothing: nil = nil       -- ERROR: `nil` is not a valid binding type
local pending: string? = nil   -- ok — `string?` means "string or nil"
```

`nil` is still legal as a **value** (`return nil`, `x = nil`, `match v case nil then …`) and as the conventional `-> nil` return type meaning "this function returns nothing".

### Casting

`as` converts between them, explicitly:

```saule
local health: integer = 100
local dmg: float = 10.5

local result: integer = health - (dmg as integer)   -- dmg truncated to 10
local precise: float = (health as float) - dmg      -- health promoted to 100.0
```

The pairs `as` converts, and nothing else:

| Cast | Result | Rule |
| --- | --- | --- |
| `float as integer` | `integer` | truncates toward zero, no rounding |
| `integer as float` | `float` | always safe, no precision loss |
| `integer` / `float` / `byte` / `boolean` `as string` | `string` | the text `tostring` gives |
| `string as integer` / `as float` | `integer?` / `float?` | parses; `nil` when the text is not a number |
| `integer` / `float` `as byte` | `byte?` | range-checks `0..=255`; `nil` outside it, never a wrap or a clamp. A `float` truncates first |
| `byte as integer` / `as float` | `integer` / `float` | always safe — a `byte` already is an `integer` |

Two rules keep a cast from ever being decoration:

- **A cast to the type the value already has is an error**, not a no-op.
  `n as integer` on an `integer` does nothing, so the compiler says so.
- **A pair not in the table is an error too.** There is no `integer as
  boolean`: which of `0` and `""` counts as false is a convention, not a
  fact, so the language makes you write the one you meant.

A cast off a nullable value converts the payload and passes `nil` through,
so `maybeFloat as integer` is `integer?` — nil in, nil out.

### Escaping `any` with `as`

The same keyword does a second job, and which one it is depends on what is
on the left. On a value whose type is known it converts, as above. On an
`any` there is nothing to convert *from*, so it **tests** instead.

`any` is the one type the checker cannot see through, so it is the one type
that needs a way out. `x as T` there is a **checked** cast: it tests the
value at runtime and evaluates to `T?` — the value when it really is a `T`,
and `nil` when it isn't.

```saule
fn describe(y: any) -> string
    match type(y)
        case "integer" then return "int " .. tostring(y as integer ?? 0)
        case "string" then return "str " .. (y as string ?? "?")
        case _ then return "other"
    end
end
```

Because the result is nullable, the failure case cannot be ignored — combine
it with `??` for a fallback or `!` to turn it back into a throw:

```saule
local value: any = 42

local n: integer = value as integer ?? 0     -- default on mismatch
local m: integer = (value as integer)!       -- fails loudly on mismatch

println(n)                                   -- 42
println(m)                                   -- 42
```

This is what makes `any` **sound**: a value annotated `integer` really is an
integer at runtime, because the only path from `any` to `integer` goes
through a test.

- `as` binds tighter than every binary operator, so `y as integer ?? 0`
  reads as `(y as integer) ?? 0`.
- Class casts respect inheritance — a `Dog` satisfies `as Animal`.
- `table<T>` is checked **elementwise**, so the element type is honest. That
  is O(n); an empty table satisfies any element type.
- The two readings never blur into each other. A `3.9` that arrives inside
  an `any` is **not** an integer, and `x as integer` says `nil` rather than
  silently truncating it — the truncation is what you get when you cast a
  value the checker already knows is a `float`.
- Every cast is explicit — Saule **never** converts on its own.

```saule
local x: float = 7.9
print(x as integer)          -- 7, not 8: truncation, not rounding

local boxed: any = 7.9
print(boxed as integer ?? -1)   -- -1: a float is not an integer
```

---
