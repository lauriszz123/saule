---
title: "Coroutines"
description: "A six-part tour of coroutines: lazy stream combinators over infinite sources, a tokenizer that yields from two frames below the coroutine body, a recursive-descent parser that pulls one token at a time, and a cooperative scheduler whose tasks interleave on one thread — one of which fails without taking the others with it. Shows Coroutine<T> and Yielder<T>, where T is learned from, stackful yielding through ordinary function calls, laziness that is counted rather than claimed, and the dynamic two-way form."
sidebar:
  order: 9
  badge:
    text: "Advanced"
    variant: caution
---

<!-- Generated from examples/coroutines by `npm run sync-docs`. Edit the example, not this file. -->

A six-part tour of coroutines: lazy stream combinators over infinite sources, a tokenizer that yields from two frames below the coroutine body, a recursive-descent parser that pulls one token at a time, and a cooperative scheduler whose tasks interleave on one thread — one of which fails without taking the others with it. Shows `Coroutine<T>` and `Yielder<T>`, where `T` is learned from, stackful yielding through ordinary function calls, laziness that is counted rather than claimed, and the dynamic two-way form.

[Browse this example on GitHub](https://github.com/lauriszz123/saule/tree/main/examples/coroutines)

## Run it

```sh
git clone https://github.com/lauriszz123/saule.git
cd saule/examples/coroutines
saule run
```

## `saule.config`

```
name: "coroutines"
version: "0.1.0"
entry: "src/main.sau"
src_dirs: ["src"]
min_saule_version: "26.1"
```

## `src/stream.sau`

```saule title="src/stream.sau"
-- Lazy streams, built out of coroutines.
--
-- A `Coroutine<T>` already *is* a lazy sequence: nothing in the body runs
-- until something resumes it, and `for v in src do` resumes it once per
-- value. So every combinator below is just a coroutine that loops over
-- another one — about six lines each, with no iterator protocol to implement
-- and no state machine to unroll by hand.
--
-- Because each stage only runs when the stage after it asks for a value, a
-- pipeline over an infinite source is finite work: `take` stops resuming, so
-- the stages above it stop producing.

export class Stream
	-- Every value of a table, in order.
	static fn of<T>(items: table<T>) -> Coroutine<T>
		return Coroutine.create(fn(co)
			for item in items do
				co.yield(item)
			end
		end)
	end

	-- Infinite: 1, 2, 3, … The coroutine never finishes, which is fine —
	-- nothing resumes it once the consumer stops.
	static fn naturals(start: integer) -> Coroutine<integer>
		return Coroutine.create(fn(co)
			local n: integer = start
			while true do
				co.yield(n)
				n = n + 1
			end
		end)
	end

	-- Infinite too, and the kind of generator that reads worst as a class:
	-- the two locals below would have to become fields just to survive
	-- between calls.
	static fn fibonacci() -> Coroutine<integer>
		return Coroutine.create(fn(co)
			local a: integer = 0
			local b: integer = 1
			while true do
				co.yield(b)
				local next: integer = a + b
				a = b
				b = next
			end
		end)
	end

	-- `T` comes from the source, `U` from the mapper's return type, and the
	-- yielder in the body is a `Yielder<U>` because that is what the
	-- declared return type promises.
	static fn map<T, U>(src: Coroutine<T>, f: fn(T) -> U) -> Coroutine<U>
		return Coroutine.create(fn(co)
			for value in src do
				co.yield(f(value))
			end
		end)
	end

	static fn filter<T>(src: Coroutine<T>, keep: fn(T) -> boolean) -> Coroutine<T>
		return Coroutine.create(fn(co)
			for value in src do
				if keep(value) then
					co.yield(value)
				end
			end
		end)
	end

	-- The first `n` values, and not one more: the loop `break`s, which leaves
	-- `src` suspended exactly where it was rather than draining it.
	static fn take<T>(src: Coroutine<T>, n: integer) -> Coroutine<T>
		return Coroutine.create(fn(co)
			if n <= 0 then
				return
			end

			local taken: integer = 0
			for value in src do
				co.yield(value)
				taken = taken + 1
				if taken == n then
					break
				end
			end
		end)
	end

	-- Groups of `size`, with a short final group. A coroutine can hold a
	-- half-built batch across a yield, so nothing has to be flushed by the
	-- caller afterwards.
	static fn chunk<T>(src: Coroutine<T>, size: integer) -> Coroutine<table<T>>
		return Coroutine.create(fn(co)
			local batch: table<T> = {}
			for value in src do
				batch[#batch + 1] = value
				if #batch == size then
					co.yield(batch)
					batch = {}
				end
			end

			if #batch > 0 then
				co.yield(batch)
			end
		end)
	end

	-- Two sources, one value from each in turn, until both are done. This is
	-- the combinator that wants `resume()` rather than `for … in`: it has to
	-- interleave two sequences, so it drives each one by hand and reads the
	-- `T?` to know when that side has finished.
	static fn interleave<T>(left: Coroutine<T>, right: Coroutine<T>) -> Coroutine<T>
		return Coroutine.create(fn(co)
			while not (left.isDone() and right.isDone()) do
				local a: T? = left.resume()
				if a != nil then
					co.yield(a!)
				end

				local b: T? = right.resume()
				if b != nil then
					co.yield(b!)
				end
			end
		end)
	end

	-- Terminal operations: these drain the stream instead of returning one.
	static fn collect<T>(src: Coroutine<T>) -> table<T>
		local out: table<T> = {}
		for value in src do
			out[#out + 1] = value
		end
		return out
	end

	static fn reduce<T, A>(src: Coroutine<T>, init: A, f: fn(A, T) -> A) -> A
		local acc: A = init
		for value in src do
			acc = f(acc, value)
		end
		return acc
	end
end
```

## `src/lexer.sau`

```saule title="src/lexer.sau"
-- A tokenizer as a coroutine, to show off the *stackful* half of the model.
--
-- The classic shape for a hand-written lexer is a class with an index field:
-- `next()` picks up where the last call left off, so the position has to live
-- outside the function that uses it. A coroutine keeps it in a local instead —
-- `scan` below is written as one straight-line loop, as if it were allowed to
-- run to completion, and every `co.yield` hands a token out and freezes the
-- loop mid-stride.
--
-- What makes that work is that the yielder is an ordinary value. `scan`,
-- `number` and `name` are plain static functions called from inside the
-- coroutine body; each takes the `Yielder<Token>` and yields from one or two
-- frames below it. The whole call stack between the yield and the resume is
-- suspended along with it, which is why no state machine has to be unrolled by
-- hand.

export enum Kind
	Number = "number"
	Name = "name"
	Operator = "op"
	Open = "open"
	Close = "close"
end

export class Token implements OpToString
	kind: Kind = Kind.Number
	text: string = ""
	-- The 1-based column the token started at, so an error can point at it.
	at: integer = 0

	fn init(kind: Kind, text: string, at: integer)
		self.kind = kind
		self.text = text
		self.at = at
	end

	fn isOperator(op: string) -> boolean
		return self.kind == Kind.Operator and self.text == op
	end

	fn toString() -> string
		return String.format("%s:%s", self.kind.value, self.text)
	end
end

export class Lexer
	-- One token per resume. Nothing is scanned until something asks, and the
	-- caller can stop asking half-way through — see `Main.showLaziness`.
	static fn tokens(source: string) -> Coroutine<Token>
		return Coroutine.create(fn(co)
			Lexer.scan(co, source)
		end)
	end

	-- Yielding from a function the coroutine body merely *called*. `i` is an
	-- ordinary local that survives every suspension, which is the whole
	-- reason this reads like a loop rather than a resumable object.
	static fn scan(co: Yielder<Token>, source: string)
		local i: integer = 1
		local len: integer = String.len(source)

		while i <= len do
			local c: string = String.sub(source, i, i)

			if c == " " or c == "\t" then
				i = i + 1
			elseif Lexer.isDigit(c) then
				i = Lexer.number(co, source, i)
			elseif Lexer.isAlpha(c) then
				i = Lexer.name(co, source, i)
			elseif c == "(" then
				co.yield(Token(Kind.Open, c, i))
				i = i + 1
			elseif c == ")" then
				co.yield(Token(Kind.Close, c, i))
				i = i + 1
			elseif String.contains("+-*/%", c) then
				co.yield(Token(Kind.Operator, c, i))
				i = i + 1
			else
				-- A throw the coroutine does not catch does not end the
				-- program: it kills this coroutine, `resume()` answers `nil`,
				-- and `error()` carries this message. Every token yielded
				-- before it was still delivered.
				throw String.format("unexpected '%s' at column %d", c, i)
			end
		end
	end

	-- Two frames below the coroutine body, and still yielding. The return
	-- value is where scanning should carry on from.
	static fn number(co: Yielder<Token>, source: string, start: integer) -> integer
		local i: integer = start
		local len: integer = String.len(source)

		while i <= len and Lexer.isDigit(String.sub(source, i, i)) do
			i = i + 1
		end

		co.yield(Token(Kind.Number, String.sub(source, start, i - 1), start))
		return i
	end

	static fn name(co: Yielder<Token>, source: string, start: integer) -> integer
		local i: integer = start
		local len: integer = String.len(source)

		while i <= len and Lexer.isAlpha(String.sub(source, i, i)) do
			i = i + 1
		end

		co.yield(Token(Kind.Name, String.sub(source, start, i - 1), start))
		return i
	end

	static fn isDigit(c: string) -> boolean
		return c >= "0" and c <= "9"
	end

	static fn isAlpha(c: string) -> boolean
		local lower: string = String.lower(c)
		return lower >= "a" and lower <= "z"
	end
end
```

## `src/calc.sau`

```saule title="src/calc.sau"
-- A recursive-descent calculator fed by the lexer coroutine.
--
-- This is the other half of the generator story. The lexer is a *producer*
-- that wants to be written as a loop; the parser is a *consumer* that wants to
-- ask for one token at a time and look one ahead. A coroutine serves both
-- without either knowing about the other: the parser pulls with `resume()`,
-- and the lexer's loop advances exactly far enough to answer.
--
-- Nothing buffers the whole token list. `Calc.eval("1 + 2 + <megabyte>")`
-- would hold one token at a time however long the input is.

from lexer import Lexer, Token, Kind

-- One token of lookahead over a `Coroutine<Token>`.
--
-- `resume()` answers a `Token?`, and `nil` means *no more* — either the lexer
-- finished or it failed. `failure()` tells those apart: a dead coroutine's
-- `error()` is `nil` when it merely ran out.
export class TokenSource
	local src: Coroutine<Token>
	local ahead: Token?

	fn init(source: string)
		self.src = Lexer.tokens(source)
		self.ahead = self.src.resume()
	end

	fn peek() -> Token?
		return self.ahead
	end

	fn take() -> Token?
		local current: Token? = self.ahead
		self.ahead = self.src.resume()
		return current
	end

	-- The lexer's own complaint, if it died mid-stream.
	fn failure() -> string?
		return self.src.error()
	end
end

export class Calc
	local tokens: TokenSource
	local env: table<string, float>?

	fn init(source: string, env: table<string, float>?)
		self.tokens = TokenSource(source)
		self.env = env
	end

	-- expr := term (('+' | '-') term)*
	static fn eval(source: string, env: table<string, float>? = nil) -> float
		local calc: Calc = Calc(source, env)
		local value: float = calc.expr()

		-- Anything left over is a syntax error — and so is a lexer that died
		-- part-way, which shows up as a `nil` token with a `failure()`.
		local leftover: Token? = calc.tokens.peek()
		if leftover != nil then
			throw String.format("unexpected %s at column %d", tostring(leftover!), leftover!.at)
		end

		local lexFailure: string? = calc.tokens.failure()
		if lexFailure != nil then
			throw lexFailure!
		end

		return value
	end

	fn expr() -> float
		local left: float = self.term()

		while true do
			local op: Token? = self.tokens.peek()
			if op == nil then
				break
			end

			if op!.isOperator("+") then
				self.tokens.take()
				left = left + self.term()
			elseif op!.isOperator("-") then
				self.tokens.take()
				left = left - self.term()
			else
				break
			end
		end

		return left
	end

	-- term := atom (('*' | '/' | '%') atom)*
	fn term() -> float
		local left: float = self.atom()

		while true do
			local op: Token? = self.tokens.peek()
			if op == nil then
				break
			end

			if op!.isOperator("*") then
				self.tokens.take()
				left = left * self.atom()
			elseif op!.isOperator("/") then
				self.tokens.take()
				local divisor: float = self.atom()
				if divisor == 0f then
					throw "division by zero"
				end
				left = left / divisor
			elseif op!.isOperator("%") then
				self.tokens.take()
				left = Math.fmod(left, self.atom())
			else
				break
			end
		end

		return left
	end

	-- atom := number | name | '(' expr ')' | '-' atom
	fn atom() -> float
		local token: Token? = self.tokens.take()
		if token == nil then
			local why: string? = self.tokens.failure()
			throw why ?? "unexpected end of input"
		end

		local t: Token = token!

		if t.kind == Kind.Number then
			local digits: integer? = t.text as integer
			return (digits ?? 0) as float
		end

		if t.kind == Kind.Name then
			-- `?[` is the safe index: no env, no lookup, no crash.
			local bound: float? = self.env?[t.text]
			if bound == nil then
				throw String.format("unknown name '%s'", t.text)
			end
			return bound!
		end

		if t.isOperator("-") then
			return -self.atom()
		end

		if t.kind == Kind.Open then
			local inner: float = self.expr()
			local close: Token? = self.tokens.take()
			if close == nil or close!.kind != Kind.Close then
				throw String.format("missing ')' for the '(' at column %d", t.at)
			end
			return inner
		end

		throw String.format("unexpected %s at column %d", tostring(t), t.at)
	end
end
```

## `src/scheduler.sau`

```saule title="src/scheduler.sau"
-- Cooperative multitasking, in about seventy lines.
--
-- A coroutine is a task that decides for itself when to give the CPU back.
-- There are no threads here and nothing runs in parallel: `resume()` transfers
-- control into a task, and the task's next `yield` transfers it back. That is
-- enough to interleave as many jobs as you like — a loading screen, a game's
-- actors, a batch of downloads — with no locks, because between two yields a
-- task has the whole program to itself.
--
-- The scheduler never learns what a task does. It resumes it, reports what it
-- yielded, and comes back on the next tick.

export class Task
	name: string = ""
	local job: Coroutine<string>
	local slices: integer = 0

	fn init(name: string, job: Coroutine<string>)
		self.name = name
		self.job = job
	end

	fn isDone() -> boolean
		return self.job.isDone()
	end

	-- One slice of work. How long a slice is, is the task's own business:
	-- `resume` runs it until *it* decides to yield.
	fn step(tick: integer) -> nil
		local note: string? = self.job.resume()

		if note != nil then
			self.slices = self.slices + 1
			printf("  tick %d  %-9s %s\n", tick, self.name, note!)
			return
		end

		-- `nil` means the task is over — but over how? `error()` tells a
		-- finish from a crash, and a crash is contained: the coroutine dies,
		-- the program does not, and the other tasks keep their turns.
		local failure: string? = self.job.error()
		if failure != nil then
			printf("  tick %d  %-9s failed: %s\n", tick, self.name, failure!)
		else
			printf("  tick %d  %-9s done after %d slices\n", tick, self.name, self.slices)
		end
	end
end

export class Scheduler
	local tasks: table<Task>

	fn init()
		self.tasks = {}
	end

	-- The parameter type *is* the coroutine contract: `fn(Yielder<string>)`.
	-- Callers hand over a lambda and never see a `Coroutine` at all.
	fn spawn(name: string, body: fn(Yielder<string>) -> nil) -> nil
		self.tasks[#self.tasks + 1] = Task(name, Coroutine.create(body))
	end

	fn alive() -> integer
		local count: integer = 0
		for task in self.tasks do
			if not task.isDone() then
				count = count + 1
			end
		end
		return count
	end

	-- Round-robin: every live task gets one slice per tick, until none is
	-- left. Swapping this for a priority queue or a frame budget would not
	-- touch a single task.
	fn run() -> integer
		local tick: integer = 0

		while self.alive() > 0 do
			tick = tick + 1
			for task in self.tasks do
				if not task.isDone() then
					task.step(tick)
				end
			end
		end

		return tick
	end
end
```

## `src/main.sau`

```saule title="src/main.sau"
-- A guided tour of Saule's coroutines. Run it and read along: each section
-- prints what it does, and the module it leans on is named next to it.
--
--   1. Generators            — a function that pauses and carries on
--   2. Lazy pipelines        — stream.sau: combinators that are coroutines
--   3. Stackful tokenizing   — lexer.sau: yielding from two frames down
--   4. Pull-based parsing    — calc.sau: a consumer that asks for one token
--   5. Cooperative tasks     — scheduler.sau: interleaving, and failure
--   6. Lifecycle and the dynamic form
--
-- What a coroutine is: a function that can stop in the middle, hand a value
-- out, and later continue from exactly that point with every local and its
-- whole call stack intact. One thread, no locks — `resume` transfers control
-- in, `yield` transfers it back out.

from stream import Stream
from lexer import Lexer, Token, Kind
from calc import Calc
from scheduler import Scheduler

class Main
	static fn main()
		Main.generators()
		Main.pipelines()
		Main.tokenizing()
		Main.parsing()
		Main.tasks()
		Main.lifecycle()

		println("")
		println("Read the modules in this order: stream.sau, lexer.sau,")
		println("calc.sau, scheduler.sau.")
	end

	-- 1 ----------------------------------------------------------------
	static fn generators()
		Main.section(1, "Generators")

		-- The declared type of the local tells the body what it yields, so
		-- `co` needs no annotation: it is a `Yielder<string>`.
		local greet: Coroutine<string> = Coroutine.create(fn(co)
			co.yield("hello")
			co.yield("from")
			co.yield("a coroutine")
		end)

		-- Creating runs nothing at all. The first `resume` starts the body.
		printf("before the first resume: %s\n", greet.status())
		printf("resume -> %s\n", greet.resume() ?? "")
		printf("resume -> %s\n", greet.resume() ?? "")
		printf("still %s, parked at the second yield\n", greet.status())

		-- A coroutine is an `Iterable<T>`, so `for … in` drains it and the
		-- loop variable is a `T` — here, a `string`.
		for word in greet do
			printf("the loop picked up the rest: %s\n", word)
		end

		-- Past the end it answers `nil` for ever, which is what lets a loop
		-- drain a generator without asking whether it is empty first.
		printf("after the body returned: %s, resume -> %s\n", greet.status(), greet.resume() ?? "nil")
	end

	-- 2 ----------------------------------------------------------------
	static fn pipelines()
		Main.section(2, "Lazy pipelines  (stream.sau)")

		-- `Stream.naturals` never finishes. That is safe, because every stage
		-- only runs when the stage after it asks: `take` stops resuming, so
		-- `filter` stops asking, so the source stops counting.
		local firstEvens: table<integer> = Stream.collect(
			Stream.take(Stream.filter(Stream.naturals(1), n => n % 2 == 0), 6)
		)
		printf("first six even naturals: %s\n", String.join(", ", firstEvens))

		-- Proof that nothing ran ahead of demand. The counter is an ordinary
		-- captured local, bumped once per value the source actually produced.
		local produced: integer = 0
		local counted: Coroutine<integer> = Stream.map(Stream.naturals(1), fn(n)
			produced = produced + 1
			return n
		end)
		local three: table<integer> = Stream.collect(Stream.take(counted, 3))
		printf(
			"took %s from an infinite source, and it produced %d values\n",
			String.join(", ", three),
			produced
		)

		-- A generator is the shape fibonacci wants: `a` and `b` are locals
		-- that happen to survive between resumes, not fields on an object.
		local fib: table<integer> = Stream.collect(Stream.take(Stream.fibonacci(), 12))
		printf("fibonacci: %s\n", String.join(", ", fib))

		-- `reduce` and `chunk` are terminal and transforming stages over the
		-- same `Coroutine<T>` — there is no second sequence abstraction here.
		local sum: integer = Stream.reduce(Stream.take(Stream.naturals(1), 100), 0, (acc, n) => acc + n)
		printf("sum of 1..100: %d\n", sum)

		local rows: table<string> = {}
		for batch in Stream.chunk(Stream.take(Stream.fibonacci(), 10), 4) do
			rows[#rows + 1] = "[" .. String.join(" ", batch) .. "]"
		end
		printf("in chunks of four: %s\n", String.join(" ", rows))

		-- Interleaving needs two sources driven by hand, which is what
		-- `resume()`'s `T?` is for: `nil` marks the side that ran out.
		local mixed: table<string> = Stream.collect(
			Stream.interleave(Stream.of({"tick", "tick", "tick"}), Stream.of({"tock", "tock"}))
		)
		printf("interleaved: %s\n", String.join(" ", mixed))
	end

	-- 3 ----------------------------------------------------------------
	static fn tokenizing()
		Main.section(3, "Stackful tokenizing  (lexer.sau)")

		-- `Lexer.scan` is a plain function called *inside* the coroutine, and
		-- `Lexer.number` is a plain function called inside that. Both yield.
		-- The suspended stack holds all three frames.
		local shown: table<string> = {}
		for token in Lexer.tokens("12 + rate * (7 - 3)") do
			shown[#shown + 1] = tostring(token)
		end
		printf("%s\n", String.join("  ", shown))

		-- Stopping early really does stop the work. `break` leaves the lexer
		-- suspended on the character after the token it just handed over.
		local stream: Coroutine<Token> = Lexer.tokens("1 + 2 + 3 + 4 + 5")
		local before: integer = 0
		for token in stream do
			before = before + 1
			if token.kind == Kind.Operator then
				break
			end
		end
		local resumed: Token? = stream.resume()
		printf(
			"read %d tokens, then broke out; the lexer is %s and next is %s\n",
			before,
			stream.status(),
			resumed?.text ?? "nothing"
		)

		-- A `throw` the coroutine does not catch kills *it*, not the program.
		-- Everything it yielded first was still delivered.
		local broken: Coroutine<Token> = Lexer.tokens("2 + $")
		local delivered: integer = 0
		while true do
			local token: Token? = broken.resume()
			if token == nil then
				break
			end
			delivered = delivered + 1
		end
		printf(
			"bad input: %d tokens delivered, then %s — error() says: %s\n",
			delivered,
			broken.status(),
			broken.error() ?? ""
		)
	end

	-- 4 ----------------------------------------------------------------
	static fn parsing()
		Main.section(4, "Pull-based parsing  (calc.sau)")

		-- The parser asks the lexer coroutine for one token at a time and
		-- keeps one in hand to look ahead. Neither side buffers the input.
		local env: table<string, float> = {}
		env["rate"] = 2.5
		env["hours"] = 8f

		local expressions: table<string> = {
			"2 + 3 * (4 - 1)",
			"10 / 4",
			"-3 + 20 % 7",
			"rate * hours",
			"2 * (rate + hours) - 1",
		}

		for source in expressions do
			printf("  %-22s = %s\n", source, tostring(Calc.eval(source, env)))
		end

		-- Three kinds of bad input, all reported the same way: the lexer's
		-- own failure travels out through the parser as a thrown string.
		local broken: table<string> = {"2 + $", "2 + (3", "1 / 0", "2 + missing"}
		for source in broken do
			try
				Calc.eval(source, env)
			catch e: string
				printf("  %-22s ! %s\n", source, e)
			end
		end
	end

	-- 5 ----------------------------------------------------------------
	static fn tasks()
		Main.section(5, "Cooperative tasks  (scheduler.sau)")

		local sched: Scheduler = Scheduler()

		-- Each task is a lambda that takes a yielder. Yielding is how a task
		-- says "this is a good place to interrupt me".
		sched.spawn("download", fn(co)
			for percent = 25, 100, 25 do
				co.yield("fetching " .. percent .. "%")
			end
		end)

		sched.spawn("render", fn(co)
			co.yield("laying out")
			co.yield("painting")
		end)

		-- A task may drive a coroutine of its own. While the inner one runs,
		-- this one's status is "normal" — resumed, but waiting on a child.
		sched.spawn("report", fn(co)
			for batch in Stream.chunk(Stream.take(Stream.fibonacci(), 6), 3) do
				co.yield("row " .. String.join("+", batch))
			end
		end)

		-- A task that fails takes nothing with it: its coroutine dies, the
		-- scheduler reads `error()`, and the other tasks keep their turns.
		sched.spawn("sensor", fn(co)
			co.yield("reading 21.4C")
			throw "sensor offline"
		end)

		-- `yield` inside a `try` resumes *inside the same try*, so a task can
		-- pause mid-retry and still catch its own failure afterwards.
		sched.spawn("retry", fn(co)
			for attempt = 1, 3 do
				try
					co.yield("attempt " .. attempt)
					if attempt < 3 then
						throw "timed out"
					end
					co.yield("connected")
				catch e: string
					co.yield("recovered from: " .. e)
				end
			end
		end)

		local ticks: integer = sched.run()
		printf("five tasks interleaved over %d ticks, on one thread\n", ticks)
	end

	-- 6 ----------------------------------------------------------------
	static fn lifecycle()
		Main.section(6, "Lifecycle and the dynamic form")

		-- "normal" is the status of a coroutine that resumed another one and
		-- is waiting on it. Only the inner one can see it, so the inner one
		-- reports it.
		local outer: Coroutine<string>? = nil
		local inner: Coroutine<string> = Coroutine.create(fn(co)
			co.yield("while I run, the one that resumed me is " .. (outer?.status() ?? "?"))
		end)
		outer = Coroutine.create(fn(co)
			for note in inner do
				co.yield(note)
			end
		end)
		printf("%s\n", outer!.resume() ?? "")

		-- A generator that might fail wants `resume()`, not `for … in`.
		-- Iterating one **raises** when it fails — a loop has no return value
		-- to report a failure in, and ending the loop quietly would make a
		-- generator that crashed look like one that finished. Driving it by
		-- hand turns that into a `nil` and a message, which is what the
		-- tokenizer in part 3 and the scheduler in part 5 both do.
		local failing: Coroutine<integer> = Coroutine.create(fn(co)
			co.yield(1)
			throw "halfway"
		end)
		while true do
			local value: integer? = failing.resume()
			if value == nil then
				break
			end
			printf("drained %d\n", value!)
		end
		printf("it stopped because: %s\n", failing.error() ?? "it simply ended")

		-- The typed contract is `fn(Yielder<T>) -> nil`: values travel one
		-- way, out. The runtime can do more than that — `resume(x)` becoming
		-- the result of the `yield` it wakes, and the body's return value
		-- reaching the last `resume` — and `any` is how to ask for it, with
		-- the checking moved to runtime like everything else `any`.
		local body: any = fn(co: any, start: integer) -> string
			local said: any = co.yield(start * 2)
			local andThen: any = co.yield("you said " .. tostring(said))
			return "signing off with " .. tostring(andThen)
		end

		local chat: any = Coroutine.create(body)
		printf("resume(21)      -> %s\n", tostring(chat.resume(21)))
		printf("resume(\"hi\")    -> %s\n", tostring(chat.resume("hi")))
		printf("resume(7)       -> %s\n", tostring(chat.resume(7)))
	end

	-- A heading, so the output can be read top to bottom.
	static fn section(number: integer, title: string)
		println("")
		printf("%d. %s\n", number, title)
		println(String.rep("-", 62))
	end
end
```
