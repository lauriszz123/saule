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
 *
 * The three groups are difficulty tiers, read in order:
 *
 *   Simple        — the language without classes: values, text, tables,
 *                   loops, functions.
 *   Intermediate  — the type system and the object model: nullability,
 *                   match, enums, classes, interfaces, generics.
 *   Advanced      — what Saule does that its ancestors do not: operator and
 *                   index overloading, pipelines, trailing blocks,
 *                   coroutines.
 *
 * A program belongs to a tier by the features it uses, not by its length.
 */

export interface Example {
	id: string;
	label: string;
	/**
	 * The difficulty tier this example sits under, and the sidebar heading
	 * it is listed beneath: `Simple`, `Intermediate` or `Advanced`.
	 * Examples are listed in source order within a tier, and the tiers in
	 * the order they first appear here — so this array's order is the
	 * reading order, and there is no second list to keep it in.
	 */
	group: string;
	/** One-line description shown under the picker. */
	blurb: string;
	source: string;
}

export const EXAMPLES: Example[] = [
	{
		id: 'hello',
		group: 'Simple',
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
		group: 'Simple',
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
		id: 'fizzbuzz',
		group: 'Simple',
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
		id: 'controlflow',
		group: 'Simple',
		label: 'Loops and control flow',
		blurb: 'Every loop Saule has — numeric, for-in, while, repeat-until — with break and continue.',
		source: `-- Every loop Saule has, and the two ways out of one.

-- A numeric \`for\` counts. The optional third value is the step, and it may
-- be negative — the loop variable is an \`integer\` here with no annotation.
for i = 1, 5 do
    print(i .. " ")
end
println("")

for i = 10, 0, -5 do
    print(i .. " ")
end
println("")

-- \`for … in\` walks anything iterable: a table, a map (two variables), a
-- coroutine, or a class that implements \`Iterable<T>\`.
local planets: table<string> = {"mercury", "venus", "earth"}
for planet in planets do
    print(planet .. " ")
end
println("")

for index, planet in planets do
    print(index .. ":" .. planet .. " ")
end
println("")

-- \`while\` tests before the body, \`repeat\` after — so a \`repeat\` body always
-- runs at least once. Note \`until\`, not \`while not\`.
local countdown: integer = 3
while countdown > 0 do
    print(countdown .. "… ")
    countdown = countdown - 1
end
println("go")

local tries: integer = 0
repeat
    tries = tries + 1
until tries >= 3
println("ran " .. tries .. " times")

-- \`break\` leaves the loop; \`continue\` skips to the next turn.
for n = 1, 20 do
    if n % 2 == 1 then
        continue
    end
    if n > 10 then
        break
    end
    print(n .. " ")
end
println("")

-- \`if / elseif / else\`, and the one-line form \`and\` / \`or\` gives you when a
-- value is what you want rather than a branch.
fn describe(n: integer) -> string
    if n < 0 then
        return "negative"
    elseif n == 0 then
        return "zero"
    elseif n < 10 then
        return "small"
    else
        return "large"
    end
end

for n in {-5, 0, 7, 99} do
    println(n .. " is " .. describe(n))
end

local temperature: integer = 31
println("it is " .. (temperature > 25 and "hot" or "mild"))

-- Loops nest, and \`break\` leaves only the innermost one.
for row = 1, 3 do
    local line: string = ""
    for col = 1, 3 do
        if col > row then
            break
        end
        line = line .. "#"
    end
    println(line)
end
`,
	},
	{
		id: 'strings',
		group: 'Simple',
		label: 'Strings and text',
		blurb: 'UTF-8 text with no methods of its own: the String class does the work, visibly.',
		source: `-- Text in Saule is a \`string\`: UTF-8, immutable, and with no methods of its
-- own. Everything that works on one lives on the \`String\` class, so the
-- operation you are calling is always visible at the call site.

local name: string = "Ada Lovelace"

-- \`..\` joins. Numbers and anything with a \`toString\` interpolate directly.
println("hello, " .. name .. "!")
println("in " .. 1843 .. " she wrote note G")

-- Length and slicing are 1-based, and \`sub\` counts characters, not bytes.
println(String.len(name))
println(String.sub(name, 1, 3))
println(String.sub(name, 5))            -- from 5 to the end
println(String.sub(name, -8))           -- or from 8 characters before the end

-- Asking questions.
println(String.contains(name, "Love"))
println(String.starts(name, "Ada"))
println(String.indexOf(name, "a") ?? 0) -- nullable: there may be no match

-- Reshaping. Each one answers a new string; the original never changes.
println(String.upper(name))
println(String.replace(name, " ", "_"))
println(String.rep("-", 20))
println(String.trim("   padded   ") .. "|")

-- Splitting and joining are the pair you reach for most.
local parts: table<string> = String.split("2026-11-10", "-")
println(#parts .. " parts: " .. String.join("/", parts))

-- \`String.format\` is Lua's \`string.format\`, and \`printf\` prints it directly.
println(String.format("%-10s %5.2f %3d%%", "coverage", 93.457, 93))
printf("%s is %d characters, %s%s", name, String.len(name), "padded: ", String.padStart("7", 3, "0"))
println("")

-- Lining up columns is \`padEnd\` plus \`padStart\`, no format string needed.
local items: table<string> = {"apples", "rice", "coffee"}
local prices: table<float> = {3.5, 1.25, 12.0}
for i, item in items do
    println(String.padEnd(item, 10, ".") .. String.padStart(String.format("%.2f", prices[i]), 7))
end
`,
	},
	{
		id: 'tables',
		group: 'Simple',
		label: 'Tables',
		blurb: 'Lua’s one data structure, typed: lists, maps, the three ways to walk one, Table vs Iter.',
		source: `-- A \`table\` is Lua's one data structure, and Saule types it. \`table<V>\` is a
-- list of \`V\`; \`table<K, V>\` is a map. Both halves can live in one value, as
-- they do in Lua, but the type says which half you are using.

-- A list. \`table<string>\` and the \`string[]\` suffix mean the same thing.
local names: table<string> = {"ada", "grace", "linus"}
local scores: integer[] = {120, 340, 90}

println(#names .. " names, first is " .. names[1])

-- Appending. \`#t + 1\` is the idiom; \`Table.insert\` is the same thing with a
-- name, and takes a position when you want one.
names[#names + 1] = "alan"
Table.insert(names, "edsger")
Table.insert(names, "grete", 1)         -- at the front
println(String.join(", ", names))

println(Table.remove(names, 1) ?? "")   -- removes and answers it
println(Table.remove(names) ?? "")      -- the last one, when no position

-- A map. The key type is part of the type, so \`ages["ada"]\` is an \`integer?\`
-- — a key that is not there is \`nil\`, and the checker makes you say so.
local ages: table<string, integer> = {}
ages["ada"] = 36
ages["grace"] = 85
println("ada is " .. (ages["ada"] ?? 0) .. ", nobody is " .. (ages["nobody"] ?? 0))

-- A literal may give the keys directly — \`theme:\` is sugar for the string
-- key "theme" — and \`t.theme\` reads the same key as \`t["theme"]\`.
local config: table<string, string> = {theme: "dark", editor: "saule"}
println((config.theme ?? "?") .. " / " .. (config["editor"] ?? "?"))

-- Three ways to walk one. \`for v in t\` is the array part; \`for k, v in t\`
-- is both halves; a numeric \`for\` is the one that can step or go backwards.
for name in names do
    print(name .. " ")
end
println("")

for who, age in ages do
    println(who .. " -> " .. age)
end

for i = #scores, 1, -1 do
    print(scores[i] .. " ")
end
println("")

-- \`Table.*\` mutates in place; \`Iter.*\` derives a new table and leaves the
-- original alone. Both are fully typed, so \`total\` below is an \`integer\`.
Table.sort(scores, (a, b) => a > b)
println("ranked: " .. String.join(" > ", scores))

local doubled: table<integer> = Iter.map(scores, s => s * 2)
local big: table<integer> = Iter.filter(scores, s => s >= 100)
local total: integer = Iter.reduce(scores, 0, (sum, s) => sum + s)

println("doubled: " .. String.join(", ", doubled))
println("at least 100: " .. String.join(", ", big))
println("total: " .. total .. ", best: " .. (Iter.first(scores) ?? 0))
println("any under 100? " .. tostring(Iter.any(scores, s => s < 100)))
`,
	},
	{
		id: 'functions',
		group: 'Simple',
		label: 'Functions',
		blurb: 'Defaults, named arguments, multiple returns, variadics, and closures.',
		source: `-- Functions: four things Saule adds to Lua's \`function\`.

-- 1. Types. Parameters and the return type are declared, and \`-> nil\` (or
--    nothing at all) means it returns nothing.
fn greet(who: string) -> string
    return "hello, " .. who
end

println(greet("ada"))

-- 2. Defaults. A parameter with a default may be left out at the call site.
fn createServer(host: string, port: integer = 8080, secure: boolean = false) -> string
    return (secure and "https://" or "http://") .. host .. ":" .. port
end

println(createServer("localhost"))
println(createServer("localhost", 3000))
println(createServer("saule.dev", 443, true))

-- 3. Named arguments, so a long call says what each value is for. Named and
--    positional mix, as long as the positional ones come first.
fn window(width: integer, height: integer, title: string, fullscreen: boolean = false) -> string
    return title .. " " .. width .. "x" .. height .. (fullscreen and " (fullscreen)" or "")
end

println(window(width: 1920, height: 1080, title: "Saule"))
println(window(800, 600, title: "Editor", fullscreen: true))

-- 4. Multiple return values, declared as a tuple and unpacked on assignment.
fn minMax(items: table<integer>) -> (integer, integer)
    local min: integer = items[1]
    local max: integer = items[1]

    for value in items do
        if value < min then
            min = value
        end
        if value > max then
            max = value
        end
    end

    return min, max
end

local low: integer, high: integer = minMax({3, 1, 7, 2, 9})
println("from " .. low .. " to " .. high)

-- Variadics collect the rest into a table of one type.
fn sum(...values: integer) -> integer
    local total: integer = 0
    for value in values do
        total = total + value
    end
    return total
end

println(sum(1, 2, 3, 4, 5))
println(sum())

-- Functions are values: pass one in, return one out, or capture a local in a
-- closure. \`x => x * 2\` is the arrow form of \`fn(x) return x * 2 end\`.
fn applyTwice(value: integer, f: fn(integer) -> integer) -> integer
    return f(f(value))
end

println(applyTwice(3, x => x * 2))

fn counter(start: integer) -> fn() -> integer
    local count: integer = start
    return fn()
        count = count + 1
        return count
    end
end

local tick: fn() -> integer = counter(10)
println(tick() .. ", " .. tick() .. ", " .. tick())
`,
	},
	{
		id: 'nullsafety',
		group: 'Intermediate',
		label: 'Null safety',
		blurb: 'nil is opt-in, and ??, ?., ?[ ] and ! are how you handle it.',
		source: `-- \`nil\` is opt-in. A \`string\` can never hold it; a \`string?\` can, and the
-- checker will not let you forget which one you have.

local name: string = "Ada"
local nickname: string? = nil

-- Four operators handle a nullable, and they compose.
--   ??  fall back to a value
--   ?.  call or read only if there is something there
--   ?[  index only if there is something there
--   !   assert it is there (a runtime error if it is not)

println(nickname ?? "no nickname")             -- ??
println(#(nickname ?? ""))                     -- # needs a definite value

-- Uncomment either line to see the compiler refuse it:
-- local broken: string = nickname             -- nullable into non-nullable
-- println(String.len(nickname))               -- String.len needs a string

class Profile
    local email: string?

    fn init(email: string?)
        self.email = email
    end

    fn emailOf() -> string?
        return self.email
    end

    fn domain() -> string?
        local at: integer? = String.indexOf(self.email ?? "", "@")
        if at == nil then
            return nil
        end
        return String.sub(self.email!, at! + 1)
    end
end

class Directory
    local people: table<string, Profile>

    fn init()
        self.people = {}
        self.people["ada"] = Profile("ada@example.com")
        self.people["grace"] = Profile(nil)
    end

    -- A lookup that may miss says so in its return type.
    fn find(who: string) -> Profile?
        return self.people[who]
    end
end

local directory: Directory = Directory()

-- \`?.\` short-circuits the whole chain: one \`nil\` anywhere and the result is
-- \`nil\`, with nothing in between ever running.
println(directory.find("ada")?.domain() ?? "unknown")
println(directory.find("grace")?.domain() ?? "unknown")
println(directory.find("nobody")?.domain() ?? "unknown")

-- \`?[\` is the same idea for an index.
local tags: table<string>? = {"typed", "fast"}
local missing: table<string>? = nil
println((tags?[1] ?? "none") .. " / " .. (missing?[1] ?? "none"))

-- \`!\` says "I know this is there". It is checked at runtime, so it is the
-- one that can fail — prefer \`??\` or a guard when you can.
local certain: string? = "present"
println(String.upper(certain!))

-- A narrowing \`if\` is the other way through: inside the branch the checker
-- knows it is not nil, so no \`!\` is needed.
local maybe: integer? = 41
if maybe != nil then
    println(maybe + 1)
end

-- \`match\` consumes a nullable directly, with \`nil\` as just another pattern.
match directory.find("ada")
    case nil then println("not found")
    case profile then println("found: " .. (profile.emailOf() ?? ""))
end

-- \`assert\` turns a nullable into a definite value or stops the program.
local port: integer = assert("8080" as integer, "port must be a number")
println(port + 1)
`,
	},
	{
		id: 'errors',
		group: 'Intermediate',
		label: 'Error handling',
		blurb: 'throw with typed catch clauses and finally — and when a nullable return is the better answer.',
		source: `-- Two ways a Saule function can fail, and when each one is right.

-- 1. \`throw\` plus \`try\` / \`catch\`, for the unexpected: bad data from outside,
--    a contract violation, an I/O failure.
class ParseError
    detail: string = ""
    line: integer = 0

    fn init(detail: string, line: integer)
        self.detail = detail
        self.line = line
    end
end

fn parsePort(text: string) -> integer
    if not String.contains(text, "=") then
        throw ParseError("no '=' in " .. text, 1)
    end

    local value: integer? = String.sub(text, (String.indexOf(text, "=") ?? 0) + 1) as integer
    if value == nil then
        throw "port is not a number"
    end
    if value! < 1 or value! > 65535 then
        throw value!                       -- the thrown value can be anything
    end

    return value!
end

-- Clauses are tried in source order, and the first whose type matches the
-- thrown value wins. \`catch e: any\` matches everything, so it belongs last.
local inputs: table<string> = {"port=8080", "port", "port=abc", "port=99999"}

for input in inputs do
    try
        println(input .. " -> " .. parsePort(input))
    catch e: ParseError
        println(input .. " -> parse error on line " .. e.line .. ": " .. e.detail)
    catch e: integer
        println(input .. " -> " .. e .. " is outside 1..65535")
    catch e: any
        println(input .. " -> " .. tostring(e))
    end
end

-- \`finally\` runs on every way out: a clean finish, a handled throw, an
-- unhandled one, and a \`return\` from inside the \`try\`.
fn withCleanup(fail: boolean) -> string
    try
        if fail then
            throw "boom"
        end
        return "worked"
    catch e: string
        return "handled: " .. e
    finally
        println("  (cleanup ran)")
    end
end

println(withCleanup(false))
println(withCleanup(true))

-- 2. A nullable return, for the expected: a lookup that might miss is not an
--    error, and \`T?\` keeps that in the signature where a caller can see it.
class Directory
    local people: table<string, string>

    fn init()
        self.people = {}
        self.people["ada"] = "ada@example.com"
    end

    fn emailOf(name: string) -> string?
        return self.people[name]
    end
end

local directory: Directory = Directory()

-- \`?.\`, \`??\` and \`!\` are how a nullable is handled: coalesce, or prove it.
println(directory.emailOf("ada") ?? "unknown")
println(directory.emailOf("grace") ?? "unknown")
println(String.len(directory.emailOf("ada") ?? ""))

-- Rule of thumb: \`throw\` for "this should not happen", \`T?\` for "this might
-- not be there". The first is a surprise; the second is a normal answer.
`,
	},
	{
		id: 'match',
		group: 'Intermediate',
		label: 'Pattern matching',
		blurb: 'Literals, guards, bindings, enum payloads and nil — as a statement or an expression.',
		source: `-- \`match\` is an expression as well as a statement, and every arm is checked:
-- the patterns must cover the value, and each arm must produce the same type.

-- Literal patterns, a binding, and a wildcard.
fn run(command: string) -> string
    return match command
        case "quit" then "bye"
        case "help" then "try: quit, help, open <path>"
        case other then "unknown command: " .. other
    end
end

for command in {"help", "quit", "dance"} do
    println(run(command))
end

-- Guards with \`when\` run after the pattern matches; a false guard moves on
-- to the next arm.
fn classify(n: integer) -> string
    return match n
        case 0 then "zero"
        case x when x < 0 then "negative"
        case x when x % 2 == 0 then "even"
        case _ then "odd"
    end
end

for n in {0, -4, 8, 7} do
    println(n .. " is " .. classify(n))
end

-- Enum payloads destructure into the arm's bindings, and the match must
-- cover every variant: delete an arm and the program stops compiling.
enum Event
    Click(x: integer, y: integer),
    Key(code: string),
    Scroll(amount: integer),
    Quit
end

fn handle(event: Event) -> string
    return match event
        case Event.Click(x, y) then "click at " .. x .. "," .. y
        case Event.Key(code) when code == "escape" then "escape closes the menu"
        case Event.Key(code) then "key " .. code
        case Event.Scroll(amount) when amount < 0 then "scroll up " .. -amount
        case Event.Scroll(amount) then "scroll down " .. amount
        case Event.Quit then "quit"
    end
end

local events: table<Event> = {
    Event.Click(12, 40),
    Event.Key("escape"),
    Event.Key("a"),
    Event.Scroll(-3),
    Event.Scroll(5),
    Event.Quit
}

for event in events do
    println(handle(event))
end

-- A nullable is matched with \`nil\` as a pattern. The binding arm captures
-- the non-nil value, so inside it the type is \`string\`, not \`string?\`.
fn describe(value: string?) -> string
    return match value
        case nil then "nothing"
        case "" then "an empty string"
        case text then "the text " .. text .. " (" .. String.len(text) .. " chars)"
    end
end

println(describe(nil))
println(describe(""))
println(describe("saule"))

-- Tuples from a multi-value function destructure too. (\`/\` on two integers
-- is integer division, so this is the quotient and the remainder.)
fn divide(a: integer, b: integer) -> (integer, integer)
    if b == 0 then
        return 0, 0
    end
    return a / b, a % b
end

match divide(17, 5)
    case (q, 0) then println("divides exactly: " .. q)
    case (q, r) then println(q .. " remainder " .. r)
end

-- As a statement, when there is nothing to return.
local level: integer = 2
match level
    case 1 then println("warming up")
    case 2 then println("getting there")
    case _ then println("done")
end
`,
	},
	{
		id: 'enums',
		group: 'Intermediate',
		label: 'Enums',
		blurb: 'Plain variants, valued variants with methods, and payloads that match destructures.',
		source: `-- Enums are real types: a closed set of variants, checked at compile time.

-- The simplest kind is a set of names.
enum Direction
    North
    South
    East
    West
end

local heading: Direction = Direction.North
println(heading == Direction.North)

-- A valued enum gives each variant a value, reachable as \`.value\`, and may
-- carry methods that use it.
enum Level
    Debug = 10
    Info = 20
    Warning = 30
    Error = 40

    fn label() -> string
        return match self
            case Level.Debug then "DEBUG"
            case Level.Info then "INFO"
            case Level.Warning then "WARN"
            case Level.Error then "ERROR"
        end
    end

    fn atLeast(floor: Level) -> boolean
        return self.value >= floor.value
    end
end

fn log(level: Level, message: string)
    if level.atLeast(Level.Info) then
        println(String.padEnd(level.label(), 6) .. message)
    end
end

log(Level.Debug, "cache warm (hidden: below the floor)")
log(Level.Info, "listening on 8080")
log(Level.Warning, "disk 80% full")
log(Level.Error, "connection refused")

-- Variants may also carry payloads, which \`match\` destructures. This is the
-- shape a result or a tagged union takes in Saule.
enum Shape
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

local shapes: table<Shape> = {Shape.Circle(1.0), Shape.Rect(3.0, 4.0), Shape.Point}
for shape in shapes do
    println(String.format("%.3f", area(shape)))
end

-- An enum used as a state machine. \`match\` must cover every variant, so
-- adding a state to the enum makes the compiler point at this function.
enum State
    Idle
    Running
    Done
end

fn next(current: State) -> State
    return match current
        case State.Idle then State.Running
        case State.Running then State.Done
        case State.Done then State.Done
    end
end

local state: State = State.Idle
for step = 1, 3 do
    state = next(state)
    println("step " .. step .. ": " .. match state
        case State.Idle then "idle"
        case State.Running then "running"
        case State.Done then "done"
    end)
end
`,
	},
	{
		id: 'classes',
		group: 'Intermediate',
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
		id: 'interfaces',
		group: 'Intermediate',
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
		id: 'iterable',
		group: 'Intermediate',
		label: 'Custom iterables',
		blurb: 'Make your own class work in a for-in loop by answering a step closure.',
		source: `-- \`for … in\` is a contract, not a special case for tables. Implement
-- \`Iterable<T>\` and your own class works in one — and the loop variable is a
-- \`T\`, with no annotation and no cast.
--
-- The contract is a single method: \`iter()\` answers a *step closure*, and
-- each call to it gives the next value, or \`nil\` when there are no more.

class Deck implements Iterable<string>
    local cards: table<string>

    fn init(cards: table<string>)
        self.cards = cards
    end

    fn iter() -> fn() -> string?
        -- The cursor lives in the closure, so two loops over the same deck
        -- each get their own.
        local cursor: integer = 0

        return fn()
            cursor = cursor + 1
            if cursor > #self.cards then
                return nil
            end
            return self.cards[cursor]
        end
    end
end

local deck: Deck = Deck({"ace", "king", "queen"})

for card in deck do
    println(card)
end

-- A step closure is an ordinary value, so a generator needs no class at all.
fn upTo(limit: integer) -> fn() -> integer?
    local n: integer = 0

    return fn()
        n = n + 1
        if n > limit then
            return nil
        end
        return n * n
    end
end

for square in upTo(5) do
    print(square .. " ")
end
println("")

-- \`Iterable2<K, V>\` is the pair version: its step closure returns two values,
-- and the loop takes two variables.
class Inventory implements Iterable2<string, integer>
    local names: table<string>
    local counts: table<integer>

    fn init()
        self.names = {}
        self.counts = {}
    end

    fn add(name: string, count: integer)
        self.names[#self.names + 1] = name
        self.counts[#self.counts + 1] = count
    end

    fn iter() -> fn() -> (string?, integer?)
        local cursor: integer = 0

        return fn()
            cursor = cursor + 1
            if cursor > #self.names then
                return nil, nil
            end
            return self.names[cursor], self.counts[cursor]
        end
    end
end

local bag: Inventory = Inventory()
bag.add("rope", 2)
bag.add("torch", 5)
bag.add("ration", 9)

for item, count in bag do
    println(String.padEnd(item, 8) .. count)
end

-- A coroutine is an \`Iterable<T>\` too, which is why the same loop drives one.
-- Write the sequence as a loop and let \`yield\` hand the values out.
fn evens(limit: integer) -> Coroutine<integer>
    return Coroutine.create(fn(co)
        for n = 2, limit, 2 do
            co.yield(n)
        end
    end)
end

for n in evens(10) do
    print(n .. " ")
end
println("")
`,
	},
	{
		id: 'generics',
		group: 'Intermediate',
		label: 'Generics and bounds',
		blurb: 'Type parameters on functions and classes, and bounds that say what a T has.',
		source: `-- Type parameters, on functions, classes and interfaces. A \`T\` is rigid: it
-- stands for whatever the caller picked, and matches only itself.

fn filter<T>(items: table<T>, keep: fn(T) -> boolean) -> table<T>
    local out: table<T> = {}

    for item in items do
        if keep(item) then
            out[#out + 1] = item
        end
    end

    return out
end

fn map<T, U>(items: table<T>, f: fn(T) -> U) -> table<U>
    local out: table<U> = {}

    for item in items do
        out[#out + 1] = f(item)
    end

    return out
end

local nums: table<integer> = {1, 2, 3, 4, 5, 6}

-- \`x => x * 2\` is the arrow form of \`fn(x) return x * 2 end\`; the parameter
-- type comes from the function being called, so it needs no annotation.
local evens: table<integer> = filter(nums, x => x % 2 == 0)
local labels: table<string> = map(evens, x => "#" .. x)

println(String.join(", ", labels))

-- A generic class. The type argument is part of the type, so \`box.get()\`
-- below is a \`string\` and nothing has to be cast.
class Box<T>
    local value: T

    fn init(value: T)
        self.value = value
    end

    fn get() -> T
        return self.value
    end

    fn replace(next: T) -> Box<T>
        return Box<T>(next)
    end
end

local greeting: Box<string> = Box<string>("hello")
println(greeting.get() .. " / " .. greeting.replace("goodbye").get())

-- Uncomment to see the checker refuse it: a \`Box<string>\` is not a \`Box<integer>\`.
-- local wrong: Box<integer> = greeting

-- A bare \`<T>\` lets a body move a value around and nothing else. A *bound*
-- says what the parameter has — and is checked in both directions: the body
-- may use what the bound declares, and every call site must satisfy it.
interface Named
    fn name() -> string
end

class Person implements Named
    local who: string

    fn init(who: string)
        self.who = who
    end

    fn name() -> string
        return self.who
    end
end

fn introduce<T: Named>(subject: T) -> string
    return "this is " .. subject.name()      -- \`Named\` promises \`name()\`
end

println(introduce(Person("Ada")))
-- println(introduce(42))                    -- ERROR: integer has no \`name\`

-- Operator bounds are the ones that pay off most: they are what makes
-- arithmetic on a type parameter possible at all. (The built-in \`integer\`
-- does not implement \`OpAdd\` — the operator interfaces are for classes, and
-- a bound is satisfied by implementing one.)
class Money implements OpAdd<Money, Money>, OpToString
    local cents: integer

    fn init(cents: integer)
        self.cents = cents
    end

    fn add(other: Money) -> Money
        return Money(self.cents + other.cents)
    end

    fn toString() -> string
        return String.format("%d.%02d", self.cents / 100, self.cents % 100)
    end
end

fn total<T: OpAdd<T, T>>(items: table<T>, zero: T) -> T
    local sum: T = zero
    for item in items do
        sum = sum + item          -- allowed only because of the bound
    end
    return sum
end

println(total({Money(350), Money(125), Money(1200)}, Money(0)))

-- A generic class with a bound, and a generic interface implemented by one.
interface Store<T>
    fn put(item: T) -> nil
    fn all() -> table<T>
end

class Registry<T: Named> implements Store<T>
    local items: table<T>

    fn init()
        self.items = {}
    end

    fn put(item: T) -> nil
        self.items[#self.items + 1] = item
    end

    fn all() -> table<T>
        return self.items
    end

    fn names() -> string
        return String.join(", ", map(self.items, person => person.name()))
    end
end

local people: Registry<Person> = Registry<Person>()
people.put(Person("Ada"))
people.put(Person("Grace"))
println(#people.all() .. " people: " .. people.names())
`,
	},
	{
		id: 'coroutines',
		group: 'Intermediate',
		label: 'Coroutines: generators',
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
		id: 'operators',
		group: 'Advanced',
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
		id: 'bitwise',
		group: 'Advanced',
		label: 'Bitwise operators',
		blurb: 'The six Lua 5.3 operators, a permission flag set, and a colour packed into one integer.',
		source: `-- The six bitwise operators, spelled as in Lua 5.3. \`~\` is xor, because \`^\`
-- is already exponentiation; in prefix position the same \`~\` is complement.

local flags: integer = 0b1100 | 0b0011     -- or        -> 15
local common: integer = 0b1100 & 0b1010    -- and       -> 8
local toggled: integer = 0b1100 ~ 0b1010   -- xor       -> 6
local inverted: integer = ~0               -- not       -> -1
local doubled: integer = 1 << 4            -- shift     -> 16
local halved: integer = 255 >> 4           --           -> 15

println(flags .. " " .. common .. " " .. toggled .. " " .. inverted .. " " .. doubled .. " " .. halved)

-- Hex and binary literals are ordinary integers, and \`_\` groups digits.
println(0b1010 .. " " .. 10 .. " " .. 0xA .. " " .. 0xFF_80_00)

-- A flag set. \`|\` adds, \`& != 0\` tests, \`& ~x\` removes — and precedence is
-- Lua's, so the test needs no parentheses.
local READ: integer = 0b100
local WRITE: integer = 0b010
local EXEC: integer = 0b001

fn render(bits: integer) -> string
    local out: string = ""
    out = out .. (bits & READ != 0 and "r" or "-")
    out = out .. (bits & WRITE != 0 and "w" or "-")
    out = out .. (bits & EXEC != 0 and "x" or "-")
    return out
end

local mine: integer = READ | WRITE
println("mine:    " .. render(mine))

mine |= EXEC                               -- compound assignment
println("plus x:  " .. render(mine))

mine = mine & ~WRITE                       -- clear one bit
println("minus w: " .. render(mine))

println("shared:  " .. render(mine & (READ | EXEC)))
println("xor r:   " .. render(mine ~ READ))

-- Packing several small numbers into one integer is the other everyday use.
-- Shift each field into place, then mask it back out.
fn rgba(r: integer, g: integer, b: integer, a: integer) -> integer
    return (r << 24) | (g << 16) | (b << 8) | a
end

local colour: integer = rgba(0x33, 0x99, 0xFF, 0xFF)
println(String.format("packed: 0x%08X", colour))
println(String.format("r=%d g=%d b=%d a=%d",
    (colour >> 24) & 0xFF,
    (colour >> 16) & 0xFF,
    (colour >> 8) & 0xFF,
    colour & 0xFF))

-- Two rules that differ from Lua: a \`float\` is rejected rather than quietly
-- converted, and \`>>\` is a logical shift, so the sign bit is not replicated.
local f: float = 6.0
println((f as integer) & 1)                -- the cast is required
println(-1 >> 63)                          -- 1, not -1
`,
	},
	{
		id: 'wrappers',
		group: 'Advanced',
		label: 'Wrapper types',
		blurb: 'Assignable, OpIndex and OpNewIndex: a class that stands in for a built-in value.',
		source: `-- Three contracts that let a class stand in for a built-in value, in a form
-- the typechecker can still see through. These are Saule's answer to Lua's
-- \`__index\`, \`__newindex\` and the "build it from a literal" trick.

-- 1. \`Assignable<T>\` — target-typed construction. The annotation picks the
--    class, and the compiler calls its \`of\`, so a literal becomes a \`Text\`.
class Text implements Assignable<string>, OpToString, OpLen
    local raw: string

    fn init(raw: string)
        self.raw = raw
    end

    -- The one name the compiler looks for. It is static: there is no
    -- instance yet to call it on.
    static fn of(s: string) -> Text
        return Text(s)
    end

    -- Nothing is injected: a wrapper declares every method it exposes, and
    -- \`string\` has none of its own, so each calls the \`String\` class.
    fn upper() -> Text
        return Text(String.upper(self.raw))
    end

    fn words() -> table<string>
        return String.split(self.raw, " ")
    end

    fn len() -> integer
        return String.len(self.raw)
    end

    fn toString() -> string
        return self.raw
    end
end

local title: Text = "the quick brown fox"     -- runs Text.of
println(title)                                 -- OpToString
println(title.upper())
println(#title .. " characters, " .. #title.words() .. " words")

-- Conversion happens at an annotated declaration and at a parameter, and
-- nowhere else — so this is a compile error, on purpose:
--     local all: table<Text> = {"a"}
fn shout(line: Text) -> string
    return tostring(line.upper()) .. "!"
end

println(shout("parameters convert too"))

-- 2. \`OpIndex\` / \`OpNewIndex\` — full control of \`[ ]\`, both ways. Unlike
--    Lua's metamethods these are not miss handlers: a class instance has no
--    keys of its own, so the method *is* the lookup and runs on every access.
class Settings implements OpIndex<string, string>, OpNewIndex<string, string>
    local data: table<string, string>
    local writes: integer = 0

    fn init()
        self.data = {}
    end

    fn index(key: string) -> string
        return self.data[key] ?? "(unset)"
    end

    fn newIndex(key: string, value: string) -> nil
        self.writes = self.writes + 1
        self.data[String.lower(key)] = String.lower(value)   -- normalise
    end

    fn writeCount() -> integer
        return self.writes
    end
end

local settings: Settings = Settings()
settings["Theme"] = "SOLARIZED"
settings["editor"] = "Saule"

println(settings["theme"])                     -- solarized
println(settings["editor"])
println(settings["missing"])                   -- (unset), not nil
println(settings.writeCount() .. " writes went through newIndex")

-- The element type comes from \`index\`'s own return type and the key from its
-- parameter, so \`settings[42]\` is a compile error rather than a runtime one.
-- \`obj.name\` is deliberately *not* routed here: field and method names stay
-- statically resolved, so a typo is still an "unknown member" diagnostic.
`,
	},
	{
		id: 'piping',
		group: 'Advanced',
		label: 'Piping with when',
		blurb: 'when(x):stage() threads a value through ordinary free functions, checked stage by stage.',
		source: `-- \`when(value)\` starts a pipeline. Every \`:stage()\` after it calls an
-- ordinary free function with the upstream value threaded in as the first
-- argument, so a chain reads top-to-bottom instead of inside-out.

fn double(n: integer) -> integer
    return n * 2
end

fn addTen(n: integer) -> integer
    return n + 10
end

fn describe(n: integer) -> string
    return "the answer is " .. n
end

-- Exactly \`describe(addTen(double(16)))\`, read in the order it happens.
println(when(16):double():addTen():describe())

-- Stages take their own arguments after the piped one.
fn repeated(s: string, times: integer) -> string
    return String.rep(s, times)
end

fn wrap(s: string, left: string, right: string) -> string
    return left .. s .. right
end

println(when("ab"):repeated(3):wrap("<", ">"))

-- The chain is typechecked stage by stage: each stage's first parameter must
-- accept what the one before it returned. Try swapping two lines below and
-- the program stops compiling rather than failing at runtime.
fn lines(s: string) -> table<string>
    return String.split(s, ",")
end

fn trimAll(items: table<string>) -> table<string>
    return Iter.map(items, item => String.trim(item))
end

fn longest(items: table<string>) -> string
    local best: string = ""
    for item in items do
        if String.len(item) > String.len(best) then
            best = item
        end
    end
    return best
end

println(when("ada , grace,  hopper "):lines():trimAll():longest())

-- A generic stage takes its type argument where the call is written.
fn keep<T>(items: table<T>, pred: fn(T) -> boolean) -> table<T>
    return Iter.filter(items, pred)
end

fn squares(items: table<integer>) -> table<integer>
    return Iter.map(items, n => n * n)
end

fn count<T>(items: table<T>) -> integer
    return #items
end

local evenSquares: integer = when({1, 2, 3, 4, 5, 6, 7, 8, 9, 10})
    :keep<integer>(n => n % 2 == 0)
    :squares()
    :count()

println(evenSquares .. " even squares")

-- Two rules worth knowing: a chain needs at least one stage (a bare
-- \`when(x)\` is a parse error, so the keyword always means a pipeline), and
-- stages are free functions — class methods and lambdas are not pipeable.
`,
	},
	{
		id: 'trailingblocks',
		group: 'Advanced',
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
		id: 'streams',
		group: 'Advanced',
		label: 'Coroutines: lazy pipelines',
		blurb: 'Stream combinators that are themselves coroutines — map, filter and take over an infinite source.',
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
		group: 'Advanced',
		label: 'Coroutines: stackful',
		blurb: 'A tokenizer that yields from two frames below the coroutine body.',
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
		group: 'Advanced',
		label: 'Coroutines: cooperative tasks',
		blurb: 'Four tasks interleaved on one thread, round-robin — one of which fails without taking the others down.',
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
