# Benchmarks

Ten small programs, each written twice: `sau/NAME.sau` and `lua/NAME.lua`,
line for line the same algorithm producing the same output. Lua is the
reference because Saule borrows its runtime model, so the two are directly
comparable and any gap is a gap in the implementation rather than in the
language.

```bash
python3 benchmarks/bench.py                    # release build vs lua, luajit
python3 benchmarks/bench.py check              # confirm outputs still agree
```

To compare two builds — which is what you want when judging a change — pass
both:

```bash
python3 benchmarks/bench.py new=./target/release/saule old=/tmp/saule-before
```

## Reading the results

**Compare ratios between engines in the same run, never seconds across runs.**
Wall-clock times on a laptop drift 20-40% between runs as clock speeds and
core assignment change — enough to invent or hide a 30% improvement. Each rep
therefore runs every engine back to back under the same conditions, and the
reported number is the minimum over `REPS` reps (default 7, override with the
environment variable).

`startup` is the control: it measures process launch, parse and typecheck with
no work to do, and should stay level with Lua.

## What each one exercises

| Benchmark | Exercises |
| --- | --- |
| `loop_arith` | numeric `for`, integer arithmetic, scope-local assignment |
| `fib` | recursive static-method calls — the call path, end to end |
| `array` | `Table.insert` and integer indexing over a 1M-element array |
| `map` | string-keyed table writes and reads, and string building |
| `oop` | instance method dispatch and field read/write |
| `mandel` | nested loops with a `while` inner loop, float arithmetic |
| `strings` | `..` concatenation and `String.len` |
| `closure` | calling a lambda through a local variable |
| `sort` | `Table.sort` with a Saule comparator — comparator call volume |
| `startup` | process launch, parse, typecheck; no work |

## Adding one

Write both files, keep them computing the same printed value, and add the name
to `BENCHES` in `bench.py`. Then run `bench.py check` — a benchmark whose two
versions disagree is measuring two different programs.
