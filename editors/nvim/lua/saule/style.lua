-- The indentation a project declares in its `saule.config`, applied to the
-- buffer's options.
--
-- A project can write
--
--   indent_style: "tab"
--   indent_width: 4
--
-- and `saule fmt` and the language server both honour it — deliberately over
-- the editor's own settings, so a CLI run and a format-on-save cannot produce
-- different bytes (see `crates/saule-fmt/src/config.rs`). The editor has to
-- agree, or everything you type is at odds with everything you save: pressing
-- Enter puts the cursor two columns in while the file around it is indented
-- with four-wide tabs, `>>` shifts by the wrong amount, and the first format
-- rewrites the whole file.
--
-- So this reads the same two keys, from the same nearest-ancestor
-- `saule.config`, and resolves them by the same precedence the printer uses:
--
--   1. Saule's canonical style — 2 spaces — as the ftplugin sets it.
--   2. What the file is already indented with, which stands in for the
--      editor's settings: a project that declares nothing otherwise leaves
--      the style to whoever opens the file, and typing 2 spaces into a
--      tab-indented file means the next format rewrites every line of it.
--   3. `saule.config`, layered on top, key by key.
--
-- Anything the config doesn't mention is left alone, and a malformed value is
-- ignored rather than guessed at — the same rule as `ConfigIndent::parse`,
-- because a typo silently reverting a project to the default is exactly the
-- failure this is here to prevent.
--
-- `vim.g.saule_project_indent = false` opts out and keeps the ftplugin's
-- defaults.

local M = {}

--- Columns a hard tab is assumed to occupy when a config asks for tabs
--- without naming a width. Mirrors `DEFAULT_TAB_WIDTH` in `saule-fmt`.
local DEFAULT_TAB_WIDTH = 4

--- Saule's canonical layout, matching `FmtOptions::default()`.
M.DEFAULT_WIDTH = 2
M.DEFAULT_USE_TABS = false

--- Strip surrounding whitespace and quotes, as `unquote` does on the Rust side.
local function unquote(value)
  return (value:gsub("^%s*(.-)%s*$", "%1"):gsub('^"(.*)"$', "%1"))
end

--- Parse the indentation keys out of a `saule.config`'s text.
---
--- Pure — no `vim.*` — so it can be tested with a plain `lua` interpreter,
--- like the indent model next to it. Returns `{ width = n|nil, use_tabs =
--- bool|nil }`; a key the config didn't state, or stated unusably, is `nil`.
---@return { width: integer|nil, use_tabs: boolean|nil }
function M.parse(text)
  local out = {}
  for raw in (text .. "\n"):gmatch("(.-)\n") do
    local line = raw:match("^%s*(.-)%s*$")
    if line ~= "" and line:sub(1, 2) ~= "--" then
      local key, value = line:match("^([^:]+):(.*)$")
      if key then
        key = key:match("^%s*(.-)%s*$")
        value = unquote(value)
        if key == "indent_style" then
          local style = value:lower()
          if style == "tab" or style == "tabs" then
            out.use_tabs = true
          elseif style == "space" or style == "spaces" then
            out.use_tabs = false
          end
        elseif key == "indent_width" then
          local n = tonumber(value)
          if n and n == math.floor(n) and n >= 1 and n <= 16 then
            out.width = n
          end
        end
      end
    end
  end
  return out
end

--- Layer `config` over a base style, as `ConfigIndent::apply_to` does.
---
--- A config that switches to tabs without naming a width would otherwise
--- inherit a width that was measured in spaces, so one tab would be assumed
--- two columns wide; it gets [`DEFAULT_TAB_WIDTH`] instead.
---@return { width: integer, use_tabs: boolean }
function M.resolve(config, base)
  base = base or { width = M.DEFAULT_WIDTH, use_tabs = M.DEFAULT_USE_TABS }
  local use_tabs = config.use_tabs
  if use_tabs == nil then
    use_tabs = base.use_tabs
  end
  local fallback = base.width
  if use_tabs and not base.use_tabs then
    fallback = DEFAULT_TAB_WIDTH
  end
  return { width = config.width or fallback, use_tabs = use_tabs }
