" Vim syntax file for Saule
" Language: Saule
" Maintainer: saule
" Filenames: *.sau

if exists("b:current_syntax")
  finish
endif

syntax case match

" ── Catch-all variable (low priority — defined first so later rules win) ─
syntax match  sauleVariable    "\<[a-z_][A-Za-z0-9_]*\>"

" ── Operators / punctuation ─────────────────────────────────────────────
" Order matters: longer / more specific patterns are defined *after* the
" shorter ones so they win the tie-break at the same start column.
syntax match  sauleOperator   "\.\.\.\|\.\.\|[+\-*/%=#]"
syntax match  sauleNullable   "??\|?\.\|[!?]"
syntax match  sauleBracket    "[(){}\[\]]"
syntax match  sauleDelimiter  "[,;:.]"
" Comparison operators (`==`, `!=`, `<=`, `>=`, `<`, `>`).
syntax match  sauleCompare    "==\|!=\|<=\|>=\|[<>]"
" Arrows are their own thing so they can be coloured distinctly from
" comparisons (return-type `->` and lambda `=>`).
syntax match  sauleArrow      "->\|=>"

" ── Numbers ─────────────────────────────────────────────────────────────
syntax match  sauleFloat       "\<\d\+\.\d\+\([eE][+-]\=\d\+\)\=\>"
syntax match  sauleNumber      "\<\d\+\>"

" ── Strings ─────────────────────────────────────────────────────────────
syntax region sauleString      start=+"+ skip=+\\"+ end=+"+ contains=sauleEscape
syntax match  sauleEscape      "\\." contained

" ── Types ───────────────────────────────────────────────────────────────
syntax keyword sauleType        integer float string boolean any
                              \ function table userdata thread number
syntax match   sauleTypeName    "\<[A-Z][A-Za-z0-9_]*\>"

" ── Functions ───────────────────────────────────────────────────────────
" `fn name` (handles `fn name<T, U>` too — the `<...>` simply isn't captured).
syntax match   sauleFunction    "\<fn\>\s\+\zs[A-Za-z_][A-Za-z0-9_]*"
" Direct call: ident( ... and generic call: ident<T>(
syntax match   sauleFuncCall    "\<[a-z_][A-Za-z0-9_]*\>\ze\s*("
syntax match   sauleFuncCall    "\<[a-z_][A-Za-z0-9_]*\>\ze\s*<[A-Za-z_][^<>]*>\s*("

" ── Keywords (defined late so they override the variable catch-all) ─────
syntax keyword sauleConditional if else elseif then end
" Pure loop intros stay `Repeat`; structural `do`/`in` get their own group
" so they don't pick up the loud loop colour many themes use for `Repeat`.
syntax keyword sauleRepeat      for while repeat until
syntax keyword sauleStructural  do in
syntax keyword sauleReturn      return
syntax keyword sauleStatement   break continue throw try catch
syntax keyword sauleMatch       match case when
syntax keyword sauleDeclaration class interface enum fn
                              \ local static export import from as
                              \ extends implements
syntax keyword sauleOperatorKW  and or not
syntax keyword sauleBoolean     true false
syntax keyword sauleNil         nil
syntax keyword sauleSelf        self super

" ── Comments (block defined last so `--[[` wins over `--` line comment) ─
syntax match  sauleLineComment  "--.*$" contains=@Spell
syntax region sauleBlockComment start=+--\[\[+ end=+\]\]+ contains=@Spell

" ── Highlight links ─────────────────────────────────────────────────────
highlight default link sauleBlockComment Comment
highlight default link sauleLineComment  Comment
highlight default link sauleString       String
highlight default link sauleEscape       SpecialChar
highlight default link sauleFloat        Float
highlight default link sauleNumber       Number
highlight default link sauleConditional  Conditional
highlight default link sauleRepeat       Keyword
highlight default link sauleStructural   Keyword
highlight default link sauleReturn       @keyword.return
highlight default link sauleStatement    Statement
highlight default link sauleMatch        Conditional
highlight default link sauleDeclaration  Keyword
highlight default link sauleOperatorKW   Operator
highlight default link sauleBoolean      Boolean
highlight default link sauleNil          Constant
if has('nvim')
  highlight default link sauleSelf       @variable.builtin
else
  highlight default link sauleSelf       Special
endif
highlight default link sauleType         Type
highlight default link sauleTypeName     Type
highlight default link sauleFunction     Function
highlight default link sauleFuncCall     Function
highlight default link sauleBracket      Delimiter
highlight default link sauleDelimiter    Delimiter

" ── Explicit fallback palette ───────────────────────────────────────────
" Direct `highlight` (not `highlight default link`) so these paint visibly
" even when the user's colourscheme doesn't give the linked group a strong
" colour. Variables, operators, nullables, comparisons, and arrows each get
" their own hue so they read at a glance. Tweak to taste.
highlight sauleVariable   ctermfg=15  guifg=#c0caf5
highlight sauleOperator   ctermfg=14  guifg=#89ddff
highlight sauleOperatorKW ctermfg=13  guifg=#bb9af7
highlight sauleNullable   ctermfg=13  guifg=#bb9af7
highlight sauleCompare    ctermfg=9   guifg=#f7768e
highlight sauleArrow      ctermfg=13  guifg=#bb9af7

let b:current_syntax = "saule"
