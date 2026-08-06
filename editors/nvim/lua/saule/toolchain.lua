-- Locates the Saule toolchain binaries (`saule`, `saule-lsp`) and the
-- workspace root to run them from.
--
-- A port of the IntelliJ plugin's `SauleToolchain` (see
-- `editors/intellij/.../SauleToolchain.kt`) and the VS Code extension's
-- `src/toolchain.ts`, with the same resolution order so every editor picks the
-- same binary in the same project:
--
--   1. Environment variable override (`SAULE_LSP_PATH` / `SAULE_PATH`).
--   2. An explicit path passed by the caller (`opts.cmd`), or `opts.dir`.
--   3. Cargo build output, walking UP from the start directory looking for
--      `target/{release,debug}/<name>`. Walking up is what lets you open a
--      sub-folder (say `examples/todo-app`) and still find the workspace-root
--      build output — and the directory holding that `target/` becomes the
--      working directory the binary is launched in.
--   4. `$PATH`.
--   5. The bare name, with `found = false`, so callers can surface a hint.

local M = {}

--- How far up the tree to look for a `target/` directory.
local MAX_DEPTH = 24

--- Windows needs the `.exe` suffix; Unix does not.
function M.exe_name(base)
  return (vim.fn.has("win32") == 1) and (base .. ".exe") or base
end

local function is_file(path)
  return vim.fn.filereadable(path) == 1
end

--- Walk up from `start`, returning the `target/{release,debug}/exe` and the
--- directory that contains that `target/` folder (the workspace root).
local function find_upward(start, exe)
  local dir = start
  for _ = 1, MAX_DEPTH do
    for _, profile in ipairs({ "release", "debug" }) do
      local candidate = dir .. "/target/" .. profile .. "/" .. exe
      if is_file(candidate) then
        return candidate, dir
      end
    end
    local parent = vim.fs.dirname(dir)
    if parent == nil or parent == dir then
      break
    end
    dir = parent
  end
  return nil
end

local function find_on_path(exe)
  local found = vim.fn.exepath(exe)
  if found ~= nil and found ~= "" then
    return found
  end
  return nil
end

--- Locate a toolchain binary.
---
---@param base string        bare binary name, e.g. `"saule"` or `"saule-lsp"`.
---@param opts? table        `env` (variable name), `cmd` (explicit path),
---                          `dir` (toolchain directory), `start` (where to
---                          begin walking up; defaults to the cwd).
---@return table  `{ command = string, working_dir = string|nil, found = boolean }`
function M.locate(base, opts)
  opts = opts or {}
  local exe = M.exe_name(base)
  local start = opts.start or vim.fn.getcwd()

  -- 1. Environment override.
  if opts.env then
    local from_env = vim.env[opts.env]
    if from_env and from_env ~= "" then
      return { command = from_env, working_dir = start, found = true }
    end
  end

  -- 2. Explicit path, then toolchain directory.
  if opts.cmd and opts.cmd ~= "" then
    return { command = opts.cmd, working_dir = start, found = is_file(opts.cmd) }
  end
  if opts.dir and opts.dir ~= "" then
    local candidate = opts.dir .. "/" .. exe
    if is_file(candidate) then
      return { command = candidate, working_dir = start, found = true }
    end
  end

  -- 3. Cargo build output, walking up from the start directory.
  local command, root = find_upward(start, exe)
  if command then
    return { command = command, working_dir = root, found = true }
  end

  -- 4. $PATH.
  local on_path = find_on_path(exe)
  if on_path then
    return { command = on_path, working_dir = start, found = true }
  end

  -- 5. Nothing found.
  return { command = exe, working_dir = start, found = false }
end

--- The directory to start searching from for `bufnr` — the buffer's own
--- directory when it is a real file, else the cwd.
function M.start_dir(bufnr)
  local name = vim.api.nvim_buf_get_name(bufnr or 0)
  if name ~= nil and name ~= "" then
    return vim.fs.dirname(name)
  end
  return vim.fn.getcwd()
end

return M
