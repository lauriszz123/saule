---
title: "Bitwise Operators"
description: "A Permissions flag set that unions, intersects and flips with the bitwise operators and prints like rwx, plus an RGBA colour packed into one integer with shifts and masks. Shows the Lua 5.3 spellings (~ is xor, because ^ is already exponentiation), the precedence that makes bits & flag != 0 need no parentheses, and the Op* interfaces that put all six operators on a class."
sidebar:
  order: 4
---

<!-- Generated from examples/bitwise-flags by `npm run sync-docs`. Edit the example, not this file. -->

A `Permissions` flag set that unions, intersects and flips with the bitwise operators and prints like `rwx`, plus an RGBA colour packed into one `integer` with shifts and masks. Shows the Lua 5.3 spellings (`~` is xor, because `^` is already exponentiation), the precedence that makes `bits & flag != 0` need no parentheses, and the `Op*` interfaces that put all six operators on a class.

[Browse this example on GitLab](https://gitlab.com/lauriszz123/saule/-/tree/main/examples/bitwise-flags)

## Run it

```sh
git clone https://gitlab.com/lauriszz123/saule.git
cd saule/examples/bitwise-flags
saule run
```

## `saule.config`

```
name: "bitwise-flags"
version: "0.1.0"
entry: "src/main.sau"
src_dirs: ["src"]
min_saule_version: "26.1"
```

## `src/permissions.sau`

```saule title="src/permissions.sau"
-- A flag set that behaves like an integer, via the bitwise `Op*` interfaces.
--
-- The same six operators the built-in `integer` has can be taken over by a
-- class, one interface per operator, exactly as `+` and `..` can. What that
-- buys over passing a raw `integer` around is a type: `Permissions` cannot be
-- accidentally added to a file size, and `has` reads better than `& 4 != 0`.

export class Permissions implements OpBAnd<Permissions, Permissions>,
	OpBOr<Permissions, Permissions>, OpBXor<Permissions, Permissions>,
	OpBNot<Permissions>, OpEq<Permissions>, OpToString

	local bits: integer

	fn init(bits: integer)
		self.bits = bits
	end

	fn raw() -> integer
		return self.bits
	end

	-- `a & b` — the flags present in both.
	fn band(other: Permissions) -> Permissions
		return Permissions(self.bits & other.raw())
	end

	-- `a | b` — the flags present in either. The usual way to combine.
	fn bor(other: Permissions) -> Permissions
		return Permissions(self.bits | other.raw())
	end

	-- `a ~ b` — the flags present in exactly one. `~` carries xor because
	-- `^` is already exponentiation in Saule, the same trade Lua 5.3 made.
	fn bxor(other: Permissions) -> Permissions
		return Permissions(self.bits ~ other.raw())
	end

	-- `~a` — every flag this one does not have. Prefix `~` is complement and
	-- infix `~` is xor; position is what tells them apart, as it already does
	-- for `-`.
	fn bnot() -> Permissions
		return Permissions(~self.bits & Permissions.ALL_BITS)
	end

	-- Without `OpEq`, `==` compares identity, so two separately built sets
	-- holding the same flags would be unequal.
	fn equals(other: Permissions) -> boolean
		return self.bits == other.raw()
	end

	fn toString() -> string
		if self.bits == 0 then
			return "---"
		end

		local out: string = ""
		out ..= Permissions.letter(self.bits, Permissions.READ_BIT, "r")
		out ..= Permissions.letter(self.bits, Permissions.WRITE_BIT, "w")
		out ..= Permissions.letter(self.bits, Permissions.EXEC_BIT, "x")
		return out
	end

	-- `bits & flag != 0` is the mask test, and it needs no parentheses:
	-- every bitwise operator binds tighter than every comparison.
	static fn letter(bits: integer, flag: integer, name: string) -> string
		if bits & flag != 0 then
			return name
		end
		return "-"
	end

	-- Does this set contain every flag in `other`? Masking and comparing to
	-- the mask is the standard subset test — and note it needs no
	-- parentheses, because `&` binds tighter than `==`.
	fn has(other: Permissions) -> boolean
		return self.bits & other.raw() == other.raw()
	end

	-- Turn flags off: mask against the complement of what to remove.
	fn without(other: Permissions) -> Permissions
		return Permissions(self.bits & ~other.raw())
	end

	static local READ_BIT: integer = 1 << 2
	static local WRITE_BIT: integer = 1 << 1
	static local EXEC_BIT: integer = 1 << 0
	static local ALL_BITS: integer = (1 << 3) - 1

	static fn none() -> Permissions
		return Permissions(0)
	end

	static fn read() -> Permissions
		return Permissions(Permissions.READ_BIT)
	end

	static fn write() -> Permissions
		return Permissions(Permissions.WRITE_BIT)
	end

	static fn exec() -> Permissions
		return Permissions(Permissions.EXEC_BIT)
	end

	-- The classic octal spelling: three bits per role, owner highest.
	static fn fromOctal(mode: integer) -> Permissions
		return Permissions(mode & Permissions.ALL_BITS)
	end
end
```

## `src/color.sau`

```saule title="src/color.sau"
-- Packing four bytes into one integer, with nothing but `<<`, `>>` and `&`.
--
-- This is the case the bitwise operators exist for: a colour is four values
-- that each fit in 8 bits, and storing them as one `integer` makes it one
-- value to copy, compare and put in a table key.
--
-- Every operator here is `integer` in, `integer` out. Saule rejects a `float`
-- operand outright rather than converting an integral one the way Lua 5.3
-- does — a bit pattern is a property an `integer` has and a `float` does not.

-- Where each channel sits in the packed word.
local RED_SHIFT: integer = 24
local GREEN_SHIFT: integer = 16
local BLUE_SHIFT: integer = 8
local ALPHA_SHIFT: integer = 0

-- One byte. `(1 << 8) - 1` is the idiomatic way to write "8 bits set", and
-- it stays readable when the width changes.
local BYTE: integer = (1 << 8) - 1

export fn pack(r: integer, g: integer, b: integer, a: integer) -> integer
	-- Mask each channel *before* shifting so a caller passing 300 corrupts
	-- its own channel rather than the one above it.
	return ((r & BYTE) << RED_SHIFT)
		| ((g & BYTE) << GREEN_SHIFT)
		| ((b & BYTE) << BLUE_SHIFT)
		| ((a & BYTE) << ALPHA_SHIFT)
end

-- Shift the channel down to the bottom, then mask off everything above it.
-- The mask is what makes the order safe: without it, `red` would carry the
-- sign bit down with it for any colour with the high bit set.
export fn channel(rgba: integer, shift: integer) -> integer
	return (rgba >> shift) & BYTE
end

export fn red(rgba: integer) -> integer
	return channel(rgba, RED_SHIFT)
end

export fn green(rgba: integer) -> integer
	return channel(rgba, GREEN_SHIFT)
end

export fn blue(rgba: integer) -> integer
	return channel(rgba, BLUE_SHIFT)
end

export fn alpha(rgba: integer) -> integer
	return channel(rgba, ALPHA_SHIFT)
end

export fn toHex(rgba: integer) -> string
	return String.format(
		"#%02X%02X%02X%02X",
		red(rgba),
		green(rgba),
		blue(rgba),
		alpha(rgba)
	)
end

-- Swapping two channels is a xor trick that needs no temporary: xor is its
-- own inverse, so applying the same difference to both slots exchanges them.
export fn swapRedAndBlue(rgba: integer) -> integer
	local r: integer = red(rgba)
	local b: integer = blue(rgba)
	local diff: integer = r ~ b
	return rgba ~ ((diff << RED_SHIFT) | (diff << BLUE_SHIFT))
end
```

## `src/main.sau`

```saule title="src/main.sau"
import Permissions from "permissions"
import pack, red, green, blue, alpha, toHex, swapRedAndBlue from "color"

class Main
	static fn main()
		Main.flags()
		Main.colors()
		Main.rules()
	end

	-- The bitwise operators on a class, through `OpBAnd` and friends.
	static fn flags()
		println("-- permissions --")

		local rw: Permissions = Permissions.read() | Permissions.write()
		local rwx: Permissions = rw | Permissions.exec()

		printf("read      = %s\n", tostring(Permissions.read()))
		printf("rw        = %s\n", tostring(rw))
		printf("rwx       = %s\n", tostring(rwx))

		-- `&` keeps what both sides have, `~` on two sets keeps what exactly
		-- one has, and `~` on one set flips every flag.
		printf("rwx & rw  = %s\n", tostring(rwx & rw))
		printf("rwx ~ rw  = %s\n", tostring(rwx ~ rw))
		printf("~rw       = %s\n", tostring(~rw))

		-- `==` runs Permissions.equals, so an independently built set matches.
		printf("rw == r|w? %s\n", tostring(rw == Permissions.read() | Permissions.write()))
		printf("rwx has rw? %s\n", tostring(rwx.has(rw)))
		printf("rw has x?   %s\n", tostring(rw.has(Permissions.exec())))
		printf("rwx drop w = %s\n", tostring(rwx.without(Permissions.write())))

		-- The octal spelling everyone already knows: 6 is `rw-`, 5 is `r-x`.
		printf("octal 6   = %s\n", tostring(Permissions.fromOctal(6)))
		printf("octal 5   = %s\n", tostring(Permissions.fromOctal(5)))
	end

	-- The same operators on plain integers, packing four bytes into one word.
	static fn colors()
		println("")
		println("-- colours --")

		local orange: integer = pack(255, 140, 0, 255)

		printf("packed    = %s\n", toHex(orange))
		printf("r,g,b,a   = %d, %d, %d, %d\n", red(orange), green(orange), blue(orange), alpha(orange))
		printf("swapped   = %s\n", toHex(swapRedAndBlue(orange)))

		-- Compound assignment exists for four of the five binary operators.
		-- There is deliberately no `~=`: that is how Lua spells "not equal",
		-- which Saule spells `!=`, so reading it as xor-assignment would turn
		-- a comparison into a silent mutation.
		local faded: integer = orange
		faded &= ~255            -- clear the alpha byte
		faded |= 128             -- and set it to half
		printf("half alpha = %s\n", toHex(faded))
	end

	-- Two rules worth seeing run, because both are easy to assume wrong.
	static fn rules()
		println("")
		println("-- two rules --")

		-- `>>` is a *logical* shift, as in Lua 5.3: the vacated bits are
		-- filled with zeros rather than with the sign bit, so a negative
		-- number comes back positive. `>>` is not "divide by a power of two"
		-- for negative values — divide when that is what you meant.
		printf("-8 >> 1           = %d\n", -8 >> 1)
		printf("-8 / 2            = %d\n", -8 / 2)

		-- A negative shift count shifts the other way, and shifting by 64 or
		-- more shifts every bit out.
		printf("1 << -1           = %d\n", 1 << -1)
		printf("16 >> -2          = %d\n", 16 >> -2)
		printf("1 << 64           = %d\n", 1 << 64)

		-- Precedence, loosest first: `|`, `~`, `&`, then the shifts — Lua's
		-- order. `..`, `+` and `*` all bind tighter than a shift, and every
		-- comparison binds looser than all of them.
		printf("1 | 2 & 3         = %d  (1 | (2 & 3))\n", 1 | 2 & 3)
		printf("1 | 1 << 3        = %d  (1 | (1 << 3))\n", 1 | 1 << 3)
		printf("1 << 2 + 1        = %d  (1 << (2 + 1))\n", 1 << 2 + 1)
		printf("6 & 4 != 0        = %s  ((6 & 4) != 0)\n", tostring(6 & 4 != 0))
	end
end
```
