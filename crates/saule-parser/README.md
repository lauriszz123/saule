# saule-parser

Recursive-descent parser for Saule.

Consumes the `Spanned<Token>` stream from `saule-lexer` and produces a
`Module` (a list of `Spanned<Stmt>`). The grammar is Lua-flavoured —
`then` / `do` / `end` block keywords, keyword-led statements, and a standard
operator-precedence ladder for expressions:

```text
or → and → ==/!= → </<=/>/>= → ?? → .. → +/- → */ /%
   → unary (-, not, #) → postfix (., ?., [], (...), :m(...), !) → primary
```

Declarations cover `fn`, `class`, `interface`, `enum`, `import` and
`export`. Combined nodes span from their leftmost to rightmost child so
diagnostics highlight the whole construct.

## Place in the pipeline

```text
lexer → [parser] → semantic → typeck → interpreter
```

Depends on `saule-ast` and `saule-lexer`. Errors are reported as
`ParseError`.
