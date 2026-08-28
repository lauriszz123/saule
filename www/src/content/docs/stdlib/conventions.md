---
title: "Conventions"
description: "A handful of patterns repeat across the stdlib:"
sidebar:
  order: 8
---

<!-- Generated from DOCS.md by `npm run sync-docs`. Edit that file, not this one. -->

A handful of patterns repeat across the stdlib:

- **1-based indexing everywhere.** `String.sub`, `String.iter`, `Table.*`,
  array literals — all 1-based. Negative indices count from the end where
  it makes sense.
- **`?` means "may not happen".** Lookups that can miss (`String.find`,
  `Io.open`, `Os.getenv`) return nullable types so the typechecker forces
  you to handle the absence with `?.`, `??`, `!`, or a `match`.
- **Multi-return is a tuple.** Functions like `String.find` return
  `(integer?, integer?)`. Destructure with `local s, e = ...` or pattern-match.
- **Conversion is a cast, not a call.** `as` converts between `integer` and
  `float`, renders either (and `boolean`) as a `string`, and parses a
  `string` back — and on an `any` it is the checked type test instead. It
  refuses any pair with no obvious answer rather than inventing one.
- **No exceptions for "expected" failures.** Filesystem helpers return
  `boolean` rather than throwing. Use `throw` / `try` / `catch` for genuinely
  exceptional cases — see [README §Error Handling](/saule/language/error-handling/).
