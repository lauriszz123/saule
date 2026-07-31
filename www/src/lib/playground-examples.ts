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
	/** One-line description shown under the picker. */
	blurb: string;
	source: string;
}

export const EXAMPLES: Example[] = [
	{
		id: 'hello',
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
		label: 'Integer vs float',
		blurb: 'Lua’s single number type, split in two. Mixing them is a compile error.',
		source: `-- Saule splits Lua's \`number\` into two distinct types.
local health: integer = 100
local speed: float = 3.14

println("health: ", health)
println("speed:  ", speed)

-- Conversions are explicit, never implicit.
println(float(health) / 3.0)
println(int(speed))

-- Try uncommenting this line — it will not compile:
-- local broken: integer = 3.14
`,
	},
	{
		id: 'classes',
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
];

export const DEFAULT_EXAMPLE = EXAMPLES[0]!;
