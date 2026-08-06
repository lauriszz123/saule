-- Neovim 0.11+ `vim.lsp.config` / `vim.lsp.enable` definition for Saule.
--
-- Loaded automatically when this folder is on the runtimepath and the user
-- calls `vim.lsp.enable("saule")`.
--
-- The binary is resolved by `saule.toolchain`, in the same order as the
-- IntelliJ plugin and the VS Code extension: `$SAULE_LSP_PATH`, then
-- `vim.g.saule_lsp_path` / `vim.g.saule_toolchain_dir`, then the Cargo build
-- output found by walking up from the file's directory, then `$PATH`.

local toolchain = require("saule.toolchain")

local located = toolchain.locate("saule-lsp", {
  env = "SAULE_LSP_PATH",
  cmd = vim.g.saule_lsp_path,
  dir = vim.g.saule_toolchain_dir,
  start = toolchain.start_dir(0),
})

if not located.found then
  vim.notify(
    "saule: could not find the 'saule-lsp' executable. Syntax highlighting "
      .. "and indentation are active, but diagnostics, hover and navigation "
      .. "are disabled. Build it with `cargo build --release -p saule-lsp`, "
      .. "set `vim.g.saule_lsp_path`, or add 'saule-lsp' to your PATH.",
    vim.log.levels.WARN
  )
end

return {
  cmd = { located.command },
  -- Launched from the directory holding the build output, so the server
  -- resolves project files the way the other editors' clients do.
  cmd_cwd = located.working_dir,
  filetypes = { "saule" },
  root_markers = { "saule.config", "Cargo.toml", ".git" },
  single_file_support = true,
  settings = {},
}
