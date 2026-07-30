---
title: "Brainfuck Interpreter"
description: "An interpreter for another language, in about 240 lines. Tables as a tape, a dispatch loop, and a program that takes its source file from Os.args()."
sidebar:
  order: 4
---

<!-- Generated from examples/bf by `npm run sync-docs`. Edit the example, not this file. -->

An interpreter for another language, in about 240 lines. Tables as a tape, a dispatch loop, and a program that takes its source file from `Os.args()`.

[Browse this example on GitHub](https://github.com/lauriszz123/saule/tree/main/examples/bf)

## Run it

```sh
git clone https://github.com/lauriszz123/saule.git
cd saule/examples/bf
saule run -- -d          # embedded hello world
saule run -- test.bf     # or a .bf file
saule run -- 400quine.bf
```

## `saule.config`

```
name: "bf"
version: "0.1.0"
entry: "src/main.sau"
src_dirs: ["src"]
min_saule_version: "26.1"
```

## `src/main.sau`

```saule title="src/main.sau"
import * from interpreter

class Main
	static fn usage()
		println("Usage:")
		println("bf <file> -- Runs the BF program.")
		println("bf -d     -- Runs a hello world embeeded in this program.")
		println("bf -h     -- Shows this screen.")
		println("bf -v     -- Prints the version.")
	end

	static fn version()
		printf("%s %s\n", Project.name, Project.version)
	end

	static fn main()
		local args = Os.args()

		if #args == 0 then
			usage()
		else
			match args[1]
				case "-h" then usage()

				case "-v" then version()

				case "-d" then runEmbeeded()

				case _ then runFile(args[1])
			end
		end
	end

	static local fn runFile(path: string)
		if Os.exists(path) then
			local file = Io.open(path, IoMode.Read)!
			runFromSource(file.read("a"))
			file.close()
		else
			printf("File '%s' does not exist.\n", path)
		end
	end

	static local fn runEmbeeded()
		local bf_helloworld: string = "++++++++[>++++[>++>+++>+++>+<<<<-]>+>+>->>+[<]<-]>>.>---.+++++++..+++.>>.<-.<.+++.------.--------.>>+.>++."
		runFromSource(bf_helloworld)
	end

	static local fn runFromSource(src: string)
		local interpreter: Interpreter = Interpreter(src)
		interpreter.run()
	end
end
```

## `src/interpreter.sau`

```saule title="src/interpreter.sau"
-- Brainfuck interpreter.
--
-- Full BF support: every command works, output streams as it's produced,
-- input is line-buffered from stdin, and unbalanced brackets are caught
-- up-front with a thrown error pointing at the offending column.
--
-- The pre-pass coalesces runs of `+ - < >` into single ops with a signed
-- delta, so tight loops in larger programs (mandelbrot, hanoi, quines)
-- don't pay per-character dispatch costs.

-- One decoded instruction. Bracket targets are resolved up-front in
-- `precompute`, so the run loop is a flat `match` with no extra state.
enum Op
	Inc(by: integer),  -- coalesced + / -
	Move(by: integer),  -- coalesced > / <
	Print,
	Read,
	JumpIfZero(target: integer),
	JumpIfNonZero(target: integer)
end

-- Lazy line-buffered byte queue over stdin. Saule's `Io.read` has no
-- byte-count form, so we pull a full line and dispense its bytes one
-- `,` at a time; on EOF every subsequent `,` yields 0 (the most common
-- BF convention).
class InputBuffer
	local pending: string
	local pos: integer
	local eof: boolean

	fn init()
		self.pending = ""
		self.pos = 1
		self.eof = false
	end

	fn next() -> integer
		if self.eof then
			return 0
		end
		if self.pos > String.len(self.pending) then
			local line: string? = Io.read("L")
			if line == nil then
				self.eof = true
				return 0
			end
			self.pending = line!
			self.pos = 1
		end
		local b: integer = String.byte(self.pending, self.pos) ?? 0
		self.pos = self.pos + 1
		return b
	end
end

export class Interpreter
	local source: string

	fn init(source: string)
		self.source = source
	end

	-- Decode source into a flat op vector with bracket targets resolved.
	-- Throws on unmatched brackets, citing the source column.
	local fn precompute() -> table<Op>
		local code: table<Op> = {}
		local stack: table<integer> = {}
		local n: integer = 0
		for ch, col in String.iter(self.source) do
			match ch
				case "+" then
					n = self.pushDelta(code, n, Op.Inc(1))

				case "-" then
					n = self.pushDelta(code, n, Op.Inc(-1))

				case ">" then
					n = self.pushDelta(code, n, Op.Move(1))

				case "<" then
					n = self.pushDelta(code, n, Op.Move(-1))

				case "." then
					n = n + 1
					code[n] = Op.Print

				case "," then
					n = n + 1
					code[n] = Op.Read

				case "[" then
					n = n + 1
					code[n] = Op.JumpIfZero(0)  -- target patched at `]`
					Table.insert(stack, n)

				case "]" then
					local open: integer? = Table.remove(stack)
					if open == nil then
						throw "unmatched `]` at column " .. tostring(col)
					end
					local openIdx: integer = open!
					n = n + 1
					code[n] = Op.JumpIfNonZero(openIdx)
					code[openIdx] = Op.JumpIfZero(n)

				case _ then nil  -- comment byte, ignored
			end
		end
		if #stack > 0 then
			throw "unmatched `[` (opened by op #" .. tostring(stack[1]) .. ")"
		end
		return code
	end

	-- Coalesce consecutive Inc / Move ops by summing their deltas.
	local fn pushDelta(code: table<Op>, n: integer, op: Op) -> integer
		if n > 0 then
			local fused: Op? = self.fuse(code[n]!, op)
			if fused != nil then
				code[n] = fused!
				return n
			end
		end
		code[n + 1] = op
		return n + 1
	end

	-- Returns the merged op when `prev` and `next` are the same `Inc` /
	-- `Move` family; `nil` when they can't be coalesced.
	local fn fuse(prev: Op, next: Op) -> Op?
		return match prev
			case Op.Inc(a) then match next
				case Op.Inc(b) then Op.Inc(a + b)

				case _ then nil
			end

			case Op.Move(a) then match next
				case Op.Move(b) then Op.Move(a + b)

				case _ then nil
			end

			case _ then nil
		end
	end

	fn run()
		local code: table<Op> = self.precompute()
		local tape: table<integer, integer> = {}
		local input: InputBuffer = InputBuffer()
		local ip: integer = 1
		local dp: integer = 0
		while code[ip] != nil do
			local cell: integer = tape[dp] ?? 0
			match code[ip]!
				case Op.Inc(by) then
					tape[dp] = self.wrap(cell + by)

				case Op.Move(by) then
					dp = dp + by

				case Op.Print then Io.write(String.char(cell))

				case Op.Read then
					tape[dp] = self.wrap(input.next())

				case Op.JumpIfZero(target) then
					if cell == 0 then
						ip = (target as integer)!
					end

				case Op.JumpIfNonZero(t) then
					if cell != 0 then
						ip = (t as integer)!
					end
			end
			ip = ip + 1
		end
	end

	-- Saule's `%` follows Rust semantics (sign of dividend), so a naive
	-- `(cell + by) % 256` would map `-1` to `-1`. Force into `0..255`.
	local fn wrap(value: integer) -> integer
		return (value % 256 + 256) % 256
	end
end
```
