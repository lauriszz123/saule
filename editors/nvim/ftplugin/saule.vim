setlocal commentstring=--\ %s
setlocal comments=s:--[[,m:\ ,e:]],:--

" Saule's canonical layout, matching `FmtOptions::default()` in `saule-fmt`,
" the IntelliJ code style defaults, and the VS Code extension: two spaces per
" level. A continuation line (inside an unclosed bracket) gets two levels —
" the indent model in `lua/saule/indent.lua` works that out.
setlocal expandtab
setlocal shiftwidth=2
setlocal softtabstop=2
setlocal tabstop=2

" `MAX_WIDTH` in `crates/saule-fmt/src/lib.rs`.
setlocal textwidth=0
setlocal colorcolumn=100

" Always end Saule files with a single trailing newline on write,
" regardless of whether the file was originally read without one.
setlocal fixendofline
setlocal endofline

" `%` jumps between block delimiters, not just brackets — the equivalent of the
" IntelliJ plugin's brace matcher, extended to Saule's word-keyword blocks.
" Requires the bundled matchit plugin (`:packadd matchit`, default in Neovim).
"
" `do` is listed because a *trailing block* — `Canvas() do … end`, the sugar
" for a call whose last argument is a block-bodied lambda — opens a block of
" its own. The `do` that merely ends a `for` / `while` header does not, and
" neither does an `fn` writing a type or an interface's bare signature;
" `b:match_skip` hides those, or the extra openers would pair every later `end`
" in the file with the wrong keyword. `lua/saule/match.lua` tells them apart
" using the same model that indents them.
let b:match_ignorecase = 0
let b:match_words =
      \ '\<\%(class\|interface\|enum\|if\|while\|for\|repeat\|try\|match\|fn\|do\)\>'
      \ . ':\<\%(elseif\|else\|catch\|case\)\>'
      \ . ':\<\%(end\|until\)\>'
let b:match_skip = "v:lua.require'saule.match'.skip()"

" Parameter info follows the cursor: the hint appears whenever the cursor is
" inside a call's parens, not only when `(` is typed. Set
" `vim.g.saule_auto_signature_help = false` to opt out.
lua require("saule.signature").attach(vim.api.nvim_get_current_buf())

" Inlay hints (inferred types, parameter names) on as soon as the server
" attaches, as in IntelliJ and VS Code. `vim.g.saule_inlay_hints = false` opts
" out; `:SauleInlayHints` toggles the current buffer.
lua require("saule.hints").attach(vim.api.nvim_get_current_buf())
command! -buffer SauleInlayHints lua require("saule.hints").toggle()

" `:SauleRun` runs the project, `:SauleRunFile` the current buffer — the two
" run configurations the IntelliJ plugin contributes.
command! -buffer -nargs=* SauleRun
      \ lua require("saule.run").project(<q-args>)
command! -buffer -nargs=* SauleRunFile
      \ lua require("saule.run").file(<q-args>)

let b:undo_ftplugin =
      \ "setlocal commentstring< comments< expandtab< shiftwidth< softtabstop<"
      \ . " tabstop< textwidth< colorcolumn< fixendofline< endofline<"
      \ . " | unlet! b:match_words b:match_ignorecase b:match_skip"
      \ . " | delcommand SauleRun | delcommand SauleRunFile"
      \ . " | delcommand SauleInlayHints"
