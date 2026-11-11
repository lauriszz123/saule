-- The `Math` module: transcendentals, rounding and the integer helpers.
--
-- `mandel` measures float arithmetic the compiler can see through — add and
-- multiply, inline. This measures the calls: a library call per operation,
-- thirteen of them per iteration, which is what numeric code that does more
-- than add actually looks like.
--
-- Both runtimes hand `sin`, `cos`, `exp` and `log` to the platform's libm,
-- so the arithmetic is the same to the last bit and the printed accumulator
-- is a real cross-check rather than an approximate one.
--
-- `round`, `sign` and `clamp` are Saule's; Lua has no such functions, so
-- they are written out. Every value they see here is positive, where
-- `floor(x + 0.5)` agrees with Saule's round-half-away-from-zero.
local function round(x)
	return math.floor(x + 0.5)
end

local function sign(x)
	if x > 0 then
		return 1
	elseif x < 0 then
		return -1
	end
	return 0
end

local function clamp(v, lo, hi)
	if v < lo then
		return lo
	elseif v > hi then
		return hi
	end
	return v
end

local n = 1000000
local acc = 0.0
local whole = 0

for i = 1, n do
	local x = (i % 1000) / 250.0 + 0.001

	acc = acc + math.sqrt(x)
	acc = acc + math.sin(x) * math.cos(x)
	acc = acc + math.tan(x / 8.0)
	acc = acc + math.atan(x)
	acc = acc + math.exp(-x)
	acc = acc + math.log(x + 1.0, 2.0)
	acc = acc + x ^ 1.5
	acc = acc + math.fmod(x, 0.75)

	whole = whole + math.floor(x) + math.ceil(x) + round(x)
	whole = whole + math.abs(i % 7 - 3) + sign(x - 2.0)
	whole = whole + math.min(i % 13, 5) + math.max(i % 11, 4)
	whole = whole + clamp(i % 100, 10, 90)
end

print(string.format("%.6f %d", acc, whole))
