# coroutines

A guided tour of coroutines, in six parts: generators, lazy stream
combinators, a stackful tokenizer, a parser that pulls from it, a cooperative
scheduler, and the lifecycle rules.

```sh
saule run
```

A coroutine is a function that can stop in the middle — **yield** a value —
and later carry on from exactly that point, with every local and its whole
call stack intact. One thread, no locks: `resume` transfers control in,
`yield` transfers it back out.

## What it shows

| Part | Module | The point |
|---|---|---|
| 1 | `src/main.sau` | `resume`, `for … in`, and the four statuses |
| 2 | `src/stream.sau` | Combinators over `Coroutine<T>` — `map`, `filter`, `take`, `chunk`, `interleave` — and infinite sources that cost only what you take |
| 3 | `src/lexer.sau` | Stackful yielding: `scan` and `number` are ordinary functions that yield from two frames below the body |
| 4 | `src/calc.sau` | The consumer half — a recursive-descent parser that pulls one token at a time and looks one ahead |
| 5 | `src/scheduler.sau` | Tasks as coroutines: round-robin slices, a task that fails without taking the others down, and `yield` inside a `try` |
| 6 | `src/main.sau` | `"normal"` status, draining a generator that fails, and the dynamic two-way form |

## Three things worth noticing

**A combinator is six lines.** `Stream.map` is a coroutine that loops over
another coroutine and yields what the mapper returns. There is no iterator
protocol to implement and no state machine to unroll, because the loop *is*
the state. The same goes for `filter`, `take` and `chunk` — and `chunk` holds
a half-built batch across a yield, so nothing needs flushing afterwards.

**Laziness is not a claim, it is counted.** Part 2 wraps an infinite source
in a mapper that bumps a captured counter, takes three values, and prints how
many the source actually produced: three. `take` stops resuming, so `filter`
stops asking, so the source stops counting.

**A failing coroutine is reported, not fatal.** `src/lexer.sau` throws on an
unexpected character. The coroutine dies, `resume()` answers `nil`, `error()`
carries the message — and every token yielded before it was still delivered.
That is what lets the scheduler in part 5 lose a task and keep running.

One exception, and part 6 shows the way around it: a coroutine that fails
*inside* a `for … in` **raises**, because a loop has no return value to
report a failure in. Drive a generator that might fail with `resume()`.

## Files

- `src/stream.sau` — lazy stream combinators, all of them coroutines
- `src/lexer.sau` — a tokenizer that yields from two frames below the body
- `src/calc.sau` — one token of lookahead, and a recursive-descent calculator
- `src/scheduler.sau` — round-robin cooperative tasks
- `src/main.sau` — the six-part tour, and what each part prints
