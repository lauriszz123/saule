-- Neovim 0.11+ `vim.lsp.config` / `vim.lsp.enable` definition for Saule.
--
-- Loaded automatically when this folder is on the runtimepath and the
-- user calls `vim.lsp.enable("saule")`. The binary path is resolved
-- from this file's location: `<repo>/editors/nvim/lsp/saule.lua` →
-- `<repo>/target/release/saule-lsp` (falling back to debug, then $PATH).

local function repo_root()
  local this = debug.getinfo(1, "S").source:sub(2)
  -- saule.lua → lsp → nvim → editors → <repo>
  return vim.fn.fnamemodify(this, ":p:h:h:h:h")
end

local function resolve_cmd()
  local repo = repo_root()
  local exe = (vim.fn.has("win32") == 1) and "saule-lsp.exe" or "saule-lsp"
  for _, profile in ipairs({ "release", "debug" }) do
    local path = repo .. "/target/" .. profile .. "/" .. exe
    if vim.fn.filereadable(path) == 1 then
      return { path }
    end
  end
  return { exe } -- trust $PATH as last resort
end

return {
  cmd = resolve_cmd(),
  filetypes = { "saule" },
  root_markers = { "saule.config", "Cargo.toml", ".git" },
  single_file_support = true,
  settings = {},
}

