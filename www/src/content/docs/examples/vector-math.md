---
title: "Operator Overloading"
description: "A Vec2 that adds, negates, compares and prints like a built-in number, plus a Path where # counts points and concatenation joins two paths. Shows how each operator is an interface whose method supplies both the behaviour and the result type."
sidebar:
  order: 3
---

<!-- Generated from examples/vector-math by `npm run sync-docs`. Edit the example, not this file. -->

A `Vec2` that adds, negates, compares and prints like a built-in number, plus a `Path` where `#` counts points and concatenation joins two paths. Shows how each operator is an interface whose method supplies both the behaviour and the result type.

[Browse this example on GitHub](https://github.com/lauriszz123/saule/tree/main/examples/vector-math)

## Run it

```sh
git clone https://github.com/lauriszz123/saule.git
cd saule/examples/vector-math
saule run
```

## `saule.config`

```
name: "vector-math"
version: "0.1.0"
entry: "src/main.sau"
src_dirs: ["src"]
min_saule_version: "26.1"
```

## `src/vec2.sau`

```saule title="src/vec2.sau"
-- A 2D vector that behaves like a number.
--
-- Each `Op*` interface in the `implements` list turns on one operator. The
-- method it names is what actually runs, and its return type is what the
-- expression evaluates to — so `a + b` below is a `Vec2`, not an `any`.

export class Vec2 implements OpAdd<Vec2, Vec2>, OpSub<Vec2, Vec2>, OpMul<Vec2, Vec2>, OpNeg<Vec2>, OpEq<Vec2>, OpToString
	local x: float
	local y: float

	fn init(x: float, y: float)
		self.x = x
		self.y = y
	end

	fn getX() -> float
		return self.x
	end

	fn getY() -> float
		return self.y
	end

	-- `a + b`
	fn add(other: Vec2) -> Vec2
		return Vec2(self.x + other.x, self.y + other.y)
	end

	-- `a - b`
	fn sub(other: Vec2) -> Vec2
		return Vec2(self.x - other.x, self.y - other.y)
	end

	-- `a * b` — componentwise. `scale` below covers the by-a-float case,
	-- because arithmetic dispatches on the *left* operand and a `float` on
	-- the left has no idea what a `Vec2` is.
	fn mul(other: Vec2) -> Vec2
		return Vec2(self.x * other.x, self.y * other.y)
	end

	-- `-a`
	fn neg() -> Vec2
		return Vec2(-self.x, -self.y)
	end

	-- `a == b` / `a != b`. Without this, `==` compares identity, so two
	-- separately built vectors holding the same numbers would be unequal.
	fn equals(other: Vec2) -> boolean
		return self.x == other.x and self.y == other.y
	end

	-- `tostring(a)`, and what `print` / `..` use.
	fn toString() -> string
		return "(" .. self.x .. ", " .. self.y .. ")"
	end

	fn scale(k: float) -> Vec2
		return Vec2(self.x * k, self.y * k)
	end

	-- `^` is exponentiation, so this reads the way the maths does.
	fn lengthSquared() -> float
		return self.x ^ 2.0 + self.y ^ 2.0
	end
end
```

## `src/path.sau`

```saule title="src/path.sau"
-- A list of points, showing the two operators that aren't arithmetic.
--
-- `OpLen` backs `#path`, and `OpConcat` backs `a .. b` — which here joins
-- two paths into a longer one rather than producing a string. That is the
-- difference between `OpConcat` and `OpToString`: the first decides what
-- `..` *builds*, the second only decides how a value renders.

import Vec2 from "vec2"

export class Path implements OpLen, OpConcat<Path, Path>, OpToString
	local points: table<Vec2>

	fn init(points: table<Vec2>)
		self.points = points
	end

	fn getPoints() -> table<Vec2>
		return self.points
	end

	-- `#path`
	fn len() -> integer
		return #self.points
	end

	-- `a .. b`
	fn concat(other: Path) -> Path
		local joined: table<Vec2> = {}

		for p: Vec2 in self.points do
			joined[#joined + 1] = p
		end

		for p: Vec2 in other.getPoints() do
			joined[#joined + 1] = p
		end

		return Path(joined)
	end

	fn toString() -> string
		local out: string = ""

		for p: Vec2 in self.points do
			if out != "" then
				out = out .. " -> "
			end

			out = out .. tostring(p)
		end

		return out
	end

	-- Total distance walked, using `-` and `lengthSquared` on Vec2.
	fn travelSquared() -> float
		local total: float = 0.0
		local prev: Vec2? = nil

		for p: Vec2 in self.points do
			if prev != nil then
				total = total + (p - prev!).lengthSquared()
			end

			prev = p
		end

		return total
	end
end
```

## `src/main.sau`

```saule title="src/main.sau"
import Vec2 from "vec2"
import Path from "path"

class Main
	static fn main()
		local a: Vec2 = Vec2(1.0, 2.0)
		local b: Vec2 = Vec2(3.0, 4.0)

		println("a         = " .. a)
		println("b         = " .. b)
		println("a + b     = " .. a + b)
		println("b - a     = " .. b - a)
		println("a * b     = " .. a * b)
		println("-a        = " .. -a)

		-- `==` runs Vec2.equals, so an independently built vector matches.
		println("a == (1,2)? " .. tostring(a == Vec2(1.0, 2.0)))
		println("a == b?     " .. tostring(a == b))

		Main.simulate()
		Main.walk()
	end

	-- A euler step reads like the physics it models once `+` and `*` mean
	-- something for vectors.
	static fn simulate()
		println("")
		println("-- falling body --")

		local gravity: Vec2 = Vec2(0.0, -9.81)
		local position: Vec2 = Vec2(0.0, 100.0)
		local velocity: Vec2 = Vec2(12.0, 0.0)
		local dt: float = .5

		for step = 1, 4 do
			velocity = velocity + gravity.scale(dt)
			position = position + velocity.scale(dt)

			printf("t=%.1f  pos=%s\n", float(step) * dt, tostring(position))
		end
	end

	-- `#` and `..` on a class, via OpLen and OpConcat.
	static fn walk()
		println("")
		println("-- paths --")

		local first: Path = Path({Vec2(0.0, 0.0), Vec2(3.0, 4.0)})
		local second: Path = Path({Vec2(3.0, 8.0), Vec2(0.0, 8.0)})

		-- `..` builds a Path here, not a string — that is OpConcat.
		local whole: Path = first .. second

		-- Note the `tostring`. Once a class overloads `..`, putting it on
		-- the left of a `..` calls *its* `concat`, so `"x = " .. path` is a
		-- type error rather than string interpolation. `Vec2` above has no
		-- OpConcat, which is why it can be interpolated directly.
		printf("first  = %s  (#%d)\n", tostring(first), #first)
		printf("second = %s  (#%d)\n", tostring(second), #second)
		printf("joined = %s  (#%d)\n", tostring(whole), #whole)
		printf("travel^2 = %.1f\n", whole.travelSquared())
	end
end
```
