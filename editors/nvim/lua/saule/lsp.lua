-- Saule LSP client registration for nvim-lspconfig (works in NvChad,
-- LazyVim, or vanilla lspconfig setups).
--
-- The defaults point straight at this repository — no install step.
-- Override `repo` or `cmd` if you've cloned somewhere else.
--
-- Usage from NvChad (`lua/configs/lspconfig.lua`):
--
--   require("saule.lsp").setup()                       -- uses this repo's target/
--   require("saule.lsp").setup({ profile = "debug" }) -- pick the debug binary
--   require("saule.lsp").setup({ repo = "/other/path" })
--   require("saule.lsp").setup({ cmd  = { "saule-lsp" } }) -- bypass detection

local M = {}

--- Path to the Saule repository root that this Lua module ships from.
--- Derived from the location of this very file: `<repo>/editors/nvim/lua/saule/lsp.lua`
--- → strip the trailing four components.
local function default_repo()
  local this = debug.getinfo(1, "S").source:sub(2) -- drop leading "@"
  -- Walk up: lsp.lua → saule → lua → nvim → editors → <repo>
  return vim.fn.fnamemodify(this, ":p:h:h:h:h:h")
end

--- Resolve the server binary path from a repo root + cargo profile.
--- Tries `target/<profile>/saule-lsp[.exe]` and returns the first that exists.
local function resolve_cmd(repo, profile)
  local exe = (vim.fn.has("win32") == 1) and "saule-lsp.exe" or "saule-lsp"
  local candidates = {
    repo .. "/target/" .. profile .. "/" .. exe,
    -- Fallback: whichever profile actually has a binary, release first.
    repo .. "/target/release/" .. exe,
    repo .. "/target/debug/" .. exe,
  }
  for _, path in ipairs(candidates) do
    if vim.fn.filereadable(path) == 1 then
      return { path }
    end
  end
  -- Last resort: trust $PATH.
  return { exe }
end

--- Register and start the Saule language server.
---@param opts? table  Options:
---   * `repo`          — repo root (default: this checkout)
---   * `profile`       — "release" (default) or "debug"
---   * `cmd`           — fully override the server command
---   * `on_attach`     — buffer-local keymaps etc.
---   * `capabilities`  — completion capabilities from nvim-cmp / blink.cmp
---   * any other lspconfig option (forwarded verbatim)
function M.setup(opts)
  opts = opts or {}

  local ok_lsp, lspconfig = pcall(require, "lspconfig")
  if not ok_lsp then
    vim.notify("saule.lsp: nvim-lspconfig not installed", vim.log.levels.ERROR)
    return
  end
  local configs = require("lspconfig.configs")

  local repo = opts.repo or default_repo()
  local profile = opts.profile or "release"
  local cmd = opts.cmd or resolve_cmd(repo, profile)

  -- Strip the consumed-here keys so they don't leak into lspconfig.setup.
  opts.repo = nil
  opts.profile = nil
  opts.cmd = nil

  -- Define the server once. Re-running `setup({})` later just reuses the
  -- existing definition (lspconfig dedups on the key).
  if not configs.saule_lsp then
    configs.saule_lsp = {
      default_config = {
        cmd = cmd,
        filetypes = { "saule" },
        -- A Saule project is identified by either a Cargo workspace
        -- root (Saule's own repo layout) or the `saule.config` file
        -- the interpreter looks for. Falls back to the file's directory.
        root_dir = function(fname)
          return lspconfig.util.root_pattern("saule.config", "Cargo.toml", ".git")(fname)
            or vim.fs.dirname(fname)
        end,
        single_file_support = true,
        settings = {},
      },
      docs = {
        description = "Language server for the Saule programming language.",
      },
    }
  end

  -- Pull in NvChad's shared on_attach + capabilities when present so we
  -- match the rest of the user's LSP setup (keymaps, completion source).
  local defaults = { cmd = cmd }
  local ok_nv, nvconf = pcall(require, "nvchad.configs.lspconfig")
  if ok_nv then
    defaults.on_attach = nvconf.on_attach
    defaults.on_init = nvconf.on_init
    defaults.capabilities = nvconf.capabilities
  end

  lspconfig.saule_lsp.setup(vim.tbl_deep_extend("force", defaults, opts))
end

return M
