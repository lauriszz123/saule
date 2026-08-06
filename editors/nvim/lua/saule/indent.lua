-- Works out the indentation of any line of Saule source.
--
-- `saule-lsp` owns *formatting*, but it is not in the loop while you type, so
-- the editor needs its own answer to "how far in does this line start?" for
-- pressing Enter, for `=`, and for the auto-dedent of `end`.
--
-- This is a port of the IntelliJ plugin's `SauleIndentModel` (see
-- `editors/intellij/.../format/SauleIndentModel.kt`) and of the VS Code
-- extension's `src/indent.ts`, kept deliberately comparable with both so the
-- three editors indent identically. All are in turn derived from the printer
-- in `crates/saule-fmt/src/lib.rs`; keep them in step or the editors and
-- `saule fmt` will disagree.
--
-- The model half of this file is pure Lua — no `vim.*` — so it can be tested
-- with a plain `lua` interpreter. Only `M.indentexpr` touches the editor.

local M = {}

-- A block that is currently open.
--
-- `INTERFACE` is tracked separately because an interface body holds bare
-- method *signatures* — `fn foo()` with no `end` — so `fn` must not open a
-- block there.
--
-- `CASE` is a `match` arm: `case … then` followed by a newline indents its
-- statements, and the arm is closed by the next `case` or by the `match`'s
-- `end` rather than by one of its own.
--
-- `LOOP_HEADER` is a `for` / `while` block whose header has not reached its
-- `do` yet. It is a block like any other — it just remembers that the next
-- `do` belongs to the header and so must not open a second one.
local BLOCK, LOOP_HEADER, INTERFACE, CASE, BRACKET = 1, 2, 3, 4, 5

local function set(list)
  local out = {}
  for _, item in ipairs(list) do
    out[item] = true
  end
  return out
end

--- Keywords that open a block closed by `end` (or, for `repeat`, `until`).
local BLOCK_OPENERS = set({ "class", "enum", "if", "repeat", "try", "match" })

--- Loop keywords, whose header runs up to a `do`.
---
--- Kept apart from `BLOCK_OPENERS` only so that `do` can tell the two uses of
--- itself apart: it terminates a loop header (which has already opened a
--- block), and anywhere else it opens a *trailing block* — `Canvas() do … end`,
--- the sugar for a call whose last argument is a block-bodied lambda
--- (`saule_fmt::trailing_block_arg`). Both bodies are one level in.
local LOOP_OPENERS = set({ "while", "for" })

--- Keywords that continue the enclosing block: written one level out.
local CONTINUATIONS = set({ "else", "elseif", "catch" })

--- Keywords that close the enclosing block: also written one level out.
local CLOSERS = set({ "end", "until" })

--- Every keyword that sits one level out from the body above it.
---
--- `case` is in here because a `match` arm with a block body is closed by the
--- next `case`, not by an `end`. Typing any of these as the whole of a line is
--- what triggers the editor's auto-dedent.
M.DEDENT_KEYWORDS = set({ "end", "until", "else", "elseif", "catch", "case" })

local OPEN_BRACKETS = set({ "(", "[", "{" })
local CLOSE_BRACKETS = set({ ")", "]", "}" })

--- Tokens after which an `fn` starts a *type* rather than a definition.
---
--- `fn(T) -> U` written as a parameter's type, a local's annotation or a
--- return type has no body and no `end`. Treating its `fn` as a block opener
--- leaves a frame on the stack that nothing ever closes, and the damage
--- compounds: with a stray block on top, the `)` closing the enclosing
--- parameter list no longer matches a bracket frame, so that stays open too.
---
--- Only these four contexts qualify, and each admits nothing but a type: `:`
--- ascribes one, `->` introduces a return type, `<` opens a generic argument
--- list, and `as` casts to one. Notably absent are `,` and `(` — an anonymous
--- function passed as an argument (`map(xs, fn(x) … end)`) follows those, and
--- it really is a block.
local TYPE_POSITION = set({ ":", "->", "<", "as" })

local THREE_CHAR_OPS = set({ "...", "..=" })
local TWO_CHAR_OPS = set({
  "..", "+=", "-=", "*=", "/=", "%=", "^=",
  "?.", "??", "==", "!=", "<=", ">=", "->", "=>",
})

-- ── tokenizer ───────────────────────────────────────────────────────────────

local function is_space(c)
  return c == " " or c == "\t" or c == "\n" or c == "\r" or c == "\f" or c == "\v"
end

local function is_digit(c)
  return c >= "0" and c <= "9"
end

local function is_ident_start(c)
  return c == "_" or c:match("%a") ~= nil
end

local function is_ident_part(c)
  return c == "_" or c:match("%w") ~= nil
end

