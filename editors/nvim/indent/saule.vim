" Indentation for Saule, computed by the shared indent model in
" `lua/saule/indent.lua` — the same model the IntelliJ plugin and the VS Code
" extension use, so all three editors indent identically.
"
" This drives `=`, `gg=G`, `o`/`O`, and — via 'indentkeys' below — the
" auto-dedent of block-closing keywords as they are typed.

if exists("b:did_indent")
  finish
endif
let b:did_indent = 1

setlocal indentexpr=v:lua.require'saule.indent'.indentexpr()

" Saule closes blocks with `end`, not `}`, so the brace-triggered dedent other
" languages rely on never fires. Each keyword is registered instead, plus the
" closing brackets that end a continuation line.
"
" `0=` means "re-indent when this word is typed at the start of the line";
" `o`/`O` cover opening a line, and `!^F` keeps CTRL-F working.
setlocal indentkeys=o,O,!^F,0=end,0=until,0=else,0=elseif,0=catch,0=case,0),0],0}

let b:undo_indent = "setlocal indentexpr< indentkeys<"
