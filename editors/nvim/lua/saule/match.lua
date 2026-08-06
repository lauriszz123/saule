-- `%` support for Saule's word-keyword blocks.
--
-- `ftplugin/saule.vim` lists the block keywords in `b:match_words` and matchit
-- pairs them with `end` by counting openers and closers. Two of those keywords
-- are not always openers, though:
--
--   * `do` ends a `for` / `while` header — the loop keyword opened the block —
--     but heads a *trailing block* anywhere else: `Canvas() do … end`.
--   * `fn` writes a type in `f: fn(T) -> U`, and a bare signature inside an
--     `interface` body. Neither has a body, and no `end` will come for it.
--
-- Counted as openers regardless, they leave every later `end` in the file
-- paired with the wrong keyword. `b:match_skip` calls [skip] below at each
-- candidate match to hide them.
--
-- Which ones to hide is not guessed at here: `saule.indent` already has to
-- tell these keywords apart to indent them, so this asks it.

local indent = require("saule.indent")

local M = {}

-- The buffer matchit is currently walking, tokenised. It re-evaluates the skip
-- expression once per candidate match, and re-reading the buffer each time
-- would make `%` quadratic; one slot is enough, since a single `%` never
-- leaves the buffer it started in.
local cached = nil

local function state()
  local buf = vim.api.nvim_get_current_buf()
  local tick = vim.api.nvim_buf_get_changedtick(buf)
  if cached and cached.buf == buf and cached.tick == tick then
    return cached
  end

  local lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
  -- Byte offset of each line inside the joined text, 1-based, as
  -- `saule.indent` counts them.
  local starts, offset = {}, 1
  for i, line in ipairs(lines) do
    starts[i] = offset
    offset = offset + #line + 1
  end

  cached = {
    buf = buf,
    tick = tick,
    starts = starts,
    false_openers = indent.false_openers(table.concat(lines, "\n")),
  }
  return cached
end

--- True when the match under the cursor sits in a comment or a string.
---
--- matchit does this itself, but only while `b:match_skip` is unset, so
--- setting it means taking the default over as well. Saule buffers are lit by
--- `syntax/saule.vim`, which is what `synID` reads.
local function in_comment_or_string()
  local name = vim.fn.synIDattr(vim.fn.synID(vim.fn.line("."), vim.fn.col("."), 1), "name")
  name = name:lower()
  return name:find("comment", 1, true) ~= nil or name:find("string", 1, true) ~= nil
end

--- `b:match_skip`: 1 when matchit should ignore the match at the cursor.
---
--- Returns a number rather than a boolean because |searchpair()| evaluates
--- this as an expression and tests it numerically.
function M.skip()
  if in_comment_or_string() then
    return 1
  end

  local st = state()
  local offset = (st.starts[vim.fn.line(".")] or 1) + vim.fn.col(".") - 1
  return st.false_openers[offset] and 1 or 0
end

return M
