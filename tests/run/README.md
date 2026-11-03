# `tests/run/` — the behaviour corpus

**Every `.sau` file under this directory must run and exit 0.** `run_tests.sh`
gates on that, and on nothing else: what a fixture *prints* is not compared
against anything. Read a fixture's output yourself when you change one — and
see [`../errors/README.md`](../errors/README.md) for what this gate has missed
before.

```
tests/run/<area>/*.sau      must run and exit 0
tests/errors/<area>/*.sau   must fail
```

Nothing but fixtures lives here: one `.sau` per test, grouped by area.

## Areas

The same split as `tests/errors/`, so a feature's passing and failing
fixtures sit at mirrored paths:

| Area | Holds |
| --- | --- |
| `basics` | The smoke tests — `hello_world`, `fibonacci`, enums |
| `classes` | Inheritance, interfaces, statics, fields, overrides, visibility, operator overloads |
| `control` | Loops, `break` / `continue`, `match`, narrowing |
| `exceptions` | `try` / `catch` / `finally` / `throw` |
| `functions` | Lambdas, closures, variadics, defaults, named arguments, trailing blocks, pipelines |
| `generics` | Type parameters, bounds, generic classes / interfaces / enums |
| `modules` | Module variables, imports, single-file scripts |
| `nullability` | `?.`, `?[ ]`, `??`, `!` and what they answer |
| `stdlib` | `String`, `Table`, `Math`, `Io`, `Os`, `Bytes`, `Iter`, `printf`, `Os.spawn` |
| `syntax` | Literals, escapes, operators, precedence, casts |
| `tables` | Table types, keys, element variance |

Put a new fixture in the area its *subject* belongs to. Where a fixture spans
two, pick the one a reader would look in first.

## Seeing the scope

`./run_tests.sh --list` prints the whole corpus instead of running it — every
fixture grouped by area, with the first sentence of its own leading comment as
the description:

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

## Adding one

1. Write `tests/run/<area>/<name>.sau`. Lead with a comment saying what it
   pins and why that is worth pinning — the corpus is read far more often
   than it is written, and that comment is what `--list` shows.
2. Run it and **read the output**. Nothing else will.

One fixture is skipped on Windows, listed in `run_tests.sh` itself so the list
cannot grow unnoticed: `stdlib/spawn.sau` needs a Unix userland, and
`crates/saule-cli/tests/spawn.rs` covers the same ground everywhere.
