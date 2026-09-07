---
title: "Wrapper Types"
description: "A Text built straight from a string literal and a Settings with full control of every read and write. Shows Assignable<T> target-typed construction and OpIndex / OpNewIndex as Saule's __index / __newindex — with nothing injected: a wrapper declares every method it exposes, calling the String class explicitly."
sidebar:
  order: 5
---

<!-- Generated from examples/wrapper-types by `npm run sync-docs`. Edit the example, not this file. -->

A `Text` built straight from a string literal and a `Settings` with full control of every read and write. Shows `Assignable<T>` target-typed construction and `OpIndex` / `OpNewIndex` as Saule's `__index` / `__newindex` — with nothing injected: a wrapper declares every method it exposes, calling the `String` class explicitly.

[Browse this example on GitHub](https://github.com/lauriszz123/saule/tree/main/examples/wrapper-types)

## Run it

```sh
git clone https://github.com/lauriszz123/saule.git
cd saule/examples/wrapper-types
saule run
```

## `saule.config`

```
name: "wrapper-types"
version: "0.1.0"
entry: "src/main.sau"
src_dirs: ["src"]
min_saule_version: "26.1"
```

## `src/text.sau`

```saule title="src/text.sau"
-- A `Text` built straight from a `string` literal, through `Assignable<string>`.
--
-- Note what this class does **not** get for free. `string` is a type and has
-- no members; `String` is a separate class holding static functions, and
-- `String.upper(s)` is a call on it. So every method `Text` exposes it
-- declares itself and implements by calling `String` explicitly. Nothing is
-- injected, and there is no mapping from the `string` type to the `String`
-- class anywhere in the language.
--
-- That is the whole point of a wrapper: you choose the surface. `Text` has
-- `words()`, which `String` does not, and deliberately does not expose
-- `String.rep` or `String.byte`.

export class Text implements Assignable<string>, OpToString
	local raw: string

	fn init(raw: string)
		self.raw = raw
	end

	-- `local t: Text = "hello"` runs this. Static, unlike the operator
	-- contracts: there is no instance yet to call it on.
	--
	-- `from` is a keyword elsewhere (`import … from …`) but reads as an
	-- ordinary name after `fn`, where an import cannot start.
	static fn of(s: string) -> Text
		return Text(s)
	end

	-- Printing. Without this a wrapper prints as `<instance of Text>`.
	fn toString() -> string
		return self.raw
	end

	fn value() -> string
		return self.raw
	end

	-- The surface this type chooses to expose, each one an explicit call.
	fn length() -> integer
		return String.len(self.raw)
	end

	fn upper() -> string
		return String.upper(self.raw)
	end

	fn slice(start: integer, stop: integer) -> string
		return String.sub(self.raw, start, stop)
	end

	fn startsWith(prefix: string) -> boolean
		return String.starts(self.raw, prefix)
	end

	-- …plus members that are about *this* type rather than about strings.
	fn words() -> integer
		local count: integer = 0
		local inWord: boolean = false
		for i = 1, String.len(self.raw) do
			local ch: string = String.sub(self.raw, i, i)
			if ch == " " then
				inWord = false
			elseif not inWord then
				inWord = true
				count = count + 1
			end
		end
		return count
	end
end
```

## `src/settings.sau`

```saule title="src/settings.sau"
-- Full control of reading and writing, through `OpIndex` / `OpNewIndex`.
--
-- These are Saule's `__index` / `__newindex`, with one deliberate difference
-- from Lua's: they are not miss handlers over a stored key space. A class
-- instance has no keys of its own, so the method *is* the lookup and runs on
-- every `s[key]` and every `s[key] = value`.
--
-- That is what makes the normalising below reliable — there is no "already
-- present" path that would skip it.
--
-- Note also what is *not* routed here: `s.name`. Field and method names are
-- resolved statically, and sending their misses to a hook would give up
-- "unknown member" diagnostics for the whole class. Dynamic access is
-- `s[key]`; a fixed surface is declared as ordinary methods.

export class Settings implements OpIndex<string, string>, OpNewIndex<string, string>
	local data: table<string, string>
	local defaults: table<string, string>
	local writes: integer

	fn init(defaults: table<string, string>)
		self.data = {}
		self.defaults = defaults
		self.writes = 0
	end

	-- Every read lands here, so a default costs no stored entry and no
	-- caller ever has to remember the fallback.
	fn index(key: string) -> string
		return self.data[key] ?? self.defaults[key] ?? ""
	end

	-- Every write lands here, so normalising and auditing happen in one
	-- place rather than at each call site.
	fn newIndex(key: string, value: string) -> nil
		self.writes = self.writes + 1
		self.data[key] = String.lower(value)
	end

	fn writeCount() -> integer
		return self.writes
	end

	-- Was this key ever explicitly set, as opposed to answered by a default?
	fn isExplicit(key: string) -> boolean
		return self.data[key] != nil
	end
end
```

## `src/main.sau`

```saule title="src/main.sau"
import Text from "text"
import Settings from "settings"

-- A parameter is one of the sites `Assignable` applies at, so callers may pass a
-- bare `string` and get a `Text` bound here.
fn describe(t: Text) -> string
  return t.words() .. " word(s), " .. t.length() .. " chars"
end

class Main
  static fn main()
    Main.building()
    Main.dynamic()
    Main.boundaries()
  end

  -- `Assignable<T>`: a value assigned straight into a typed slot builds the
  -- object. Everything `Text` exposes, it declares itself.
  static fn building()
    println("-- building from a value --")

    local greeting: Text = "hello wrapped world"

    printf("printed   = %s\n", tostring(greeting))
    printf("length    = %d\n", greeting.length())
    printf("upper     = %s\n", greeting.upper())
    printf("slice     = %s\n", greeting.slice(1, 5))
    printf("starts    = %s\n", tostring(greeting.startsWith("hello")))

    -- A member that is about `Text`, not about strings.
    printf("words     = %d\n", greeting.words())

    -- Conversion happens at a parameter too.
    printf("describe  = %s\n", describe("two words"))
  end

  -- `OpIndex` / `OpNewIndex`: full control of fetching and setting.
  static fn dynamic()
    println("")
    println("-- dynamic get / set --")

    local s: Settings = Settings({theme: "dark", editor: "vim"})

    s["theme"] = "SOLARIZED"
    s["font"] = "Mono"

    -- Reads run `index`, so the defaults answer without being stored…
    printf("theme     = %s\n", s["theme"])
    printf("editor    = %s\n", s["editor"])
    printf("font      = %s\n", s["font"])
    printf("unknown   = '%s'\n", s["nothing"])

    -- …and writes run `newIndex`, so normalising happened in one place.
    printf("writes    = %d\n", s.writeCount())
    printf(
      "explicit? = %s / %s\n",
      tostring(s.isExplicit("theme")),
      tostring(s.isExplicit("editor"))
    )

    -- Compound assignment reads and then writes, so it runs both hooks.
    s["font"] ..= "space"
    printf("font      = %s\n", s["font"])
  end

  -- The edges are as much a part of the design as the happy path.
  static fn boundaries()
    println("")
    println("-- boundaries --")

    -- Nothing is injected: `Text` exposes only what it declares, so a
    -- name `String` happens to have is still a compile error unless
    -- `Text` declared it:
    --
    --   greeting.rep(3)     -- no member `rep` on `Text`
    println("a wrapper exposes only what it declares")

    -- `Assignable` applies only where the interpreter can see a declared
    -- type — an annotated binding or a parameter:
    --
    --   local all: table<Text> = {"a"}   -- table element
    --   local t: Text = "a"; t = "b"     -- later assignment
    println("conversion is limited to sites that really convert")
  end
end
```