--- The significant tokens of `text[1..limit]`, as a list of `{word, start}`
--- with `start` a 1-based byte index.
---
--- Whitespace and comments carry no block structure and are skipped; strings
--- are skipped whole, so `"end end end"` contributes nothing. Multi-character
--- operators are matched longest-first, or `->` would arrive as `-` then `>`
--- and stop marking a return type.
local function tokens(text, limit)
  local out = {}
  local n = math.min(limit, #text)
  local i = 1
  while i <= n do
    local c = text:sub(i, i)

    if is_space(c) then
      i = i + 1
    elseif c == "-" and text:sub(i + 1, i + 1) == "-" then
      -- Comments: `--[[ … ]]` block, `--` line. Before the `-` operator.
      if text:sub(i + 2, i + 3) == "[[" then
        local close = text:find("]]", i + 4, true)
        i = close and (close + 2) or (#text + 1)
      else
        local nl = text:find("\n", i + 2, true)
        i = nl or (#text + 1)
      end
    elseif c == '"' or c == "'" then
      -- Both quote styles, as in Lua; the opening quote is the only one that
      -- closes the literal, and an unterminated one stops at the newline.
      local j = i + 1
      while j <= #text do
        local ch = text:sub(j, j)
        if ch == "\\" then
          j = j + 2
        elseif ch == c then
          j = j + 1
          break
        elseif ch == "\n" then
          break
        else
          j = j + 1
        end
      end
      i = j
    elseif is_ident_start(c) then
      local j = i + 1
      while j <= #text and is_ident_part(text:sub(j, j)) do
        j = j + 1
      end
      out[#out + 1] = { word = text:sub(i, j - 1), start = i }
      i = j
    elseif is_digit(c) then
      local j = i + 1
      while j <= #text do
        local ch = text:sub(j, j)
        if is_ident_part(ch) or ch == "." then
          j = j + 1
        else
          break
        end
      end
      out[#out + 1] = { word = text:sub(i, j - 1), start = i }
      i = j
    else
      local three = text:sub(i, i + 2)
      local two = text:sub(i, i + 1)
      if THREE_CHAR_OPS[three] then
        out[#out + 1] = { word = three, start = i }
        i = i + 3
      elseif TWO_CHAR_OPS[two] then
        out[#out + 1] = { word = two, start = i }
        i = i + 2
      else
        out[#out + 1] = { word = c, start = i }
        i = i + 1
      end
    end
  end
  return out
end

-- ── the stack machine ───────────────────────────────────────────────────────

local function apply(word, previous, stack)
  local top = stack[#stack]
  if word == "interface" then
    stack[#stack + 1] = INTERFACE
  elseif word == "fn" then
    -- Also covers lambdas — `fn(x) … end` is a block like any other. A `fn` in
    -- type position (`f: fn(T) -> U`) opens nothing: it is a type, it has no
    -- body, and no `end` will ever come for it.
    if top ~= INTERFACE and not (previous and TYPE_POSITION[previous]) then
      stack[#stack + 1] = BLOCK
    end
  elseif BLOCK_OPENERS[word] then
    stack[#stack + 1] = BLOCK
  elseif LOOP_OPENERS[word] then
    stack[#stack + 1] = LOOP_HEADER
  elseif word == "do" then
    -- Closing a loop header costs nothing — `for`/`while` opened the block
    -- already — but a `do` anywhere else is a trailing block, and that one is
    -- its own level.
    if top == LOOP_HEADER then
      stack[#stack] = BLOCK
    else
      stack[#stack + 1] = BLOCK
    end
  elseif CLOSERS[word] then
    -- Pop one real block. Any `case` arms — and any brackets left open by
    -- half-typed code — are discarded with it, mirroring how `end` closes a
    -- `match` and all of its arms at once.
    while
      #stack > 0
      and stack[#stack] ~= BLOCK
      and stack[#stack] ~= LOOP_HEADER
      and stack[#stack] ~= INTERFACE
    do
      stack[#stack] = nil
    end
    if #stack > 0 then
      stack[#stack] = nil
    end
  elseif word == "case" then
    if top == CASE then
      stack[#stack] = nil
    end
    stack[#stack + 1] = CASE
  elseif OPEN_BRACKETS[word] then
    stack[#stack + 1] = BRACKET
  elseif CLOSE_BRACKETS[word] then
    if top == BRACKET then
      stack[#stack] = nil
    end
  end
end

--- `match` arms open above the block that `end` is about to close.
local function open_case_frames(stack)
  local n = 0
  for i = #stack, 1, -1 do
    if stack[i] == CASE then
      n = n + 1
    elseif stack[i] ~= BRACKET then
      -- A bracket is half-typed code; not a level of its own here.
      return n
    end
  end
  return n
end

--- The indent the line spanning `[line_start, line_end]` of `text` should
--- have, as `blocks, continuations`. Byte indices are 1-based and inclusive of
--- `line_start`, exclusive of `line_end`.
---
--- The line's own first token matters: `end`, `else` and friends sit one level
--- out from the body they close, so the caller gets the right answer for a
--- line that is already (partly) typed as well as for an empty one.
function M.indent_for_line(text, line_start, line_end)
  local stack = {}
  local first_on_line, previous

  for _, token in ipairs(tokens(text, line_end - 1)) do
    if token.start >= line_start then
      first_on_line = token.word
      break
    end
    apply(token.word, previous, stack)
    previous = token.word
  end

  local blocks = 0
  for _, frame in ipairs(stack) do
    if frame ~= BRACKET then
      blocks = blocks + 1
    end
  end
  local brackets = #stack - blocks
  local top = stack[#stack]

  local block_dedent, bracket_dedent = 0, 0
  if first_on_line == nil then
    -- A blank line takes the enclosing block's indent.
  elseif CLOSERS[first_on_line] then
    block_dedent = 1 + open_case_frames(stack)
  elseif CONTINUATIONS[first_on_line] then
    block_dedent = 1
  elseif first_on_line == "case" then
    -- A `case` only steps out when a previous arm is still open; the first arm
    -- of a `match` already sits at the body level.
    if top == CASE then
      block_dedent = 1
    end
  elseif CLOSE_BRACKETS[first_on_line] then
    if top == BRACKET then
      bracket_dedent = 1
    end
  end

  return math.max(0, blocks - block_dedent), math.max(0, brackets - bracket_dedent)
end

--- Keywords that open a block only sometimes; see `M.false_openers`.
local AMBIGUOUS_OPENERS = set({ "fn", "do" })

--- The 1-based byte offsets of every keyword in `text` that *looks* like it
--- opens a block but does not, as a set: a `do` closing a `for` / `while`
--- header (the loop opened the block), an `fn` writing a type
--- (`f: fn(T) -> U`), and an interface's bare method signatures.
---
--- `%` needs this. `b:match_words` in `ftplugin/saule.vim` pairs the block
--- keywords with `end` by counting them, so it has to be shown the openers
--- that really are one — otherwise a trailing block's `end` pairs with the
--- enclosing `fn`, exactly the bug this model has for indenting.
---
--- Neovim-only: the other two ports match brackets rather than keywords, so
--- there is nothing to keep in step with here. The answer comes out of the
--- shared stack machine either way — a keyword opens a block if and only if
--- applying it pushes a frame.
function M.false_openers(text)
  local stack, previous, out = {}, nil, {}
  for _, token in ipairs(tokens(text, #text)) do
    local before = #stack
    apply(token.word, previous, stack)
    if #stack == before and AMBIGUOUS_OPENERS[token.word] then
      out[token.start] = true
    end
    previous = token.word
  end
  return out
end

local MAX_KEYWORD_LENGTH = 6 -- `elseif`

--- True when the word being typed just before `offset` is the whole of its
--- line and could change that line's indent — i.e. it is one of
--- `DEDENT_KEYWORDS`, or it *was* one until the character that has just
--- extended it (`end` → `endless`), which has to put the indent back.
function M.keyword_typed_at(text, offset)
  local line_start = offset
  while line_start > 1 and text:sub(line_start - 1, line_start - 1) ~= "\n" do
    line_start = line_start - 1
  end

  local word = text:sub(line_start, offset - 1):gsub("^%s+", ""):gsub("%s+$", "")
  if #word == 0 or #word > MAX_KEYWORD_LENGTH + 1 then
    return false
  end
  if word:match("^%a+$") == nil then
    return false
  end

  return M.DEDENT_KEYWORDS[word] == true
    or M.DEDENT_KEYWORDS[word:sub(1, -2)] == true
end

-- ── editor glue ─────────────────────────────────────────────────────────────

--- `indentexpr` for Saule buffers: the indent, in columns, of `v:lnum`.
---
--- One continuation is two indents — the ratio the IntelliJ code style
--- defaults to (indent 2, continuation 4) and the one `saule fmt` prints.
function M.indentexpr()
  local lnum = vim.v.lnum
  local lines = vim.api.nvim_buf_get_lines(0, 0, lnum, false)
  local text = table.concat(lines, "\n")

  -- Byte offset of the target line inside `text`, 1-based.
  local line_start = 1
  for i = 1, lnum - 1 do
    line_start = line_start + #lines[i] + 1
  end

  local blocks, continuations =
    M.indent_for_line(text, line_start, line_start + #(lines[lnum] or ""))

  local sw = vim.fn.shiftwidth()
  return blocks * sw + continuations * sw * 2
end

return M
