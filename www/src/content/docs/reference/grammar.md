---
title: "Grammar"
description: "The complete syntax of Saule, in the notation the Lua reference manual uses: {a} means zero or more a, [a] means an optional a, | separates…"
---

<!-- Generated from README.md by `npm run sync-docs`. Edit that file, not this one. -->

The complete syntax of Saule, in the notation the Lua reference manual uses:
`{a}` means zero or more `a`, `[a]` means an optional `a`, `|` separates
alternatives, and quoted text is literal. Names in `Title` case are lexical
tokens defined at the end.

It is transcribed from the recursive-descent parser in `crates/saule-parser`,
so it describes what the compiler actually accepts — including the corners the
guide simplifies.

### Chunks and statements

```ebnf
chunk ::= {stat}

stat ::= ';'
       | local
       | assign
       | compoundAssign
       | exp
       | if
       | while
       | repeat
       | forNum
       | forIn
       | try
       | 'return' [explist]
       | 'throw' exp
       | 'break'
       | 'continue'
       | decl

local  ::= 'local' nameDecl {',' nameDecl} ['=' explist]
assign ::= exp {',' exp} '=' explist

compoundAssign ::= exp compoundOp exp
compoundOp     ::= '+=' | '-=' | '*=' | '/=' | '%=' | '^=' | '..='
                 | '&=' | '|=' | '<<=' | '>>='

if     ::= 'if' exp 'then' chunk
           {'elseif' exp 'then' chunk}
           ['else' chunk] 'end'
while  ::= 'while' exp 'do' chunk 'end'
repeat ::= 'repeat' chunk 'until' exp
forNum ::= 'for' nameDecl '=' exp ',' exp [',' exp] 'do' chunk 'end'
forIn  ::= 'for' nameDecl {',' nameDecl} 'in' exp 'do' chunk 'end'
try    ::= 'try' chunk 'catch' Name ':' type chunk 'end'

nameDecl ::= Name [':' type]
explist  ::= exp {',' exp}
```

An assignment target is parsed as a full expression; whether it is something
you can actually assign to is decided later, by the semantic pass.

### Declarations

```ebnf
decl ::= ['export'] (function | class | interface | enum)
       | 'local' function
       | import

function  ::= 'fn' Name [typeParams] params ['->' type] chunk 'end'

class     ::= 'class' Name [typeParams]
              ['extends' Name [typeArgs]]
              ['implements' Name [typeArgs] {',' Name [typeArgs]}]
              {member} 'end'
member    ::= modifiers (method | field)
modifiers ::= ['static'] ['local'] | ['local'] ['static']
method    ::= 'fn' Name [typeParams] params ['->' type] chunk 'end'
field     ::= Name ':' type ['=' exp]

interface ::= 'interface' Name [typeParams]
              ['extends' Name [typeArgs] {',' Name [typeArgs]}]
              {methodSig} 'end'
methodSig ::= 'fn' Name [typeParams] params ['->' type]

enum      ::= 'enum' Name [typeParams] {variant} {enumMethod} 'end'
variant   ::= [','] Name ['=' exp | params]
enumMethod::= 'fn' Name params ['->' type] chunk 'end'

import    ::= 'import' ('*' | importName {',' importName})
              'from' (String | Name {'.' Name})
importName::= Name ['as' Name]

params ::= '(' [param {',' param}] ')'
param  ::= ['...'] (Name | 'self') [':' type] ['=' exp]
```

`export` applies only to the four declaration forms listed; there is no
`export local`. On a declared `fn` or method every parameter needs its type —
the `[':' type]` above is optional only inside a lambda, where the target type
supplies it, and on `self`, which is typed as the enclosing class.

### Types

```ebnf
type      ::= baseType ['?']
baseType  ::= Name [typeArgs]
            | 'table' '<' type [',' type] '>'
            | 'nil'
            | 'fn' '(' [type {',' type}] ')' '->' type
            | '(' [type {',' type}] ')'

typeArgs   ::= '<' type {',' type} '>'
typeParams ::= '<' Name {',' Name} '>'
```

`typeParams` **declares** — each entry is a bare name that stands for whatever
the user of the declaration picks. `typeArgs` **applies** — each entry is a
type, filling one of those slots. `Name typeArgs` in a type position is a
generic application: `Box<integer>`, `Result<string>`, `Repository<Player>`.
The count has to match what the named declaration declares, and `table<K, V>`
is its own form rather than an application of a `table` declaration.

A parenthesised list of one type is just grouping; two or more is a tuple,
which is how a function returning multiple values states its return type.
`table<V>` is the array form and `table<K, V>` the map form.

### Expressions

Written as a precedence ladder, loosest binding first, because that is how the
parser reads them. Each level is left-associative unless marked otherwise.

