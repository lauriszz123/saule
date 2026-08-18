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

## The trap this directory has already fallen into twice

**The harness gates on exit status, so a fixture that fails for the wrong
reason passes.** Two did, silently, for a long time:

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
span" is.

A fixture whose message is the generic `cannot determine the type of this
expression` is a signal, not a pass — it usually means the precise check the
fixture is named for does not exist yet. `match_variant_arity_mismatch.sau`
is the current example, and it is recorded as a gap in `VM_TASKS.md`.

## Two are exempt from the engine diff

`SAULE_DIFF=1 ./run_tests.sh` compares tree-walker and VM output character
for character. `stack_overflow_recursion.sau` and
`stack_overflow_reentrant.sau` are exempted in `diff_exempt()` because the
two engines deliberately name different limits (VM_DESIGN.md §6.4). Both
still *report*, which is what those fixtures pin. The exemption count is
printed on every run so the list cannot grow unnoticed.
