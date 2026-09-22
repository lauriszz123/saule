# `tests/ui/` — the diagnostic corpus

**Every `.sau` file in this directory is a deliberate error.** Each one exists
to pin a specific diagnostic, and `run_tests.sh` requires all of them to
**fail**:

```
tests/*.sau      must run and exit 0
tests/ui/*.sau   must fail — each one exists to pin a specific diagnostic
```

Most are compile-time errors — parser, `saule-semantic`, `saule-typeck` — but
not all. `throw_uncaught`, `io_use_after_close`, `force_unwrap_*`,
`table_insert_oob`, `pow_negative_exponent` and the two `stack_overflow_*`
fixtures are **runtime** errors, and are here for the same reason: the
message and its span are the behaviour being pinned.

Each fixture's full output is recorded in `expected/<name>.out`, and
`run_tests.sh` compares against it — so the diagnostic, not just the failure,
is what a fixture pins. `SAULE_BLESS=1 ./run_tests.sh` re-records every one;
read the diff before committing it.

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

So when adding or editing a fixture: **run it and read the message** before
recording it. "It fails" is not the assertion; "it fails with *this*
diagnostic, at *this* span" is — and the recording only pins whatever it was
given.

A fixture whose message is the generic `cannot determine the type of this
expression` is a signal, not a pass — it usually means the precise check the
fixture is named for does not exist yet. `match_variant_arity_mismatch.sau`
is the current example, and it is recorded as a gap in `VM_TASKS.md`.

## One is exempt from the output check

`stack_overflow_reentrant.sau` is exempted in `output_exempt()`: both
profiles report a stack overflow, but not the same one. A level of
re-entrant nesting (a comparator that sorts with itself) costs several times
more stack in a debug build, so it exhausts the thread's stack where a
release build reaches the 10,000-level count first — and the message, plus
how many `Table.sort: comparator failed:` wrappings surround it, differs
with it. That the run *reports* rather than dying is what the fixture pins,
and that is still checked. The exemption count is printed on every run so
the list cannot grow unnoticed.