```ebnf
exp        ::= orExp
orExp      ::= andExp {'or' andExp}
andExp     ::= eqExp {'and' eqExp}
eqExp      ::= cmpExp {('==' | '!=') cmpExp}
cmpExp     ::= borExp {('<' | '<=' | '>' | '>=') borExp}
borExp     ::= bxorExp {'|' bxorExp}
bxorExp    ::= bandExp {'~' bandExp}
bandExp    ::= shiftExp {'&' shiftExp}
shiftExp   ::= coalesce {('<<' | '>>') coalesce}
coalesce   ::= concat ['??' coalesce]                    (* right *)
concat     ::= additive ['..' concat]                    (* right *)
additive   ::= multiply {('+' | '-') multiply}
multiply   ::= unary {('*' | '/' | '%') unary}
unary      ::= ('-' | 'not' | '#' | '~') unary | power
power      ::= cast ['^' unary]                          (* right *)
cast       ::= postfix {'as' type}
postfix    ::= primary {suffix}

suffix ::= '.' (Name | 'super')
         | '?.' Name
         | '[' exp ']'
         | [typeArgs] args
         | 'do' [params ['->' type]] chunk 'end'
         | '!'

args ::= '(' [arg {',' arg}] ')'
arg  ::= [Name ':'] exp

primary ::= Numeral | String | 'true' | 'false' | 'nil' | 'self'
          | Name
          | table
          | lambda
          | match
          | pipeline
          | '(' exp ')'

table ::= '{' [entry {',' entry} [',']] '}'
entry ::= Name ':' exp | String ':' exp | exp

lambda ::= 'fn' params ['->' type] chunk 'end'
         | '(' [param {',' param}] ')' ['->' type] '=>' exp
         | Name '=>' exp

match   ::= 'match' exp {arm} 'end'
arm     ::= 'case' pattern ['when' exp] 'then' (exp | chunk)
pattern ::= 'nil' | 'true' | 'false' | ['-'] Numeral | String
          | '_'
          | Name
          | Name '.' Name ['(' [pattern {',' pattern}] ')']
          | '(' [pattern {',' pattern}] ')'

pipeline ::= 'when' '(' exp ')' stage {stage}
stage    ::= ':' Name [typeArgs] args
```

Four parts of this need a word of explanation.

**`^` binds tighter than unary minus**, as in Lua: `-2 ^ 2` is `-(2 ^ 2)`, and
because the right operand is itself `unary`, `2 ^ -1` parses.

**The bitwise rungs sit where Lua 5.3 puts them** — just above comparison, in
the order `|`, `~`, `&`, shifts — so `flags & mask != 0` masks before it
compares, and `..`, `+` and `*` all bind tighter than a shift.

**`as` binds tighter than every binary operator but looser than the postfix
chain.** So `y as integer ?? 0` casts before it coalesces, and
`obj.field() as string` casts the call's result rather than the callee.

**The `do … end` suffix is the trailing-block form**, sugar for passing a
lambda as the final positional argument: `Panel(title: "x") do … end` is
`Panel(title: "x", fn() … end)`. It attaches only to something that is already
a call, and it is suppressed while parsing the header of a `while` or `for`,
where a `do` closes the header instead. Parenthesising re-enables it:
`while (next() do … end) do … end`.

There is no `obj:method()` call form. A method call is `obj.method()`; `:`
appears only in type ascriptions, named arguments, table keys, `catch`
bindings, and pipeline stages.

### Lexical elements

```ebnf
Name    ::= ('_' | letter) {'_' | letter | digit}
Numeral ::= integer | float
integer ::= digit {digit}
          | '0' ('x' | 'X') hexDigit {'_' | hexDigit}
          | '0' ('b' | 'B') binDigit {'_' | binDigit}
float   ::= digit {digit} '.' digit {digit} [fSuffix]
          | '.' digit {digit} [fSuffix]
          | digit {digit} fSuffix
fSuffix ::= 'f' | 'F'
String  ::= '"' {stringChar | escape} '"'
          | "'" {stringChar | escape} "'"
escape  ::= '\n' | '\t' | '\r' | '\0' | '\\' | '\"' | "\'"
comment ::= '--' {any} newline | '--[[' {any} ']]'
```

`Name` is ASCII only, and excludes the keywords:

```
and       as        break     case      catch     class     continue  do
else      elseif    end       enum      export    extends   false     fn
for       from      if        implements import    in        interface local
match     nil       not       or        repeat    return    self      static
super     then      throw     true      try       until     when      while
```

Strings take either quote style, and only the delimiter that opened one closes
it — so `'he said "hi"'` needs no escaping. The seven escapes above are the only
ones recognised; anything else after a backslash is an error, and there is no
hex or unicode escape. `_` groups digits in
hex and binary literals only. There is no exponent notation, no octal form, and
no `f` suffix on a hex literal (`0xFF_80f` is one hexadecimal integer, since
`f` is a hex digit).

Whitespace and comments separate tokens and are otherwise insignificant. `;` is
a separator, never a statement: it is accepted between statements, at the end of
a block, and at the end of a file, and is never required anywhere.
