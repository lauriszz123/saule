# saule-lexer

Hand-written tokenizer for Saule.

Turns source text into a `Vec<Spanned<Token>>`, preserving the byte span of
every token for diagnostics. Two entry points:

- `Lexer::tokenize` — the default; discards comments so the parser never
  sees trivia.
- `Lexer::tokenize_with_trivia` — keeps `--` line and `--[[ … ]]` block
  comments as `Token::LineComment` / `Token::BlockComment`, used by
  `saule-fmt` to round-trip comments.

## Place in the pipeline

```text
[lexer] → parser → semantic → typeck → interpreter
```

Depends on `saule-ast` (for `Spanned`). Errors are reported as
`LexerError`, a `miette`-aware diagnostic.