end

--- The indentation `lines` are *already* written with, or nil when they are
--- not indented at all.
---
--- This stands in for the editor's own settings in the precedence chain: a
--- project that declares nothing leaves the style to whoever opens the file,
--- and typing 2 spaces into a tab-indented file means the next format rewrites
--- every line of it. Reading what the file already does is the conservative
--- answer, and the one VS Code reaches by the same route with
--- `editor.detectIndentation`. A declared `saule.config` still overrides it.
---
--- Pure, so it can be tested without an editor.
---@return { width: integer, use_tabs: boolean }|nil
function M.detect(lines)
  local smallest
  for i = 1, math.min(#lines, 200) do
    local indent = lines[i]:match("^[ \t]+")
    -- Only lines with something on them: a blank line's whitespace is noise.
    if indent and #indent < #lines[i] then
      if indent:sub(1, 1) == "\t" then
        -- One tab anywhere settles it — a tab cannot be a fraction of a
        -- space-indented level, so this is not a close call.
        return { width = DEFAULT_TAB_WIDTH, use_tabs = true }
      end
      if smallest == nil or #indent < smallest then
        smallest = #indent
      end
    end
  end
  if smallest == nil or smallest < 1 or smallest > 16 then
    return nil
  end
  -- `saule fmt` writes exact multiples of one level, so the shallowest
  -- indentation in the file *is* one level.
  return { width = smallest, use_tabs = false }
end

--- The nearest `saule.config` at or above `path`, or nil.
---
--- `path` is a file, so the search starts in its directory — the same rule as
--- `find_project_config`.
function M.find_config(path)
  if path == nil or path == "" then
    return nil
  end
  local dir = vim.fn.fnamemodify(path, ":p:h")
  local previous
  while dir ~= previous do
    local candidate = dir .. "/saule.config"
    if vim.fn.filereadable(candidate) == 1 then
      return candidate
    end
    previous, dir = dir, vim.fn.fnamemodify(dir, ":h")
  end
  return nil
end

--- The style for the file at `path`, resolved through the whole chain:
--- canonical default, then what the file itself is written with, then what
--- the project declares. Returns nil when nothing has an opinion, so the
--- caller can leave the buffer's options exactly as they were.
---@param existing string[]|nil  the buffer's current lines, when it has any
function M.for_file(path, existing)
  local base = existing and M.detect(existing) or nil

  local config = M.find_config(path)
  local declared = {}
  if config then
    local ok, lines = pcall(vim.fn.readfile, config)
    if ok and type(lines) == "table" then
      declared = M.parse(table.concat(lines, "\n"))
    end
  end

  if base == nil and declared.width == nil and declared.use_tabs == nil then
    return nil
  end
  return M.resolve(declared, base)
end

--- Apply the project's declared indentation to `bufnr`.
---
--- Called from the ftplugin, *after* it has set the canonical defaults, so a
--- file in no project — or in one that declares nothing — keeps them.
function M.apply(bufnr)
  if vim.g.saule_project_indent == false then
    return
  end
  if not vim.api.nvim_buf_is_valid(bufnr) then
    return
  end
  local style = M.for_file(
    vim.api.nvim_buf_get_name(bufnr),
    vim.api.nvim_buf_get_lines(bufnr, 0, 200, false)
  )
  if not style then
    return
  end

  vim.bo[bufnr].expandtab = not style.use_tabs
  vim.bo[bufnr].shiftwidth = style.width
  -- With hard tabs one level is one `\t`, so 'softtabstop' must stay out of
  -- the way; 'tabstop' is then how wide that `\t` renders, which is the width
  -- the config named. With spaces the three agree.
  vim.bo[bufnr].tabstop = style.width
  vim.bo[bufnr].softtabstop = style.use_tabs and 0 or style.width
end

return M
