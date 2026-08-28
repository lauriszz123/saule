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
syntax match  sauleOperator   "\.\.\.\|\.\.\|[+\-*/%^=#]"
syntax match  sauleNullable   "??\|?\.\|[!?]"
syntax match  sauleBracket    "[(){}\[\]]"
syntax match  sauleDelimiter  "[,;:.]"
" Comparison operators (`==`, `!=`, `<=`, `>=`, `<`, `>`).
syntax match  sauleCompare    "==\|!=\|<=\|>=\|[<>]"
" Arrows are their own thing so they can be coloured distinctly from
" comparisons (return-type `->` and lambda `=>`).
syntax match  sauleArrow      "->\|=>"
" Bitwise operators (`&`, `|`, `~`, `<<`, `>>`). Defined after `sauleCompare`
" so `<<` and `>>` win over the single `<` / `>` at the same start column;
" `<=` and `>=` are unaffected because neither matches here.
syntax match  sauleBitwise    "<<\|>>\|[&|~]"
" Compound assignment (`+=`, `-=`, `*=`, `/=`, `%=`, `^=`, `..=`, `&=`, `|=`,
" `<<=`, `>>=`). Defined last so it wins the tie-break against `sauleOperator`
" and `sauleBitwise` at the same start column — otherwise `..=` would
" highlight as `..` followed by a stray `=`, and `<<=` as `<<` plus one.
syntax match  sauleOperator   "<<=\|>>=\|\.\.=\|[+\-*/%^&|]="

" ── Numbers ─────────────────────────────────────────────────────────────
" Mirrors `Lexer::number` in crates/saule-lexer: hex and binary prefixes take
" `_` separators, the fractional part needs a digit on both sides of the dot
" (so `1..2` stays concatenation), the integer part may be omitted, and an
" `f`/`F` suffix forces a float. There is no exponent notation.
syntax match  sauleNumber      "\<0[xX][0-9a-fA-F_]\+\>"
syntax match  sauleNumber      "\<0[bB][01_]\+\>"
syntax match  sauleFloat       "\<\d\+\.\d\+[fF]\=\>"
syntax match  sauleFloat       "\%([[:alnum:]_.]\)\@<!\.\d\+[fF]\=\>"
syntax match  sauleFloat       "\<\d\+[fF]\>"
syntax match  sauleNumber      "\<\d\+\>"

" ── Strings ─────────────────────────────────────────────────────────────
" Both quote styles, as in Lua: only the delimiter that opened the literal
" closes it, so the other quote needs no escaping inside.
syntax region sauleString      start=+"+ skip=+\\.+ end=+"+ contains=sauleEscape
syntax region sauleString      start=+'+ skip=+\\.+ end=+'+ contains=sauleEscape
syntax match  sauleEscape      "\\." contained

" ── Types ───────────────────────────────────────────────────────────────
" No `function`: a function's type is its signature, written `fn(...) -> T`,
" and `fn` is already highlighted as a declaration keyword.
syntax keyword sauleType        integer float string boolean any
                              \ table userdata thread
syntax match   sauleTypeName    "\<[A-Z][A-Za-z0-9_]*\>"

" ── Functions ───────────────────────────────────────────────────────────
" `fn name` (handles `fn name<T, U>` too — the `<...>` simply isn't captured).
syntax match   sauleFunction    "\<fn\>\s\+\zs[A-Za-z_][A-Za-z0-9_]*"
" Direct call: `ident(`. And generic call: `ident<T>(`, where the trailing
" `(` is what separates it from `a < b`. One level of nesting is allowed, so
" `filter<table<integer>>(xs)` counts; the same window the TextMate grammar
" uses in `#generics`.
syntax match   sauleFuncCall    "\<[a-z_][A-Za-z0-9_]*\>\ze\s*("
syntax match   sauleFuncCall    "\<[a-z_][A-Za-z0-9_]*\>\ze\s*<[^<>]*\%(<[^<>]*>[^<>]*\)*>\s*("

" A `table<...>` type is a bracketed list, not a run of comparisons, and the
" `>>` closing a nested one is two brackets rather than a right shift. The
" region nests via `contains=sauleGeneric`, so the inner list eats its own `>`
" and `table<table<string>>` stops highlighting as a shift. Anchored on
" `table` alone: it is the one generic head that cannot be a variable being
" compared, so `table <` is never a less-than.
syntax region  sauleGeneric matchgroup=sauleDelimiter
                              \ start="\%(\<table\>\s*\)\@<=<" end=">" oneline
                              \ contains=sauleGeneric,sauleType,sauleTypeName,sauleNullable,
                              \          sauleDelimiter,sauleBracket,sauleArrow,sauleDeclaration

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
highlight default link sauleBitwise      Operator
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
highlight sauleBitwise    ctermfg=14  guifg=#89ddff
highlight sauleArrow      ctermfg=13  guifg=#bb9af7

let b:current_syntax = "saule"
