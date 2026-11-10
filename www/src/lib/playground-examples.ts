/**
 * Programs the playground offers from its example picker.
 *
 * Each one is a complete single-file program: the playground runs in
 * single-file mode, so there is no `class Main` requirement and no imports
 * (module resolution needs a filesystem, which the browser build does not
 * have).
 *
 * Every program here is verified against the real compiler by
 * `npm run check-samples` — see scripts/check-samples.mjs.
 */

export interface Example {
	id: string;
	label: string;
	/**
	 * The sidebar heading this example sits under. Examples are listed in
	 * source order within a group, and the groups in the order they first
	 * appear here — so this array's order is the reading order, and there
	 * is no second list to keep it in.
	 */
	group: string;
	/** One-line description shown under the picker. */
	blurb: string;
	source: string;
}

export const EXAMPLES: Example[] = [
	{
		id: 'hello',
		group: 'Basics',
		label: 'Hello, world',
		blurb: 'The smallest program, plus typed locals.',
		source: `println("Hello, world!")

local name: string = "Saule"
local year: integer = 2026
local version: float = 1.0

println("Welcome to " .. name .. " " .. version .. ", " .. year)
`,
	},
	{
		id: 'types',
		group: 'Basics',
		label: 'Integer vs float',
		blurb: 'Lua’s single number type, split in two. Mixing them is a compile error.',
		source: `-- Saule splits Lua's \`number\` into two distinct types.
local health: integer = 100
local speed: float = 3.14

println("health: ", health)
println("speed:  ", speed)

-- A whole number that belongs in a float has to say so. Three ways,
-- all the same value:
local a: float = 2.0
local b: float = 2f     -- the \`f\` suffix forces a float
local c: float = .5 * 4f -- the integer part may be omitted

println(a, b, c)

-- Which is why \`2f\` is nicer than \`2 as float\` mid-expression:
println(speed * 2f)

-- Conversions are explicit, never implicit.
println((health as float) / 3.0)
println(speed as integer)

-- Try uncommenting this line — it will not compile:
-- local broken: integer = 3.14
`,
	},
	{
		id: 'classes',
		group: 'Classes and interfaces',
		label: 'Classes',
		blurb: 'Fields, a constructor, methods, statics, and inheritance.',
		source: `class Entity
    local name: string

    fn init(name: string)
        self.name = name
    end

    fn getName() -> string
        return self.name
    end
end

class Player extends Entity
    local health: integer

    static maxHealth: integer = 100

    fn init(name: string, health: integer)
        self.super(name)
        self.health = health
    end

    fn damage(amount: integer)
        self.health = self.health - amount

        if self.health <= 0 then
            self.health = 0
        end

        println(self.getName() .. " is at " .. self.health .. " HP")

        if self.health == 0 then
            println(self.getName() .. " has fallen")
        end
    end
end

local p: Player = Player("Arthur", Player.maxHealth)
p.damage(30)
p.damage(50)
p.damage(40)
`,
	},
	{
		id: 'nullsafety',
		group: 'The type system',
		label: 'Null safety',
		blurb: 'nil is opt-in, and the compiler makes you handle it.',
		source: `-- A variable can only hold nil if its type says so.
local nickname: string? = nil
local name: string = "Arthur"

-- \`??\` supplies a fallback.
println(nickname ?? "no nickname")

-- \`#\` needs a definite value, so coalesce first.
local len: integer = #(nickname ?? "")
println("length: ", len)

-- Assigning a nullable to a non-nullable is a compile error.
-- Uncomment to see the diagnostic:
-- local broken: string = nickname

println("hello, " .. name)
`,
	},
	{
		id: 'match',
		group: 'The type system',
		label: 'Pattern matching',
		blurb: 'Enums with payloads, matched exhaustively.',
		source: `enum Shape
    Circle(radius: float),
    Rect(w: float, h: float),
    Point
end

fn area(s: Shape) -> float
    return match s
        case Shape.Circle(r) then Math.pi * r * r
        case Shape.Rect(w, h) then w * h
        case Shape.Point then 0.0
    end
end

local shapes: table<Shape> = {
    Shape.Circle(2.0),
    Shape.Rect(3.0, 4.0),
    Shape.Point
}

for s: Shape in shapes do
    println(area(s))
end

-- Delete one of the \`case\` arms above and the program stops compiling:
-- match must be exhaustive.
`,
	},
	{
		id: 'generics',
		group: 'The type system',
		label: 'Generics & lambdas',
		blurb: 'Type parameters, function types, and arrow lambdas.',
		source: `fn filter<T>(items: table<T>, predicate: fn(T) -> boolean) -> table<T>
    local result: table<T> = {}

    for item: T in items do
        if predicate(item) then
            result[#result + 1] = item
        end
    end

    return result
end

fn map<T, U>(items: table<T>, f: fn(T) -> U) -> table<U>
    local out: table<U> = {}

    for item: T in items do
        out[#out + 1] = f(item)
    end

    return out
end

local nums: table = {1, 2, 3, 4, 5, 6}

local evens = filter(nums, x => x % 2 == 0)
local doubled = map(evens, x => x * 2)

for n in doubled do
    println(n)
end
`,
	},
	{
		id: 'interfaces',
		group: 'Classes and interfaces',
		label: 'Interfaces',
		blurb: 'Multiple implementation, and interfaces used as types.',
		source: `interface Named
    fn getName() -> string
end

interface Damageable
    fn damage(amount: integer) -> nil
    fn isAlive() -> boolean
end

class Monster implements Named, Damageable
    local name: string
    local health: integer

    fn init(name: string, health: integer)
        self.name = name
        self.health = health
    end

    fn getName() -> string
        return self.name
    end

    fn damage(amount: integer)
        self.health = self.health - amount

        if self.health <= 0 then
            self.health = 0
        end
    end

    fn isAlive() -> boolean
        return self.health > 0
    end
end

local goblin: Monster = Monster("Goblin", 30)

goblin.damage(20)
println(goblin.getName() .. " alive? " .. (goblin.isAlive() and "yes" or "no"))

goblin.damage(20)
println(goblin.getName() .. " alive? " .. (goblin.isAlive() and "yes" or "no"))
`,
	},
	{
		id: 'operators',
		group: 'Classes and interfaces',
		label: 'Operator overloading',
		blurb: 'Classes define what `+`, `==`, `<` and `tostring` mean for them.',
		source: `-- Operators are interfaces. A class opts into \`+\`, \`-\`, \`==\`, \`<\` or
-- \`tostring\` by implementing the matching \`Op*\` contract — Saule's answer
-- to Lua's __add / __sub / __eq metamethods.

class Vec2 implements OpAdd<Vec2, Vec2>, OpSub<Vec2, Vec2>, OpNeg<Vec2>, OpEq<Vec2>, OpToString
    local x: float
    local y: float

    fn init(x: float, y: float)
        self.x = x
        self.y = y
    end

    fn add(other: Vec2) -> Vec2
        return Vec2(self.x + other.x, self.y + other.y)
    end

    fn sub(other: Vec2) -> Vec2
        return Vec2(self.x - other.x, self.y - other.y)
    end

    fn neg() -> Vec2
        return Vec2(-self.x, -self.y)
    end

    fn equals(other: Vec2) -> boolean
        return self.x == other.x and self.y == other.y
    end

    fn toString() -> string
        return "(" .. self.x .. ", " .. self.y .. ")"
    end
end

local a: Vec2 = Vec2(1.0, 2.0)
local b: Vec2 = Vec2(3.0, 4.0)

-- The result type comes from the method, so \`a + b\` is a \`Vec2\`.
local sum: Vec2 = a + b

println(sum)                     -- toString() runs here
println(a - b)
println(-a)
println(a == Vec2(1.0, 2.0))     -- equals(), not pointer identity

-- One \`compare\` drives all four ordering operators. It returns a negative
-- number when self sorts first, zero when equivalent, positive when last.
class Version implements OpCompare<Version>
    local major: integer
    local minor: integer

    fn init(major: integer, minor: integer)
        self.major = major
        self.minor = minor
    end

    fn compare(other: Version) -> integer
        if self.major != other.major then
            return self.major - other.major
        end

        return self.minor - other.minor
    end
end

local old: Version = Version(1, 9)
local fresh: Version = Version(2, 0)

println(old < fresh)
println(fresh >= old)

-- \`^\` is exponentiation: right-associative and tighter than unary minus.
println(2 ^ 10)
println(2 ^ 3 ^ 2)
`,
	},
	{
		id: 'fizzbuzz',
		group: 'Basics',
		label: 'FizzBuzz',
		blurb: 'Loops and conditionals, the traditional way.',
		source: `for i = 1, 20 do
    if i % 15 == 0 then
        println("FizzBuzz")
    elseif i % 3 == 0 then
        println("Fizz")
    elseif i % 5 == 0 then
        println("Buzz")
    else
        println(i)
    end
end
`,
	},
	{
		id: 'trailingblocks',
		group: 'Syntax',
		label: 'Trailing blocks',
		blurb: 'A declarative UI, where a container takes its children as a do ... end block.',
		source: `class Ui
    static lines: table<string> = {}

    static fn text(value: string) -> nil
        Table.insert(Ui.lines, value)
    end

    static fn mark() -> integer
        return #Ui.lines
    end

    static fn since(mark: integer) -> table<string>
        local taken: table<string> = {}
        local i: integer = mark + 1

        while i <= #Ui.lines do
            Table.insert(taken, Ui.lines[i])
            i = i + 1
        end

        while #Ui.lines > mark do
            Table.remove(Ui.lines)
        end

        return taken
    end
end

-- The last parameter is a function, so callers can pass it as a trailing
-- block. Because it is last, "spacing" may sit in front of it with a default
-- and be skipped at the call site: the block still lands on "body".
class Panel
    fn init(title: string, spacing: integer = 0, body: fn() -> nil)
        local start: integer = Ui.mark()
        body()
        local kids: table<string> = Ui.since(start)

        local inner: integer = String.len(title)
        for _, line in kids do
            if String.len(line) > inner then
                inner = String.len(line)
            end
        end

        Ui.text("+" .. title .. String.rep("-", inner - String.len(title)) .. "+")

        local i: integer = 0
        while i < spacing do
            Ui.text("|" .. String.rep(" ", inner) .. "|")
            i = i + 1
        end

        for _, line in kids do
            Ui.text("|" .. line .. String.rep(" ", inner - String.len(line)) .. "|")
        end

        Ui.text("+" .. String.rep("-", inner) .. "+")
    end
end

class Text
    fn init(value: string)
        Ui.text(value)
    end
end

-- The call below is exactly:
--     Panel(title: "Saule", spacing: 1, fn()
--         ...
--     end)
-- Same call, less ceremony. Try rewriting it that way and re-running.
Panel(title: "Saule", spacing: 1) do
    Text("Trailing blocks!")

    Panel(title: "Scores") do
        -- A block holds statements, so "for" and "if" work inside it.
        local names: table<string> = {"ada", "grace", "linus"}
        local scores: table<integer> = {120, 340, 90}

        for i, name in names do
            if scores[i] >= 100 then
                Text(name .. ": " .. scores[i] .. " *")
            else
                Text(name .. ": " .. scores[i])
            end
        end
    end
end

for _, line in Ui.lines do
    println(line)
end
`,
	},
	{
		id: 'coroutines',
		group: 'Coroutines',
		label: 'Generators',
		blurb: 'Functions that pause with yield and pick up where they left off — typed, and stackful.',
		source: `-- A coroutine pauses at each yield and resumes from exactly there.
-- The declared return type tells the body what it yields, so \`co\`
-- needs no annotation: it is a Yielder<integer>.
fn range(n: integer) -> Coroutine<integer>
    return Coroutine.create(fn(co)
        for i = 1, n do
            co.yield(i)
        end
    end)
end

-- Generators compose: each one pulls from the one before.
fn squares(src: Coroutine<integer>) -> Coroutine<integer>
    return Coroutine.create(fn(co)
        for v in src do
            co.yield(v * v)
        end
    end)
end

for sq in squares(range(5)) do
    println(sq)                   -- 1, 4, 9, 16, 25
end

-- Stackful: pass the yielder down, and any function can yield.
fn countdown(co: Yielder<string>, n: integer)
    if n == 0 then
        co.yield("liftoff")
        return
    end
    co.yield("" .. n)
    countdown(co, n - 1)
end

local launch: Coroutine<string> = Coroutine.create(fn(co)
    countdown(co, 3)
end)

println(launch.resume())          -- 3
println(launch.status())          -- suspended
for step in launch do
    println(step)                 -- 2, 1, liftoff
end
println(launch.status())          -- dead

-- A failure inside a coroutine is reported, not fatal.
local risky: Coroutine<integer> = Coroutine.create(fn(co)
    co.yield(1)
    throw "sensor offline"
end)
println(risky.resume())           -- 1
println(risky.resume())           -- nil
println(risky.error() ?? "")      -- sensor offline
`,
	},
	{
		id: 'streams',
		group: 'Coroutines',
		label: 'Lazy pipelines',
		blurb: 'Stream combinators that are themselves coroutines — map, filter and take over an infinite source, costing only what you take.',
		source: `-- Lazy streams, where every combinator is itself a coroutine.
--
-- A Coroutine<T> is already a lazy sequence: nothing in the body runs until
-- something resumes it, and \`for v in src do\` resumes it once per value. So a
-- combinator is just a coroutine that loops over another one.

-- Infinite. That is safe here, because a stage only runs when the stage after
-- it asks for a value.
fn naturals(start: integer) -> Coroutine<integer>
    return Coroutine.create(fn(co)
        local n: integer = start
        while true do
            co.yield(n)
            n = n + 1
        end
    end)
end

-- The generator shape fibonacci wants: \`a\` and \`b\` are locals that survive
-- between resumes, not fields on an object.
fn fibonacci() -> Coroutine<integer>
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

-- \`T\` comes from the source, \`U\` from the mapper, and the body's yielder is a
-- Yielder<U> because that is what the declared return type promises.
fn map<T, U>(src: Coroutine<T>, f: fn(T) -> U) -> Coroutine<U>
    return Coroutine.create(fn(co)
        for value in src do
            co.yield(f(value))
        end
    end)
end

fn filter<T>(src: Coroutine<T>, keep: fn(T) -> boolean) -> Coroutine<T>
    return Coroutine.create(fn(co)
        for value in src do
            if keep(value) then
                co.yield(value)
            end
        end
    end)
end

-- \`break\` leaves the source suspended where it was rather than draining it.
fn take<T>(src: Coroutine<T>, n: integer) -> Coroutine<T>
    return Coroutine.create(fn(co)
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

fn collect<T>(src: Coroutine<T>) -> table<T>
    local out: table<T> = {}
    for value in src do
        out[#out + 1] = value
    end
    return out
end

local evens: table<integer> = collect(take(filter(naturals(1), n => n % 2 == 0), 6))
println("first six even naturals: " .. String.join(", ", evens))

local fib: table<integer> = collect(take(fibonacci(), 12))
println("fibonacci: " .. String.join(", ", fib))

local labelled: table<string> = collect(map(take(fibonacci(), 5), n => "<" .. n .. ">"))
println("mapped: " .. String.join(" ", labelled))

-- Laziness, counted rather than claimed: the counter is an ordinary captured
-- local, bumped once per value the infinite source actually produced.
local produced: integer = 0
local counted: Coroutine<integer> = map(naturals(1), fn(n: integer) -> integer
    produced = produced + 1
    return n
end)
println("took " .. String.join(", ", collect(take(counted, 3))) ..
    " from an infinite source, which produced " .. produced .. " values")
`,
	},
	{
		id: 'stackful',
		group: 'Coroutines',
		label: 'Stackful tokenizer',
		blurb: 'A tokenizer that yields from two frames below the coroutine body, because the yielder is an ordinary value.',
		source: `-- Stackful: the yielder is an ordinary value, so any function the coroutine
-- called can yield on its behalf.
--
-- A hand-written tokenizer usually needs a class with an index field, because
-- \`next()\` has to pick up where the last call left off. Here \`scan\` is written
-- as one straight-line loop — \`i\` is a local — and each \`co.yield\` hands a
-- token out and freezes the loop mid-stride. The whole call stack between the
-- yield and the resume is suspended with it.

fn isDigit(c: string) -> boolean
    return c >= "0" and c <= "9"
end

-- Two frames below the coroutine body, and still yielding. The return value
-- is where scanning carries on from.
fn scanNumber(co: Yielder<string>, source: string, start: integer) -> integer
    local i: integer = start
    while i <= String.len(source) and isDigit(String.sub(source, i, i)) do
        i = i + 1
    end
    co.yield("number " .. String.sub(source, start, i - 1))
    return i
end

fn scan(co: Yielder<string>, source: string)
    local i: integer = 1
    while i <= String.len(source) do
        local c: string = String.sub(source, i, i)
        if c == " " then
            i = i + 1
        elseif isDigit(c) then
            i = scanNumber(co, source, i)
        elseif String.contains("+-*/()", c) then
            co.yield("symbol " .. c)
            i = i + 1
        else
            -- A throw the coroutine does not catch kills *it*, not the
            -- program: resume() answers nil and error() carries this message.
            throw "unexpected '" .. c .. "' at column " .. i
        end
    end
end

fn tokens(source: string) -> Coroutine<string>
    return Coroutine.create(fn(co)
        scan(co, source)
    end)
end

for token in tokens("12 + (34 * 5)") do
    println(token)
end

-- Stopping early really stops the work: \`break\` leaves the lexer suspended on
-- the character after the token it just handed over.
local stream: Coroutine<string> = tokens("1 + 2 + 3 + 4")
for token in stream do
    if String.starts(token, "symbol") then
        break
    end
end
println("broke out; the lexer is " .. stream.status())
println("resuming carries on: " .. (stream.resume() ?? ""))

-- Bad input, reported rather than fatal — and everything yielded before the
-- failure was still delivered.
local broken: Coroutine<string> = tokens("7 * $")
while true do
    local token: string? = broken.resume()
    if token == nil then
        break
    end
    println("delivered " .. token!)
end
println("then: " .. broken.status() .. ", because " .. (broken.error() ?? ""))
`,
	},
	{
		id: 'tasks',
		group: 'Coroutines',
		label: 'Cooperative tasks',
		blurb: 'Four tasks interleaved on one thread, round-robin — including one that fails without taking the others down.',
		source: `-- Cooperative multitasking: a task is a coroutine that decides for itself
-- when to hand control back.
--
-- Nothing runs in parallel and there are no locks. \`resume()\` transfers
-- control into a task, and that task's next \`yield\` transfers it back — so
-- between two yields a task has the whole program to itself.

class Task
    name: string = ""
    local job: Coroutine<string>

    fn init(name: string, body: fn(Yielder<string>) -> nil)
        self.name = name
        self.job = Coroutine.create(body)
    end

    fn isDone() -> boolean
        return self.job.isDone()
    end

    -- One slice of work. How long a slice is, is the task's own business:
    -- \`resume\` runs it until *it* yields.
    fn step(tick: integer)
        local note: string? = self.job.resume()

        if note != nil then
            println(String.format("tick %d  %-9s %s", tick, self.name, note!))
            return
        end

        -- \`nil\` means the task is over — but over how? A crash is contained:
        -- the coroutine dies, the program does not, and the other tasks keep
        -- their turns. \`error()\` tells a crash from a clean finish.
        local failure: string? = self.job.error()
        if failure != nil then
            println(String.format("tick %d  %-9s FAILED: %s", tick, self.name, failure!))
        else
            println(String.format("tick %d  %-9s done", tick, self.name))
        end
    end
end

local tasks: table<Task> = {}

-- The parameter type is the coroutine contract itself: fn(Yielder<string>).
fn spawn(name: string, body: fn(Yielder<string>) -> nil)
    tasks[#tasks + 1] = Task(name, body)
end

spawn("download", fn(co)
    for percent = 25, 100, 25 do
        co.yield("fetching " .. percent .. "%")
    end
end)

spawn("render", fn(co)
    co.yield("laying out")
    co.yield("painting")
end)

spawn("sensor", fn(co)
    co.yield("reading 21.4C")
    throw "sensor offline"
end)

-- A \`yield\` inside a \`try\` resumes *inside the same try*, so a task can pause
-- mid-retry and still catch its own failure afterwards.
spawn("retry", fn(co)
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

-- Round-robin: every live task gets one slice per tick, until none is left.
fn alive() -> integer
    local count: integer = 0
    for task in tasks do
        if not task.isDone() then
            count = count + 1
        end
    end
    return count
end

local tick: integer = 0
while alive() > 0 do
    tick = tick + 1
    for task in tasks do
        if not task.isDone() then
            task.step(tick)
        end
    end
end

println("four tasks interleaved over " .. tick .. " ticks, on one thread")
`,
	},
];

export const DEFAULT_EXAMPLE = EXAMPLES[0]!;
