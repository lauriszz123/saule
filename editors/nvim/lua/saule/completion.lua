-- Two adjustments that make the completion popup say what `saule-lsp` meant.
--
-- Both are about the same thing: the server has answered carefully, and the
-- completion plugin discards half of the answer. One drops what the server
-- deliberately left out; the other restores the order it asked for.
--
-- ── 1. no words scraped out of the buffer ───────────────────────────────────
--
-- Completion plugins ship a source that scrapes words out of the open buffers
-- (`buffer` for nvim-cmp) and offers them everywhere, independently of the
-- language server. That puts noise exactly where the server is deliberately
-- silent: a declaration name. `saule-lsp` answers `fn <name>`, `class <name>`,
-- `local <name>`, a parameter, a field, an enum variant — every position where
-- you are *inventing* a name — with nothing at all, because the names already
-- in scope are precisely the wrong suggestions there. A word scraper puts them
-- straight back, so naming a new `fn` offers every similar-looking word in the
-- file.
--
-- The VS Code extension turns the same thing off with
-- `editor.wordBasedSuggestions: "off"`, and this is the Neovim counterpart.
--
-- Only Saule buffers are touched, and only the word sources are dropped —
-- whatever else is configured (LSP, snippets, path, …) is kept in the order it
-- was given, so this composes with any existing setup rather than replacing
-- it. Set `vim.g.saule_word_completion = true` to keep the word sources.
--
-- ── 2. the server's ranking, honoured ───────────────────────────────────────
--
-- nvim-cmp ranks by LSP *kind* before it looks at `sortText`, so a method
-- always outranks a field whatever either one means here. `saule-lsp` ranks
-- by what the cursor is actually doing — the parameters of the call you are
-- inside, then whatever fits the type of the slot you are filling, then
-- locals, members, module functions, classes, the stdlib — and carries all of
-- it in `sortText`. See `promote_sort_text`.

local M = {}

--- Sources that offer *words* rather than knowledge of the language. Keyed by
--- the `name` a source is registered under with nvim-cmp.
local WORD_SOURCES = {
  buffer = true,
  treesitter = true,
  tags = true,
  spell = true,
  rg = true,
  dictionary = true,
}

--- The word sources removed from `sources`, or nil when there are none.
local function without_word_sources(sources)
  if vim.g.saule_word_completion == true then
    return nil
  end
  local kept, dropped = {}, false
  for _, source in ipairs(sources) do
    if type(source) == "table" and WORD_SOURCES[source.name] then
      dropped = true
    else
      kept[#kept + 1] = source
    end
  end
  return dropped and kept or nil
end

--- `comparators` with `sort_text` moved ahead of `kind`, or nil when it
--- already is (or when there is no `kind` to get ahead of).
---
--- nvim-cmp ranks by the LSP *kind* before it looks at the server's
--- `sortText`, and kinds are compared by their protocol number: Method is 2,
--- Field is 5. So a method always outranks a field however irrelevant it is
--- — which is why `VStack(ali…)` offered the inherited `aligned()` above the
--- `alignment:` parameter it is actually asking for.
---
--- `saule-lsp` spends real effort on that order. It puts the parameters of
--- the call you are inside first, then whatever fits the type of the slot
--- you are filling, then locals, members, module functions, classes, the
--- stdlib, keywords — the whole point being that what ranks first depends on
--- where the cursor is. All of that is carried in `sortText` and thrown away
--- here, so this promotes it above `kind` and leaves the rest of the chain
--- alone: `offset` and `exact` still come first, and the fuzzy `score` still
--- orders items the server ranked equally.
local function promote_sort_text(comparators)
  local ok, compare = pcall(require, "cmp.config.compare")
  if not ok then
    return nil
  end
  local kind_at, sort_at
  for i, c in ipairs(comparators) do
    if c == compare.kind and kind_at == nil then
      kind_at = i
    elseif c == compare.sort_text and sort_at == nil then
      sort_at = i
    end
  end
  if kind_at == nil or (sort_at ~= nil and sort_at < kind_at) then
    return nil
  end
  local out = {}
  for i, c in ipairs(comparators) do
    if i == kind_at then
      out[#out + 1] = compare.sort_text
    end
    if c ~= compare.sort_text then
      out[#out + 1] = c
    end
  end
  return out
end

--- Apply both nvim-cmp adjustments to the current buffer, in one write —
--- `cmp.setup.buffer` replaces the buffer's config rather than merging into
--- it, so they have to go together.
---
--- Returns true once it has had its say, so the caller can stop retrying.
local function configure_cmp()
  local ok, cmp = pcall(require, "cmp")
  if not ok then
    return false
  end

  -- `get_config()` already folds in any buffer-local override, so re-running
  -- this on a buffer that was handled finds nothing left to change.
  local ok_config, config = pcall(cmp.get_config)
  if not ok_config or type(config) ~= "table" then
    return true
  end

  local buffer_config = {}
  if type(config.sources) == "table" then
    buffer_config.sources = without_word_sources(config.sources)
  end
  local comparators = type(config.sorting) == "table" and config.sorting.comparators
  if type(comparators) == "table" then
    local promoted = promote_sort_text(comparators)
    if promoted then
      buffer_config.sorting =
        vim.tbl_extend("force", config.sorting, { comparators = promoted })
    end
  end

  if buffer_config.sources == nil and buffer_config.sorting == nil then
    return true
  end
  -- Anything left nil here is inherited from the global config, so a change
  -- to only one of the two does not freeze the other.
  pcall(cmp.setup.buffer, buffer_config)
  return true
end

--- Wire up `bufnr`. Called from the ftplugin.
---
--- The completion plugin is often lazy-loaded and may not exist yet when the
--- filetype is set — `require` pulls it in under lazy.nvim, but not under
--- every setup — so a plugin that is still absent gets one more chance at the
--- first `InsertEnter`, which is the earliest moment its absence would matter.
function M.attach(bufnr)
  if not vim.api.nvim_buf_is_valid(bufnr) then
    return
  end
  if configure_cmp() then
    return
  end

  vim.api.nvim_create_autocmd("InsertEnter", {
    group = vim.api.nvim_create_augroup("SauleCompletion" .. bufnr, { clear = true }),
    buffer = bufnr,
    once = true,
    callback = function()
      configure_cmp()
    end,
  })
end

return M
