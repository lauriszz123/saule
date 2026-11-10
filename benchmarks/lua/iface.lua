-- Interface dispatch across unrelated implementors.
--
-- `entity` measures an override chain — subclasses of one base. This
-- measures the other polymorphism: three classes with no common ancestor,
-- reached through a single `interface` slot. There is no shared layout to
-- exploit, so every call site has to find the method on whichever concrete
-- class turned up, in a list arranged so consecutive elements never agree.
--
-- Lua has no interfaces; three independent metatables that happen to answer
-- the same two method names is how the same program is written there.
local Square = {}
Square.__index = Square

function Square.new(s)
	return setmetatable({ s = s }, Square)
end

-- Mutates on every call, so neither runtime can hoist the result out of the
-- loop or fold it away.
function Square:area()
	self.s = (self.s + 1) % 1000
	return self.s * self.s
end

function Square:tag()
	return "sq"
end

local Rect = {}
Rect.__index = Rect

function Rect.new(w, h)
	return setmetatable({ w = w, h = h }, Rect)
end

function Rect:area()
	self.w = (self.w + 2) % 1000
	return self.w * self.h
end

function Rect:tag()
	return "rect"
end

local Ring = {}
Ring.__index = Ring

function Ring.new(r)
	return setmetatable({ r = r }, Ring)
end

function Ring:area()
	self.r = (self.r + 3) % 1000
	return (self.r * self.r * 314) // 100
end

function Ring:tag()
	return "ring"
end

local n = 6000
local ticks = 600
local shapes = {}
for i = 1, n do
	local m = i % 3
	if m == 0 then
		table.insert(shapes, Square.new(i))
	elseif m == 1 then
		table.insert(shapes, Rect.new(i, 3))
	else
		table.insert(shapes, Ring.new(i))
	end
end

local total = 0
for t = 1, ticks do
	for i = 1, n do
		total = (total + shapes[i]:area()) % 1000000007
	end
end

local tags = 0
for i = 1, n do
	tags = tags + #shapes[i]:tag()
end
print(total .. " " .. tags)
