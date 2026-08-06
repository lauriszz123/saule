-- Inlay hints — the "ghost text" showing inferred local types and
-- parameter-name labels that `saule-lsp` returns for `textDocument/inlayHint`.
--
-- The IntelliJ plugin shows these as soon as the server attaches (LSP4IJ turns
-- them on by default), and so does VS Code (`editor.inlayHints.enabled` is
-- `"on"` out of the box). Neovim has the same feature but leaves it off until
-- something calls `vim.lsp.inlay_hint.enable()` — so this does, for Saule
-- buffers only.
--
-- Set `vim.g.saule_inlay_hints = false` to opt out, or toggle per buffer with
-- `:SauleInlayHints`.

local M = {}

--- `vim.lsp.inlay_hint.enable` changed signature during 0.10: it took
--- `(bufnr, enable)` before settling on `(enable, filter)`. Try the current
--- form and fall back, so the integration works on both.
local function set_enabled(bufnr, enabled)
  local ok = pcall(vim.lsp.inlay_hint.enable, enabled, { bufnr = bufnr })
  if not ok then
    pcall(vim.lsp.inlay_hint.enable, bufnr, enabled)
  end
end

local function is_enabled(bufnr)
  local ok, enabled = pcall(vim.lsp.inlay_hint.is_enabled, { bufnr = bufnr })
  if ok then
    return enabled
  end
  return false
end

--- True when `client` can answer `textDocument/inlayHint`.
local function supports_hints(client)
  if client.supports_method then
    return client:supports_method("textDocument/inlayHint")
  end
  return client.server_capabilities ~= nil
    and client.server_capabilities.inlayHintProvider ~= nil
end

--- Turn hints on for `bufnr` if any attached client provides them.
local function enable_if_supported(bufnr)
  if vim.g.saule_inlay_hints == false then
    return
  end
  if not vim.api.nvim_buf_is_valid(bufnr) then
    return
  end
  for _, client in ipairs(vim.lsp.get_clients({ bufnr = bufnr })) do
    if supports_hints(client) then
      set_enabled(bufnr, true)
      return
    end
  end
end

--- Wire up `bufnr`. Called from the ftplugin, which may run either before or
--- after the language server attaches — so both orders are handled: enable now
--- for a client that is already there, and listen for one that arrives later.
function M.attach(bufnr)
  if vim.g.saule_inlay_hints == false then
    return
  end

  enable_if_supported(bufnr)

  local group =
    vim.api.nvim_create_augroup("SauleInlayHints" .. bufnr, { clear = true })
  vim.api.nvim_create_autocmd("LspAttach", {
    group = group,
    buffer = bufnr,
    callback = function(args)
      -- Deferred: at LspAttach time the client is registered but the buffer is
      -- not yet in its attached set on every Neovim version.
      vim.schedule(function()
        enable_if_supported(args.buf)
      end)
    end,
  })
end

--- Toggle hints for the current buffer, for `:SauleInlayHints`.
function M.toggle(bufnr)
  bufnr = bufnr or vim.api.nvim_get_current_buf()
  local now = is_enabled(bufnr)
  set_enabled(bufnr, not now)
  vim.notify("saule: inlay hints " .. (now and "off" or "on"), vim.log.levels.INFO)
end

return M
