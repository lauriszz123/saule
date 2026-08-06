-- The indent model is pure text-in / levels-out, so it is tested without an
-- editor. Run from `editors/nvim`:
--
--   lua tests/indent_spec.lua
--
-- These are the same cases as the IntelliJ plugin's `SauleIndentModelTest` and
-- the VS Code extension's `src/indent.test.ts` — the three implementations are
-- ports of each other, so they share a test corpus. Add a case to one, add it
-- to all.

package.path = "lua/?.lua;lua/?/init.lua;" .. package.path

local indent = require("saule.indent")

local failures, checks = 0, 0

local function fail(message)
  failures = failures + 1
  io.write("  FAIL: " .. message .. "\n")
end

local function test(name, body)
  io.write("• " .. name .. "\n")
  local before = failures
  body()
  if failures == before then
    io.write("  ok\n")
  end
end

local function line_starts(text)
  local starts = { 1 }
  for i = 1, #text do
    if text:sub(i, i) == "\n" then
      starts[#starts + 1] = i + 1
    end
  end
  return starts
end

--- The model's answer for 0-based line `n` of `text`.
local function indent_of_line(text, n)
  local start = line_starts(text)[n + 1]
  local stop = start
  while stop <= #text and text:sub(stop, stop) ~= "\n" do
    stop = stop + 1
  end
  return indent.indent_for_line(text, start, stop)
end

local function assert_indent(expected_blocks, expected_conts, text, n, note)
  checks = checks + 1
  local blocks, conts = indent_of_line(text, n)
  if blocks ~= expected_blocks or conts ~= expected_conts then
    fail(
      string.format(
        "line %d: expected (%d, %d), got (%d, %d)%s",
        n,
        expected_blocks,
        expected_conts,
        blocks,
        conts,
        note and (" — " .. note) or ""
      )
    )
  end
end

local function assert_true(value, note)
  checks = checks + 1
  if not value then
    fail(note or "expected true")
  end
end

local function assert_false(value, note)
  checks = checks + 1
  if value then
    fail(note or "expected false")
  end
end

--- Strip the common leading indentation the sample is written at.
local function trim_indent(sample)
  local lines = {}
  for line in (sample .. "\n"):gmatch("([^\n]*)\n") do
    lines[#lines + 1] = line
  end
  while #lines > 0 and lines[1]:match("^%s*$") do
    table.remove(lines, 1)
  end
  while #lines > 0 and lines[#lines]:match("^%s*$") do
    table.remove(lines)
  end
  local common = math.huge
  for _, line in ipairs(lines) do
    if not line:match("^%s*$") then
      common = math.min(common, #line:match("^ *"))
    end
  end
  local out = {}
  for _, line in ipairs(lines) do
    out[#out + 1] = line:sub(common + 1)
  end
  return table.concat(out, "\n") .. "\n"
end

--- Asserts that re-deriving each line's indent reproduces the sample, i.e.
--- that a file already in canonical form is a fixed point of the model.
local function assert_round_trips(sample)
  local text = trim_indent(sample)
  local n = 0
  for line in (text):gmatch("([^\n]*)\n") do
    if not line:match("^%s*$") then
      assert_indent(#line:match("^ *") / 2, 0, text, n, string.format('"%s"', line))
    end
    n = n + 1
  end
end

local function typed_at(text)
  return indent.keyword_typed_at(text, #text + 1)
end

-- ── cases ───────────────────────────────────────────────────────────────────

--- The 1-based byte offset of the `n`th whole-word `word` in `text`.
local function nth_word_offset(text, word, n)
  local init, found = 1, 0
  while true do
    local first, last = text:find("%f[%w_]" .. word .. "%f[^%w_]", init)
    if first == nil then
      return nil
    end
    found = found + 1
    if found == n then
      return first
    end
    init = last + 1
  end
end

--- Asserts whether the `n`th `word` of `text` is one of the keywords that look
--- like a block opener without being one.
local function assert_false_opener(set, text, word, n, expected)
  checks = checks + 1
  local offset = nth_word_offset(text, word, n)
  if offset == nil then
    fail(string.format("`%s` #%d is not in the sample", word, n))
  elseif (set[offset] == true) ~= expected then
    fail(
      string.format(
        "`%s` #%d: expected false_openers[%d] to be %s",
        word,
        n,
        offset,
        tostring(expected)
      )
    )
  end
end

test("class body and methods", function()
  assert_round_trips([[
    export class Warrior extends Entity
      local health: integer

      fn init(name: string)
        self.super(name)
      end
    end
  ]])
end)

test("if elseif else", function()
  assert_round_trips([[
    fn f()
      if a then
        x()
      elseif b then
        y()
      else
        z()
      end
    end
  ]])
end)

test("loops and repeat until", function()
  assert_round_trips([[
    fn f()
      for i: integer in {1, 2, 3} do
        printf("hit %d\n", i)
      end

      while cond do
        step()
      end

      repeat
        step()
      until done
    end
  ]])
end)

test("a trailing block is a level of its own", function()
  -- `f(a) do … end` is sugar for a call whose last argument is a block-bodied
  -- lambda, so its body indents exactly like any other block.
  assert_round_trips([[
    fn f()
      local screen = Canvas() do
        Panel(title: "Saule UI", spacing: 1) do
          Text("Trailing blocks, drawn.")
        end
      end

      println(screen.render())
    end
  ]])
end)

test("a loop header's `do` opens one block, not two", function()
  -- The `do` closing a `for` / `while` header belongs to the loop, which is
  -- already open; only a `do` outside a header opens a block of its own. Get
  -- that wrong and each loop swallows an extra `end`.
  assert_round_trips([[
    fn f()
      Row(spacing: 3) do
        for i, name in players do
          while ready(name) do
            Button(name)
          end
        end
      end

      done()
    end
  ]])
end)

test("both kinds of `do` indent their body once", function()
  for _, opener in ipairs({ "while x", "for i in xs", "Canvas()", "f(a, b)" }) do
    assert_indent(1, 0, opener .. " do\n\n", 1, opener)
    assert_indent(0, 0, opener .. " do\n  step()\nend\n\n", 3, opener)
  end
end)

test("match arms stay at body level", function()
  assert_round_trips([[
    fn f()
      return match self.health
        case 0 then false
        case hp when hp < 0 then false
        case _ then true
      end
    end
  ]])
end)

test("match arm with a block body indents its statements", function()
  assert_round_trips([[
    fn f()
      match x
        case 1 then
          a()
          b()
        case 2 then
          c()
        case _ then nothing()
      end
    end
  ]])
end)

test("interface signatures have no end", function()
  assert_round_trips([[
    export interface Drawable
      fn draw(target: any)
      fn bounds() -> table
    end

    class Sprite implements Drawable
      fn draw(target: any)
        target.blit(self)
      end
    end
  ]])
end)

test("enum variants then methods", function()
  assert_round_trips([[
    enum Color
      Red,
      Green,

      fn name() -> string
        return "?"
      end
    end
  ]])
end)

test("try catch", function()
  assert_round_trips([[
    fn f()
      try
        risky()
      catch e: any
        log(e)
      end
    end
  ]])
end)

test("lambda block body", function()
  assert_round_trips([[
    local handler = fn(x: integer)
      return x + 1
    end
  ]])
end)

test("a fn type annotation is not a block", function()
  assert_round_trips([[
    fn map<T, U>(items: table<T>, f: fn(T) -> U) -> table<U>
      local out: table<U> = {}

      for item: T in items do
        out[#out + 1] = f(item)
      end

      return out
    end

    local lengths = map({"a", "bb"}, s => #s)
  ]])
end)

test("a fn type in a local annotation is not a block", function()
  assert_round_trips([[
    local double: fn(integer) -> integer = fn(x: integer) -> integer
      return x * 2
    end

    println(double(2))
  ]])
end)

test("a new line after fn-typed signatures starts at column zero", function()
  -- The reported bug: opening a line below the last statement of a file whose
  -- functions take `fn(T) -> U` callbacks produced two tabs and sixteen spaces.
  local text = "fn map<T, U>(items: table<T>, f: fn(T) -> U) -> table<U>\n"
    .. "  return items\n"
    .. "end\n"
    .. "\n"
    .. "fn filter<T>(items: table<T>, p: fn(T) -> boolean) -> table<T>\n"
    .. "  return items\n"
    .. "end\n"
    .. "\n"
    .. 'local lengths = map({"a"}, s => #s)\n'
    .. "\n"
  assert_indent(0, 0, text, 9)
end)

test("an anonymous fn argument is still a block", function()
  -- The counter-case the type-position rule must not break: `fn` after a comma
  -- opens a real body.
  assert_indent(1, 1, "map(xs, fn(x: integer) -> integer\n\n", 1)
end)

test("keywords inside strings and comments are ignored", function()
  -- Written with escapes rather than a `[[ … ]]` literal: the sample contains
  -- a Saule block comment, whose `]]` would close a Lua long string early.
  assert_round_trips(
    "fn f()\n"
      .. "  -- end\n"
      .. '  local s: string = "end end end"\n'
      .. "  --[[ class Foo ]]\n"
      .. "  return s\n"
      .. "end\n"
  )
end)

test("inside a block comment the enclosing block's indent is used", function()
  assert_indent(1, 0, "class A\n  --[[ text\n\n  ]]\nend\n", 2)
end)

test("open bracket adds a continuation level", function()
  local text = "foo(\n  a,\n  b,\n)\n"
  assert_indent(0, 0, text, 0)
  assert_indent(0, 1, text, 1)
  assert_indent(0, 1, text, 2)
  assert_indent(0, 0, text, 3)
end)

test("a blank line takes the enclosing block's indent", function()
  local text = "class A\n\n  fn f()\n\nend\n"
  assert_indent(1, 0, text, 1)
  -- Inside `fn f()`, still open at this point.
  assert_indent(2, 0, text, 3)
end)

test("a closer typed at the body indent still resolves one level out", function()
  -- What the editor sees mid-keystroke: Enter has indented the line to the body
  -- level and the closer has just been typed into it. The answer must not
  -- depend on the whitespace already there.
  local openers =
    { "fn f()", "if a then", "while a do", "for i in x do", "Canvas() do", "try", "match x" }
  for _, opener in ipairs(openers) do
    assert_indent(1, 0, "class A\n  " .. opener .. "\n    end\n", 2, opener)
  end
  assert_indent(1, 0, "class A\n  repeat\n    until done\n", 2)
  assert_indent(1, 0, "class A\n  if a then\n    else\n", 2)
  assert_indent(1, 0, "class A\n  try\n    catch e: any\n", 2)
end)

test("a closer that turns out to be an identifier keeps the body indent", function()
  -- `end` dedents as it is typed, so `endless` has to put it back.
  assert_indent(2, 0, "class A\n  fn f()\n    endless()\n", 2)
end)

-- `false_openers` has no counterpart in the other two ports: they match
-- brackets, not keywords, so only Neovim's `%` needs to know which keywords
-- open no block.
test("false_openers hides the keywords that open no block", function()
  local text = trim_indent([[
    interface Drawable
      fn draw(target: any)
    end

    fn map(items: table<T>, f: fn(T) -> U)
      local screen = Canvas() do
        for i in items do
          while ready(i) do
            f(i)
          end
        end
      end
    end
  ]])
  local false_openers = indent.false_openers(text)

  -- `fn map`, the lambda-shaped `fn` of a real body, and the trailing `do`
  -- all open a block; the rest only look like it.
  assert_false_opener(false_openers, text, "fn", 1, true) -- interface signature
  assert_false_opener(false_openers, text, "fn", 2, false) -- fn map(...)
  assert_false_opener(false_openers, text, "fn", 3, true) -- `f: fn(T) -> U`
  assert_false_opener(false_openers, text, "do", 1, false) -- Canvas() do
  assert_false_opener(false_openers, text, "do", 2, true) -- for … do
  assert_false_opener(false_openers, text, "do", 3, true) -- while … do
end)

test("keyword_typed_at fires only on a bare closer", function()
  assert_true(typed_at("fn f()\n  end"), "end")
  assert_true(typed_at("fn f()\n  else"), "else")
  assert_true(typed_at("repeat\n  until"), "until")
  assert_true(typed_at("match x\n  case"), "case")
  -- One character past a closer: the indent has to be restored.
  assert_true(typed_at("fn f()\n  endl"), "endl")
  -- Half-typed, mid-expression, or not a keyword at all.
  assert_false(typed_at("fn f()\n  en"), "en")
  assert_false(typed_at("fn f()\n  x = end"), "x = end")
  assert_false(typed_at("fn f()\n  endles"), "endles")
  assert_false(typed_at("fn f()\n  "), "blank")
end)

-- ── summary ─────────────────────────────────────────────────────────────────

io.write(string.format("\n%d checks, %d failure(s)\n", checks, failures))
os.exit(failures == 0 and 0 or 1)
