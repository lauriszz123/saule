-- Operator overloading: `+`, `-`, `*` and `==` on a user-defined type.
--
-- Saule's answer to Lua's `__add` / `__sub` / `__mul` / `__eq` metamethods,
-- and the direct comparison for them. Each iteration is three overload
-- dispatches, three fresh instances and an `equals`, so this measures the
-- cost of routing an operator to a method plus the allocation that a
-- value-type style forces — the shape all vector and matrix code has.
--
-- Integer components on purpose: the result is exact, so the two languages
-- are compared on a value neither can round differently.
local Vec3 = {}
Vec3.__index = Vec3

function Vec3.new(x, y, z)
	return setmetatable({ x = x, y = y, z = z }, Vec3)
end

function Vec3.__add(a, b)
	return Vec3.new(a.x + b.x, a.y + b.y, a.z + b.z)
end

function Vec3.__sub(a, b)
	return Vec3.new(a.x - b.x, a.y - b.y, a.z - b.z)
end

-- Wrapped, so the components stay bounded over 300k iterations instead of
-- drifting into bignum territory in one language and not the other.
function Vec3.__mul(a, b)
	return Vec3.new((a.x * b.x) % 1000003, (a.y * b.y) % 1000003, (a.z * b.z) % 1000003)
end

function Vec3.__eq(a, b)
	return a.x == b.x and a.y == b.y and a.z == b.z
end

function Vec3.__tostring(v)
	return "<" .. v.x .. " " .. v.y .. " " .. v.z .. ">"
end

local n = 900000
local acc = Vec3.new(1, 2, 3)
local step = Vec3.new(7, 11, 13)
local unit = Vec3.new(1, 1, 1)
local hits = 0

-- Multiply before subtracting: the product can be zero, and adding `step`
-- first keeps every component positive, so `%` never has to answer the sign
-- question the two languages answer differently.
for i = 1, n do
	acc = acc * step
	acc = acc + step
	acc = acc - unit
	if acc == step then
		hits = hits + 1
	end
end

print(tostring(acc) .. " " .. hits)
