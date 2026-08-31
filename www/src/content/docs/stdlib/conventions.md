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
- **Multi-return is a tuple.** Functions like `String.find` and `Os.capture`
  return `(integer?, integer?)` / `(integer, string, string)`. Destructure
  with `local s, e = ...` or pattern-match.
- **Conversion is a cast, not a call.** `as` converts between `integer` and
  `float`, renders either (and `boolean`) as a `string`, and parses a
  `string` back — and on an `any` it is the checked type test instead. It
  refuses any pair with no obvious answer rather than inventing one.
- **Numeric flavour never mixes.** `integer` and `float` are separate types
  and nothing promotes between them silently — not the operators, and not the
  stdlib. `Math.*` is generic over the flavour (`<N>`), so a call takes either
  and answers in kind, but one call may not take both.
- **No exceptions for "expected" failures.** Filesystem helpers return
  `boolean` rather than throwing. Use `throw` / `try` / `catch` for genuinely
  exceptional cases — see [README §Error Handling](/saule/language/error-handling/).
  The exception is `Os.list`, which throws; see
  [Failure conventions](/saule/stdlib/os/#failure-conventions).
- **Mutate with `Table`, transform with `Iter`.** `Table.*` writes to the
  table it is given or answers a question about it; `Iter.*` builds a new
  sequence and never writes. Where both could claim a name they are named
  apart — `Table.reverse` / `Iter.reverse`, `Table.indexOf` /
  `Iter.findIndex`.

Anything not in this reference is not in the toolchain. Libraries — JSON among
them — ship as ordinary Saule packages: a git repo with a `saule.config`,
added to `dependencies:`. See
[README §Importing from a Dependency](/saule/language/imports-and-file-structure/#importing-from-a-dependency).
