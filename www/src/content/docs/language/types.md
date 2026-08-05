---
title: "Types"
description: "Saule has 9 primitive types, inherited from Lua's type system but statically declared. Lua's single number type is split into two distinct types in Saule:"
sidebar:
  order: 1
---

<!-- Generated from README.md by `npm run sync-docs`. Edit that file, not this one. -->

Saule has 9 primitive types, inherited from Lua's type system but statically declared. Lua's single `number` type is split into two distinct types in Saule:

| Type | Description |
|---|---|
| `integer` | Whole numbers, no decimal component |
| `float` | Decimal numbers, 64-bit precision |
| `string` | Immutable sequences of characters |
| `boolean` | `true` or `false` |
| `nil` | Absence of value |
| `function` | First-class function values |
| `table<T>` | The only data structure, typed generically |
| `any` | A value of unknown type. Anything may be assigned **to** an `any`; getting a value back **out** requires a checked [`as` cast](/saule/language/types/#escaping-any-with-as) |
| `userdata` | Raw memory for native integrations |
| `thread` | Coroutines |

Parameters and fields must declare their type; on a local or a return type the annotation is optional and inferred from what you wrote. A variable cannot be `nil` unless its type is marked nullable with `?`.

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
`float` has to *say* it is one. There are two ways to write it:

```saule
local ratio: float = 0.75
local half: float = .5         -- the integer part may be omitted
local speed: float = 10f       -- `f` / `F` suffix: this is 10.0
local scale: float = 1.0       -- the same thing, written out
local exact: float = 2.5f      -- allowed, though the `.5` already decided it
```

The suffix earns its keep in expressions, where `10f` is considerably easier to
read than `float(10)`:

```saule
local speed: float = 3.5
println(speed * 2f)            -- 7.0
```

A trailing dot is **not** a float: `1.` lexes as `1` followed by `.`, which is
what keeps `1..2` (concatenation) and `1.foo` (member access) unambiguous.
Write `1.0` or `1f` instead.

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

Seven escapes are recognised, and both quote escapes work in either style, so
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

Anything else after a backslash is a lex error — there is no hex or unicode
escape, so use `String.char` for codepoints. A literal newline inside a string
is allowed and is kept as-is.

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
local q: float = float(7) / 2.0    -- 3.5
```

Because mixing kinds is a compile error, `7 / 2.0` won't silently produce `3.5` — the checker rejects it and forces an explicit `float(7)` (or `int(2.0)`) so the intent is visible at the call site.

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

### `nil` Is a Value, Not a Binding Type

`nil` exists only as the **value** that inhabits a nullable slot. Writing `: nil` as a binding type is rejected so the meaning of the type system stays "every variable has a real type, and `?` says whether it can be empty":

```saule
local nothing: nil = nil       -- ERROR: `nil` is not a valid binding type
local pending: string? = nil   -- ok — `string?` means "string or nil"
```

`nil` is still legal as a **value** (`return nil`, `x = nil`, `match v case nil then …`) and as the conventional `-> nil` return type meaning "this function returns nothing".

### Casting

Use `int()` and `float()` to explicitly convert between the two:

```saule
local health: integer = 100
local dmg: float = 10.5

local result: integer = health - int(dmg)       -- dmg truncated to 10
local precise: float = float(health) - dmg      -- health promoted to 100.0
```

Casting rules:
- `int(float)` — truncates toward zero, no rounding
- `float(integer)` — always safe, no precision loss

### Escaping `any` with `as`

`any` is the one type the checker cannot see through, so it is the one type
that needs a way out. `x as T` is a **checked** cast: it tests the value at
runtime and evaluates to `T?` — the value when it really is a `T`, and `nil`
when it isn't.

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
local n: integer = value as integer ?? 0     -- default on mismatch
local m: integer = (value as integer)!       -- throw on mismatch
```

This is what makes `any` **sound**: a value annotated `integer` really is an
integer at runtime, because the only path from `any` to `integer` goes
through a test.

- `as` binds tighter than every binary operator, so `y as integer ?? 0`
  reads as `(y as integer) ?? 0`.
- Class casts respect inheritance — a `Dog` satisfies `as Animal`.
- `table<T>` is checked **elementwise**, so the element type is honest. That
  is O(n); an empty table satisfies any element type.
- `as` on a value whose type is already known is an error, not a no-op —
  use `int()` / `float()` for numeric conversion.
- Both are explicit — Saule **never** casts silently

```saule
local x: float = 7.9
print(int(x))    -- 7, not 8, truncation not rounding
```

---
