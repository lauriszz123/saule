# `tests/errors/` — the diagnostic corpus

**Every `.sau` file under this directory is a deliberate error.** Each one
exists to pin a specific diagnostic, and `run_tests.sh` requires all of them
to **fail**:

```
tests/run/<area>/*.sau      must run and exit 0
tests/errors/<area>/*.sau   must fail
```

**Only the failure is checked, not the diagnostic.** The recorded outputs
this suite compared against were removed deliberately, so a fixture that
fails for the wrong reason now passes — which is the trap described at the
bottom of this file, reopened on purpose. The fixture's name and its leading
comment are the only statement of what it should say; run it and read the
message when you touch one.

Both trees are split into the same areas, so a feature's passing and failing
fixtures sit at mirrored paths: `tests/run/generics/` and
`tests/errors/generics/`. The areas are `basics` (passing only), `classes`,
`control`, `exceptions`, `functions`, `generics`, `modules`, `nullability`,
`stdlib`, `syntax` and `tables`. Put a new fixture in the area its *subject*
belongs to rather than the one its error message mentions — a bad argument to
`Table.insert` is a `stdlib` fixture, not a `functions` one.

Most are compile-time errors — parser, `saule-semantic`, `saule-typeck` — but
not all. `throw_uncaught`, `io_use_after_close`, `force_unwrap_*`,
`table_insert_oob`, `pow_negative_exponent` and the two `stack_overflow_*`
fixtures are **runtime** errors, and are here for the same reason: the
message and its span are the behaviour being pinned.

Nothing but fixtures lives here: one `.sau` per test, grouped by area.

## Seeing the scope

`./run_tests.sh --list` prints the whole corpus instead of running it —
every fixture grouped by area, with the first sentence of its own leading
comment as the description:

```
$ ./run_tests.sh --list generics
run/generics (11)
  generic_bounds        `<T: Bound>` — what makes a type parameter usable ...
  generic_identity      Generic identity function.
  …

errors/generics (21)
  bound_not_satisfied   The call site's half of a bound: `integer` has no ...
  …
```

`--list <area>` narrows to one area, in both trees (`generics`) or in one
(`errors/generics`). The descriptions are read from the fixtures themselves
rather than kept in a table here, so they cannot go stale — which is also why
**every fixture should open with a comment saying what it pins**. One that
does not is listed with a `?`.

## The trap this directory fell into twice

**Before outputs were recorded, the harness gated on exit status alone, so a
fixture that failed for the wrong reason passed.** Two did, silently, for a
long time:

* `unknown_field.sau` was written with `constructor(label)`, which is not
  Saule syntax. It failed in the *parser* and never reached a member check —
  while its comment claimed the typechecker had no class registry, which had
  stopped being true. The language catches the real thing correctly
  (`no member ... on Box`); nothing was testing that it did.
* `io_use_after_close.sau` opened `/tmp/...`, which does not exist on
  Windows. `Io.open` returned nil, the `!` unwrapped it, and the run died on
  line 4 — never reaching `close()`, let alone the use after it.

So when adding or editing a fixture: **run it and read the message.** "It
fails" is not the assertion; "it fails with *this* diagnostic, at *this*
span" is — and with nothing recorded, you are the only thing checking it.

A fixture whose message is the generic `cannot determine the type of this
expression` is a signal, not a pass — it usually means the precise check the
fixture is named for does not exist yet. `match_variant_arity_mismatch.sau`
is the current example, and it is recorded as a gap in `VM_TASKS.md`.
