# bitwise-flags

The bitwise operators, end to end. A `Permissions` flag set that combines with
`|`, intersects with `&`, flips with `~` and prints like `rwx`, and an RGBA
colour packed into a single `integer` with nothing but shifts and masks.

Run with:

```sh
saule run
```

## What it shows

Saule spells the bitwise operators as Lua 5.3 does. `^` is already
exponentiation (Lua 5.1 style), so xor is `~` — the same trade Lua made:

| Operator | Meaning |
|---|---|
| `a & b` | and |
| `a \| b` | or |
| `a ~ b` | xor |
| `~a` | complement |
| `a << b` `a >> b` | shifts |

Prefix `~` is complement and infix `~` is xor; position tells them apart, as it
already does for `-`.

| File | What it uses |
|---|---|
| `src/permissions.sau` | `OpBAnd` `OpBOr` `OpBXor` `OpBNot` `OpEq` `OpToString` — the operators on a class |
| `src/color.sau` | `<<` `>>` `&` `\|` `~` on plain integers — packing four bytes into one word |
| `src/main.sau` | both, plus the precedence and shift rules |

## Three things worth noticing

**Every bitwise operator binds tighter than every comparison.** So the mask
test reads the way it looks — `bits & flag != 0` is `(bits & flag) != 0`, no
parentheses needed. Among themselves the order is Lua's: `|` loosest, then `~`,
then `&`, then the shifts, all of them looser than `..`, `+` and `*`.

**`>>` is a *logical* shift.** The vacated bits are filled with zeros in both
directions rather than with the sign bit, so `-8 >> 1` is a large positive
number, not `-4`. It is not "divide by a power of two" for negative values —
divide when that is what you meant. A negative shift count shifts the other
way, and shifting by 64 or more shifts every bit out and yields `0`.

**There is no `~=`.** Every other binary operator has a compound form
(`&=`, `|=`, `<<=`, `>>=` are all here), but `~=` is how Lua spells "not
equal", which Saule spells `!=`. Reading it as xor-assignment would turn a
habitual `if a ~= b then` into a silent mutation, so it is left as a syntax
error. Write `a = a ~ b`.

## Integers only

Unlike Lua 5.3, a `float` operand is rejected rather than converted when it
happens to have no fractional part:

```saule
local f: float = 6.0
local bad: integer = f & 1      -- ERROR: `&` expects `integer`
local ok: integer = int(f) & 1  -- 0
```

A bit pattern is a property an `integer` has and a `float` does not, and Saule
does not mix the two numeric kinds implicitly anywhere else.

## Files

- `src/permissions.sau` — a flag set that takes over six operators
- `src/color.sau` — RGBA packing and unpacking with shifts and masks
- `src/main.sau` — both in use, plus the precedence and shift rules
