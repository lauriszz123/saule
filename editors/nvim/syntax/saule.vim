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
syntax match  sauleOperator   "\.\.\.\|\.\.\|[+*/%=#]"
syntax match  sauleNullable   "??\|?\.\|[!?]"
syntax match  sauleBracket    "[(){}\[\]]"
syntax match  sauleDelimiter  "[,;:.]"
" sauleCompare defined last so multi-char compares win over single-char
" sauleOperator/sauleNullable at the same start position.
syntax match  sauleCompare    "==\|!=\|<=\|>=\|=>\|->\|[<>]"

" ── Numbers ─────────────────────────────────────────────────────────────
syntax match  sauleFloat       "\<\d\+\.\d\+\([eE][+-]\=\d\+\)\=\>"
syntax match  sauleNumber      "\<\d\+\>"

" ── Strings ─────────────────────────────────────────────────────────────
syntax region sauleString      start=+"+ skip=+\\"+ end=+"+ contains=sauleEscape
syntax match  sauleEscape      "\\." contained

" ── Types ───────────────────────────────────────────────────────────────
syntax keyword sauleType        integer float string boolean any
syntax match   sauleTypeName    "\<[A-Z][A-Za-z0-9_]*\>"

" ── Functions ───────────────────────────────────────────────────────────
syntax match   sauleFunction    "\<fn\>\s\+\zs[A-Za-z_][A-Za-z0-9_]*"
syntax match   sauleFuncCall    "\<[a-z_][A-Za-z0-9_]*\>\ze\s*("

" ── Keywords (defined late so they override the variable catch-all) ─────
syntax keyword sauleConditional if else then end
syntax keyword sauleRepeat      for while repeat until do in
syntax keyword sauleReturn      return
syntax keyword sauleStatement   break continue throw try catch
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
highlight default link sauleRepeat       Repeat
highlight default link sauleReturn       @keyword.return
highlight default link sauleStatement    Statement
highlight default link sauleDeclaration  Keyword
highlight default link sauleOperatorKW   sauleCompare
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
highlight default link sauleVariable     @variable
highlight default link sauleOperator     Operator
highlight default link sauleNullable     Special
highlight default link sauleBracket      @punctuation.bracket
highlight default link sauleDelimiter    @punctuation.delimiter

highlight sauleCompare ctermfg=Red guifg=#f7768e

let b:current_syntax = "saule"
