-- Saule LSP client registration for nvim-lspconfig (works in NvChad, LazyVim,
-- or vanilla lspconfig setups).
--
-- The server binary is resolved by `saule.toolchain`, in the same order as the
-- IntelliJ plugin and the VS Code extension: `$SAULE_LSP_PATH`, then an
-- explicit path, then the Cargo build output found by walking up from the
-- current file, then `$PATH`. No install step is needed inside this repo.
--
-- Usage from NvChad (`lua/configs/lspconfig.lua`):
--
--   require("saule.lsp").setup()                          -- resolve automatically
--   require("saule.lsp").setup({ profile = "debug" })     -- prefer the debug binary
--   require("saule.lsp").setup({ repo = "/other/path" })  -- search from elsewhere
--   require("saule.lsp").setup({ cmd  = { "saule-lsp" } })-- bypass detection

local toolchain = require("saule.toolchain")

local M = {}

--- Path to the Saule repository root that this Lua module ships from.
--- Derived from the location of this very file:
--- `<repo>/editors/nvim/lua/saule/lsp.lua` → strip the trailing five components.
local function default_repo()
  local this = debug.getinfo(1, "S").source:sub(2) -- drop leading "@"
  -- Walk up: lsp.lua → saule → lua → nvim → editors → <repo>
  return vim.fn.fnamemodify(this, ":p:h:h:h:h:h")
end

--- Resolve the server command, honouring an explicit `profile` preference
--- before falling back to the shared discovery order.
local function resolve_cmd(repo, profile)
  local exe = toolchain.exe_name("saule-lsp")
  if profile then
    local candidate = repo .. "/target/" .. profile .. "/" .. exe
    if vim.fn.filereadable(candidate) == 1 then
      return { candidate }, repo
    end
  end
  local located = toolchain.locate("saule-lsp", {
    env = "SAULE_LSP_PATH",
    cmd = vim.g.saule_lsp_path,
    dir = vim.g.saule_toolchain_dir,
    start = repo,
  })
  if not located.found then
    vim.notify(
      "saule.lsp: could not find the 'saule-lsp' executable. Build it with "
        .. "`cargo build --release -p saule-lsp`, set `vim.g.saule_lsp_path`, "
        .. "or add 'saule-lsp' to your PATH.",
      vim.log.levels.WARN
    )
  end
  return { located.command }, located.working_dir
end

--- Register and start the Saule language server.
---@param opts? table  Options:
---   * `repo`          — where to start searching (default: this checkout)
---   * `profile`       — prefer "release" or "debug" build output
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
  local cmd, cmd_cwd = opts.cmd, nil
  if not cmd then
    cmd, cmd_cwd = resolve_cmd(repo, opts.profile)
  end

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
        cmd_cwd = cmd_cwd,
        filetypes = { "saule" },
        -- A Saule project is identified by either a Cargo workspace root
        -- (Saule's own repo layout) or the `saule.config` file the interpreter
        -- looks for. Falls back to the file's directory.
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

  -- Pull in NvChad's shared on_attach + capabilities when present so we match
  -- the rest of the user's LSP setup (keymaps, completion source).
  local defaults = { cmd = cmd, cmd_cwd = cmd_cwd }
  local ok_nv, nvconf = pcall(require, "nvchad.configs.lspconfig")
  if ok_nv then
    defaults.on_attach = nvconf.on_attach
    defaults.on_init = nvconf.on_init
    defaults.capabilities = nvconf.capabilities
  end

  lspconfig.saule_lsp.setup(vim.tbl_deep_extend("force", defaults, opts))
end

return M
