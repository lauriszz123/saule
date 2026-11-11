-- Generic classes and generic functions, instantiated at several types.
--
-- The question a static language has to answer that Lua never asks: what
-- does a type parameter cost at runtime? A `Stack<T>` holds and returns a
-- `T` through the same field and the same method bodies whatever `T` is, and
-- a generic function takes a comparator it cannot know the type of. If
-- generics are erased this should land on top of the plain-table version;
-- anything slower is boxing or a dispatch that `oop` does not pay.
--
-- Lua's version is the same program with the annotations deleted — one
-- untyped stack serving both element types, which is the baseline a generic
-- is being measured against.
local Stack = {}
Stack.__index = Stack

function Stack.new()
	return setmetatable({ items = {}, top = 0 }, Stack)
end

function Stack:push(v)
	self.top = self.top + 1
	self.items[self.top] = v
end

function Stack:pop()
	if self.top == 0 then
		return nil
	end
	local v = self.items[self.top]
	self.top = self.top - 1
	return v
end

function Stack:size()
	return self.top
end

-- Generic over the element and over how to rank it. Nothing in the body can
-- look inside the value, so the comparator is the only way through.
local function pickMax(a, b, better)
	if better(a, b) then
		return a
	end
	return b
end

local n = 700000
local st = Stack.new()
local total = 0
-- Hoisted: constructing a closure per iteration is what `closure` measures,
-- and it would sit on top of what this one is for.
local better = function(x, y) return x > y end

for i = 1, n do
	st:push(i % 997)
	st:push(i % 89)
	local b = st:pop() or 0
	local a = st:pop() or 0
	total = total + pickMax(a, b, better)
end

-- A second instantiation, so the stack is exercised at more than one element
-- type in the same program.
local ws = Stack.new()
for i = 1, 2000 do
	ws:push("w" .. (i % 17))
end
local wide = ""
local wider = function(x, y) return #x > #y end
for i = 1, 2000 do
	wide = pickMax(wide, ws:pop() or "", wider)
end

print(total .. " " .. st:size() .. " " .. wide)
