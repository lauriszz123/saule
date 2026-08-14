---
title: "Declarative UI"
description: "A miniature SwiftUI-shaped toolkit drawn to the terminal: every widget is a class, constructing one draws it, and containers take their children as a trailing block (Panel(title: \"…\") do … end). Shows an immediate-mode layout engine that builds no widget tree, and why a block beats a table of children — if and for work inside one."
sidebar:
  order: 7
---

<!-- Generated from examples/ui-blocks by `npm run sync-docs`. Edit the example, not this file. -->

A miniature SwiftUI-shaped toolkit drawn to the terminal: every widget is a class, constructing one draws it, and containers take their children as a trailing block (`Panel(title: "…") do … end`). Shows an immediate-mode layout engine that builds no widget tree, and why a block beats a table of children — `if` and `for` work inside one.

[Browse this example on GitHub](https://github.com/lauriszz123/saule/tree/main/examples/ui-blocks)

## Run it

```sh
git clone https://github.com/lauriszz123/saule.git
cd saule/examples/ui-blocks
saule run
```

## `saule.config`

```
name: "ui-blocks"
version: "0.1.0"
entry: "src/main.sau"
src_dirs: ["src"]
min_saule_version: "26.1"
```

## `src/canvas.sau`

```saule title="src/canvas.sau"
-- The surface widgets draw onto, and the root of every screen.
--
--     local screen: Canvas = Canvas() do
--         Text("hello")
--     end
--
-- `Canvas`'s initialiser takes the screen's contents as its last parameter, so
-- it can be written as a trailing block. Everything constructed inside that
-- block draws onto the canvas as it is constructed — by the time `Canvas()`
-- returns, the screen is already built.
--
-- The buffer is one flat list of lines plus the two operations that make
-- nesting work:
--
--   * `mark()`  — remember how many lines exist right now
--   * `since()` — take back everything drawn after a mark
--
-- A container marks, runs its body (during which the children append their own
-- lines), then takes back exactly what they drew. That is the entire "child
-- list" an immediate-mode UI needs — no widget tree is ever built.
export class Canvas
	-- Where widgets draw. Ambient rather than passed around, so a widget's
	-- initialiser needs no parameter for it.
	static lines: table<string> = {}

	local out: string = ""

	fn init(body: fn() -> nil)
		Canvas.lines = {}
		body()
		self.out = Table.concat(Canvas.lines, "\n")
	end

	fn render() -> string
		return self.out
	end

	-- ── The drawing surface ─────────────────────────────────────────────

	static fn add(line: string) -> nil
		Table.insert(Canvas.lines, line)
	end

	static fn mark() -> integer
		return #Canvas.lines
	end

	-- Remove and return every line added after `mark`.
	static fn since(mark: integer) -> table<string>
		local taken: table<string> = {}
		local i: integer = mark + 1

		while i <= #Canvas.lines do
			Table.insert(taken, Canvas.lines[i])
			i = i + 1
		end

		while #Canvas.lines > mark do
			Table.remove(Canvas.lines)
		end

		return taken
	end

	-- ── Measuring ───────────────────────────────────────────────────────

	-- Width of the widest line. A container is sized to its contents, and the
	-- contents are only known once its body has run.
	static fn widthOf(lines: table<string>) -> integer
		local w: integer = 0
		for _, line in lines do
			local n: integer = String.len(line)
			if n > w then
				w = n
			end
		end
		return w
	end

	-- Pad each line out to `w` so a frame's right edge lines up.
	static fn padAll(lines: table<string>, w: integer) -> table<string>
		local out: table<string> = {}
		for _, line in lines do
			Table.insert(out, line .. String.rep(" ", w - String.len(line)))
		end
		return out
	end
end
```

## `src/widgets.sau`

```saule title="src/widgets.sau"
import Canvas from "canvas"

-- The widget set. Every widget is a class, and constructing one draws it.
--
-- A widget that has children declares that as its **last** initialiser
-- parameter — a `fn() -> nil` — so callers hand the children over as a
-- trailing block:
--
--     Panel(title: "Session") do
--         Field("player", "ada")
--     end
--
-- which is exactly `Panel(title: "Session", fn() Field("player", "ada") end)`.
-- Because `body` is last, anything in front of it can be named, positional or
-- defaulted, and the block still lands on `body`.

-- ── Leaves ──────────────────────────────────────────────────────────────

export class Text
	fn init(value: string)
		Canvas.add(value)
	end
end

export class Button
	fn init(label: string)
		Canvas.add("[ " .. label .. " ]")
	end
end

-- A label on the left and a value on the right, `width` apart.
export class Field
	fn init(label: string, value: string, width: integer = 22)
		local gap: integer = width - String.len(label) - String.len(value)
		if gap < 1 then
			gap = 1
		end
		Canvas.add(label .. String.rep(" ", gap) .. value)
	end
end

export class Rule
	fn init(width: integer = 22)
		Canvas.add(String.rep("-", width))
	end
end

-- ── Containers ──────────────────────────────────────────────────────────

-- Frames its children in a box with a title.
--
-- `spacing` sits between `title` and `body` and has a default, so
-- `Panel(title: "x") do … end` skips it entirely and the block still binds to
-- `body`. A trailing block always fills the callee's last parameter.
export class Panel
	fn init(title: string, spacing: integer = 0, body: fn() -> nil)
		local start: integer = Canvas.mark()
		body()
		local kids: table<string> = Canvas.since(start)

		local inner: integer = Canvas.widthOf(kids)
		if String.len(title) > inner then
			inner = String.len(title)
		end
		kids = Canvas.padAll(kids, inner)

		local blank: string = "| " .. String.rep(" ", inner) .. " |"
		Canvas.add("+-" .. title .. String.rep("-", inner - String.len(title)) .. "-+")
		local i: integer = 0
		while i < spacing do
			Canvas.add(blank)
			i = i + 1
		end
		for _, line in kids do
			Canvas.add("| " .. line .. " |")
		end
		i = 0
		while i < spacing do
			Canvas.add(blank)
			i = i + 1
		end
		Canvas.add("+" .. String.rep("-", inner + 2) .. "+")
	end
end

-- Lays its children out side by side rather than stacked, with `spacing`
-- blanks between columns — so a `Row` block reads as "these, across".
export class Row
	fn init(spacing: integer = 2, body: fn() -> nil)
		local start: integer = Canvas.mark()
		body()
		local kids: table<string> = Canvas.since(start)
		Canvas.add(Table.concat(kids, String.rep(" ", spacing)))
	end
end
```

## `src/main.sau`

```saule title="src/main.sau"
import Canvas from "canvas"
import * from "widgets"

-- A tiny declarative UI, drawn to the terminal.
--
-- Read `Panel(title: "…") do … end` as "a panel, containing these". The block
-- after the parentheses is the panel's contents — an ordinary lambda passed as
-- the initialiser's last argument, without the `fn(` / `end)` ceremony.
class Main
  static local players: table<string> = {"ada", "grace", "linus"}
  static local scores: table<integer> = {120, 95, 87}

  static fn main()
    local screen = Canvas() do
      Panel(title: "Saule UI", spacing: 1) do
        Text("Trailing blocks, drawn.")

        -- Nesting is just a block inside a block.
        Panel(title: "Session") do
          Field("player", "ada")
          Field("region", "eu-north")
          Field("build", "26.1")
        end

        Panel(title: "Scoreboard") do
          -- The reason this is a block and not a table of children:
          -- ordinary control flow works inside it.
          for i, name in Main.players do
            local score: integer = Main.scores[i]
            if score >= 100 then
              Field(name, score .. " *")
            else
              Field(name, "" .. score)
            end
          end

          Rule()
          Field("total", "" .. Main.total())
        end

        -- `Row` lays its children across instead of down.
        Row(spacing: 3) do
          Button("Play")
          Button("Options")
          Button("Quit")
        end
      end
    end

    println(screen.render())
  end

  static local fn total() -> integer
    local sum: integer = 0
    for _, score in Main.scores do
      sum = sum + score
    end


    return sum
  end
end
```
