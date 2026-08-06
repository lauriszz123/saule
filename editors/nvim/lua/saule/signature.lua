-- Shows signature help whenever the cursor sits inside a call's argument list
-- — on entry, and again when you come back to edit an argument.
--
-- The IntelliJ plugin does this with a caret listener (see
-- `SauleParameterInfoAutoPopup`); this is the Neovim equivalent, so the two
-- editors pop the same hint at the same moments. `saule-lsp` answers
-- `textDocument/signatureHelp` at any offset inside the parens, so the trigger
-- is cursor position, not a keystroke.
--
-- Set `vim.g.saule_auto_signature_help = false` before the filetype loads to
-- turn this off and use `vim.lsp.buf.signature_help()` on demand instead.

local M = {}

--- How far back to scan for an enclosing `(`; keeps a deep cursor cheap.
local SCAN_LIMIT = 4000

--- Byte offset of the `(` whose argument list contains `caret`, or nil when the
--- cursor isn't inside one.
---
--- A backward scan is enough: the server decides whether there is actually a
--- call there. Strings and comments aren't excluded for the same reason — a
--- false positive costs one request that returns no signature.
function M.enclosing_open_paren(text, caret)
  local depth = 0
  local limit = math.max(1, caret - SCAN_LIMIT)
  for i = caret - 1, limit, -1 do
    local c = text:sub(i, i)
    if c == ")" then
      depth = depth + 1
    elseif c == "(" then
      if depth == 0 then
        return i
      end
      depth = depth - 1
    end
  end
  return nil
end

--- Text of the buffer up to the cursor, and the cursor's 1-based byte offset
--- within it. Only the text before the cursor matters to the scan, so the rest
--- of the buffer is never fetched.
local function text_before_cursor()
  local row, col = unpack(vim.api.nvim_win_get_cursor(0))
  local lines = vim.api.nvim_buf_get_lines(0, 0, row, false)
  if #lines == 0 then
    return "", 1
  end
  lines[#lines] = lines[#lines]:sub(1, col)
  local text = table.concat(lines, "\n")
  return text, #text + 1
end

--- Request signature help if the cursor is inside a `(...)` argument list.
function M.show_if_inside_call()
  if vim.fn.mode():sub(1, 1) ~= "i" then
    return
  end
  local clients = vim.lsp.get_clients({ bufnr = 0, method = "textDocument/signatureHelp" })
  if #clients == 0 then
    return
  end

  local text, caret = text_before_cursor()
  if M.enclosing_open_paren(text, caret) == nil then
    return
  end

  -- `focusable = false` keeps the float out of the window cycle, so it behaves
  -- like the IDE popup: visible, never stealing the cursor.
  vim.lsp.buf.signature_help({ focus = false, focusable = false, silent = true })
end

--- Attach the cursor listener to the current buffer.
function M.attach(bufnr)
  if vim.g.saule_auto_signature_help == false then
    return
  end
  local group = vim.api.nvim_create_augroup("SauleSignatureHelp" .. bufnr, { clear = true })
  vim.api.nvim_create_autocmd({ "CursorMovedI", "InsertEnter" }, {
    group = group,
    buffer = bufnr,
    callback = function()
      -- Deferred so the hint is requested after the keystroke has landed in the
      -- document, matching what the server has been told about the buffer.
      vim.schedule(M.show_if_inside_call)
    end,
  })
end

return M
