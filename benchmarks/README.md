# Benchmarks

Thirty-one small programs, each written twice: `sau/NAME.sau` and
`lua/NAME.lua`, line for line the same algorithm producing the same output.
Lua is the reference because Saule borrows its runtime model, so the two are
directly comparable and any gap is a gap in the implementation rather than in
the language.

```bash
python3 benchmarks/bench.py                    # release build vs lua
python3 benchmarks/bench.py check              # confirm outputs still agree
ONLY=coroutine,fib python3 benchmarks/bench.py # just these two
```

To compare two builds — which is what you want when judging a change — pass
both:

```bash
python3 benchmarks/bench.py new=./target/release/saule old=/tmp/saule-before
```

Lua 5.4 or newer. The twins use integer division (`//`) and integer-valued
arithmetic, which 5.1 and 5.2 do not have.

## Reading the results

**Compare ratios between engines in the same run, never seconds across runs.**
Wall-clock times on a laptop drift 20-40% between runs as clock speeds and
core assignment change — enough to invent or hide a 30% improvement. Each rep
therefore runs every engine back to back under the same conditions, and the
reported number is the minimum over `REPS` reps (default 7, override with the
environment variable).

Each section prints a **geometric mean** of its ratios, which is the average a
column of ratios has: a 4x and a 0.25x cancel to 1x, where an arithmetic mean
would call them 2.1x.

`startup` is the control: it measures process launch, parse and typecheck with
no work to do, and should stay level with Lua.

## What each one exercises

### Core mechanisms

One language primitive each, with as little else in the way as possible.

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

### Language features

The constructs Saule has that Lua does not, measured against what a Lua
programmer writes instead — metatables, `pcall`, an `and`/`or` chain, a table
with a `tag` field. These are the rows that say what a feature costs.

| Benchmark | Exercises |
| --- | --- |
| `coroutine` | `resume` / `yield` round trips, with nothing else in the body |
| `coro_pipe` | three chained generators — a coroutine resuming a coroutine |
| `coro_tree` | stackful yield: suspending from deep inside a recursion |
| `vecops` | operator overloading (`OpAdd`/`OpSub`/`OpMul`/`OpEq`) vs metamethods |
| `iface` | `interface` dispatch across classes with no common ancestor |
| `generic` | what a type parameter costs: `Stack<T>` and a generic function |
| `errors` | `throw` / `try` / two `catch` clauses vs `error` / `pcall` |
| `nullsafe` | `?.`, `?[ ]`, `??`, `!` down a three-deep path vs `and`/`or` |
| `match` | `match` arms that bind enum payloads, plus `when` guards |

### Standard library

The modules, exercised the way application code uses them: a whole job per
pass, not one function in a loop. Where Lua has no counterpart the twin writes
it out by hand, which is the honest comparison — the question is how fast the
job gets done in each language, not how fast two functions with the same name
are.

| Benchmark | Exercises |
| --- | --- |
| `stringlib` | `String`: trim, split, join, replace, pad, case, format |
| `pattern` | `String.match` / `gmatch` / `gsub` — the pattern engine |
| `tablelib` | `Table`: positional insert, splice, reverse, slice, search, join |
| `iterlib` | `Iter`: the whole combinator set, so lambda-call volume |
| `mathlib` | `Math`: transcendentals, rounding, the integer helpers |
| `bytes` | `Bytes`: byte-by-byte scanning, framing and hex over a buffer |

### Whole programs

The shape everyday code actually takes, where no single mechanism dominates.

| Benchmark | Exercises |
| --- | --- |
| `bintree` | allocation and pointer chasing through nullable fields |
| `matrix` | float arithmetic over nested tables |
| `entity` | a virtual call per element over a mixed-subclass list |
| `wordfreq` | string slicing, map update, then a comparator sort |
| `json` | a branchy recursive-descent scanner over 1MB of text |
| `interp` | a stack machine dispatched by `match` on an enum |

### Control

| Benchmark | Exercises |
| --- | --- |
| `startup` | process launch, parse, typecheck; no work |

## Adding one

Write both files, keep them computing the same printed value, and add the name
to the right group in `GROUPS` in `bench.py`. Then run `bench.py check` — a
benchmark whose two versions disagree is measuring two different programs.

Two things to get right, both of which have bitten this suite:

- **Keep the output exact.** Print integers, or floats through
  `String.format("%.6f", …)`. A bare float leaves the comparison at the mercy
  of two renderers agreeing on the last digit.
- **Stay inside the semantics the two languages share.** `%` on a negative
  operand, `#` or a pattern over non-ASCII text, and the iteration order of a
  map are all places where the twins are answering different questions.
  Sort keys before depending on them, keep `%` operands positive, and keep
  text ASCII.
