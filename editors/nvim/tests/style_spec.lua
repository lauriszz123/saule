-- The project-style model is pure text-in / options-out, so it is tested
-- without an editor. Run from `editors/nvim`:
--
--   lua tests/style_spec.lua
--
-- The cases mirror `ConfigIndent` in `crates/saule-fmt/src/config.rs` — the
-- parser and the precedence rules are a port of it, and the editor typing a
-- different layout from the one `saule fmt` writes is exactly what this is
-- here to prevent. Change one, change both.

package.path = "lua/?.lua;lua/?/init.lua;" .. package.path

local style = require("saule.style")

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

local function eq(got, want, label)
  checks = checks + 1
  if got ~= want then
    fail(string.format("%s: got %s, want %s", label, tostring(got), tostring(want)))
  end
end

--- Assert a resolved `{ width, use_tabs }`.
local function style_eq(got, width, use_tabs, label)
  checks = checks + 1
  if got == nil then
    fail(label .. ": got nil")
    return
  end
  if got.width ~= width or got.use_tabs ~= use_tabs then
    fail(string.format("%s: got %d/%s, want %d/%s", label,
      got.width, tostring(got.use_tabs), width, tostring(use_tabs)))
  end
end

-- ── parsing ─────────────────────────────────────────────────────────────────

test("reads both keys", function()
  local c = style.parse('indent_style: "tab"\nindent_width: 8\n')
  eq(c.use_tabs, true, "use_tabs")
  eq(c.width, 8, "width")
end)

test("a config that declares neither leaves both unset", function()
  local c = style.parse('name: "demo"\nsrc_dirs: ["src"]\n')
  eq(c.use_tabs, nil, "use_tabs")
  eq(c.width, nil, "width")
end)

test("both spellings of each style", function()
  eq(style.parse("indent_style: tab").use_tabs, true, "tab")
  eq(style.parse("indent_style: tabs").use_tabs, true, "tabs")
  eq(style.parse('indent_style: "space"').use_tabs, false, "space")
  eq(style.parse("indent_style: spaces").use_tabs, false, "spaces")
  eq(style.parse("indent_style: TAB").use_tabs, true, "TAB")
end)

test("an unusable value is ignored, never guessed at", function()
  -- Left unset so the caller falls back, rather than silently re-styling a
  -- whole project off a typo.
  eq(style.parse('indent_style: "tabss"').use_tabs, nil, "misspelt style")
  eq(style.parse("indent_width: 0").width, nil, "zero width")
  eq(style.parse("indent_width: 17").width, nil, "width past 16")
  eq(style.parse("indent_width: wide").width, nil, "non-numeric width")
  eq(style.parse("indent_width: 2.5").width, nil, "fractional width")
end)

test("comments and blank lines are skipped", function()
  local c = style.parse('-- indent_style: "tab"\n\nindent_width: 4\n')
  eq(c.use_tabs, nil, "commented-out style")
  eq(c.width, 4, "width")
end)

-- ── precedence ──────────────────────────────────────────────────────────────

test("an empty config keeps the canonical default", function()
  style_eq(style.resolve({}), 2, false, "default")
end)

test("the config wins over the base", function()
  style_eq(style.resolve({ width = 4, use_tabs = true }, { width = 2, use_tabs = false }),
    4, true, "both declared")
end)

test("a key the config omits is left to the base", function()
  style_eq(style.resolve({ width = 8 }, { width = 2, use_tabs = true }), 8, true, "width only")
  style_eq(style.resolve({ use_tabs = false }, { width = 3, use_tabs = true }), 3, false, "style only")
end)

test("switching to tabs without a width does not inherit a space width", function()
  -- 2 was measured in spaces; one tab is not 2 columns wide.
  style_eq(style.resolve({ use_tabs = true }, { width = 2, use_tabs = false }),
    4, true, "spaces -> tabs")
  -- Already tabs: the base width was measured in tabs and is the better answer.
  style_eq(style.resolve({ use_tabs = true }, { width = 8, use_tabs = true }),
    8, true, "tabs -> tabs")
end)

-- ── detection ───────────────────────────────────────────────────────────────

local function lines(text)
  local out = {}
  for line in (text .. "\n"):gmatch("(.-)\n") do
    out[#out + 1] = line
  end
  return out
end

test("detects what a file is already written with", function()
  style_eq(style.detect(lines("class A\n\tfn go()\n\t\treturn 1\n\tend\nend")),
    4, true, "tabs")
  style_eq(style.detect(lines("class A\n    fn go()\n        return 1\n    end\nend")),
    4, false, "four spaces")
  style_eq(style.detect(lines("class A\n  fn go()\n    return 1\n  end\nend")),
    2, false, "two spaces")
end)

test("a file with nothing indented has no opinion", function()
  eq(style.detect(lines("import * from x\nclass A end")), nil, "flat file")
  eq(style.detect({}), nil, "empty file")
end)

test("whitespace-only lines are not indentation", function()
  -- A blank line carrying leftover autoindent must not be read as one level.
  style_eq(style.detect(lines("class A\n  \n    fn go()\n    end\nend")),
    4, false, "blank line ignored")
end)

test("one tab settles it, whatever else is in the file", function()
  style_eq(style.detect(lines("class A\n    fn go()\n\t\treturn 1\n    end\nend")),
    4, true, "mixed, tab wins")
end)

-- ── summary ─────────────────────────────────────────────────────────────────

io.write(string.format("\n%d checks, %d failure(s)\n", checks, failures))
os.exit(failures == 0 and 0 or 1)
