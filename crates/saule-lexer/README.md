# saule-lexer

Hand-written tokenizer for Saule.

Turns source text into a `Vec<Spanned<Token>>`, preserving the byte span of
every token for diagnostics. Two entry points:

- `Lexer::tokenize` — the default; discards comments so the parser never
  sees trivia.
- `Lexer::tokenize_with_trivia` — keeps `--` line and `--[[ … ]]` block
  comments as `Token::LineComment` / `Token::BlockComment`, used by
  `saule-fmt` to round-trip comments.

## Numeric literals

Saule never auto-promotes between `integer` and `float`, so float literals get
two shorthands that save writing a redundant `0` or `.0`:

| Form    | Token          | Notes |
|---------|----------------|-------|
| `12`    | `Int(12)`      | |
| `1.5`   | `Float(1.5)`   | |
| `.5`    | `Float(0.5)`   | The integer part may be omitted. |
| `1f`    | `Float(1.0)`   | An `f` / `F` suffix forces a float. |
| `2.5f`  | `Float(2.5)`   | The suffix is allowed but redundant here. |

Two lookahead rules keep these unambiguous:

- A `.` only opens a literal when a **digit** follows it. In `1..2` the next
  character is another dot, so `..` still lexes as concat, and `1.foo` still
  lexes as member access.
- `f` is only a suffix when **no identifier character** follows it, so `1foo`
  stays `1` then `foo` rather than `1f` then `oo`.

The suffix is part of the token's span (for diagnostics) but not of the text
handed to `parse`. `saule-fmt` normalises both shorthands to the canonical
form — `.5` prints as `0.5`, `1f` as `1.0`.

## Place in the pipeline

```text
[lexer] → parser → semantic → typeck → interpreter
```

Depends on `saule-ast` (for `Spanned`). Errors are reported as
`LexerError`, a `miette`-aware diagnostic.
