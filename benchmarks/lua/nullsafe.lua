-- Optional chaining: `?.`, `?[ ]`, `??` and `!` down a three-deep path.
--
-- A language feature with no Lua counterpart, measured against what Lua
-- programmers write instead — an `and` / `or` chain. The data is built so
-- every link is missing some of the time, meaning the short-circuit is taken
-- on a genuinely unpredictable branch rather than never or always.
--
-- `x and x.y or default` is the idiom `??` replaces. It behaves the same
-- here because every value on the path is a table or a non-zero integer, and
-- Lua counts 0 as true — the one case where the two disagree is absent.
local Inner = {}
Inner.__index = Inner

function Inner.new(v)
	return setmetatable({ v = v }, Inner)
end

local Mid = {}
Mid.__index = Mid

function Mid.new(inner, tags)
	return setmetatable({ inner = inner, tags = tags }, Mid)
end

local Outer = {}
Outer.__index = Outer

function Outer.new(mid)
	return setmetatable({ mid = mid }, Outer)
end

local n = 30000
local rows = {}
for i = 1, n do
	local m = i % 5
	if m == 0 then
		-- No `mid` at all: the chain stops at the first link.
		table.insert(rows, Outer.new(nil))
	elseif m == 1 then
		-- A `mid`, but nothing under it.
		table.insert(rows, Outer.new(Mid.new(nil, nil)))
	elseif m == 2 then
		-- Present all the way down, but the leaf itself is nil.
		table.insert(rows, Outer.new(Mid.new(Inner.new(nil), {})))
	else
		table.insert(rows, Outer.new(Mid.new(Inner.new(i), { i, i + 1 })))
	end
end

local sum = 0
local tagged = 0
local forced = 0
for pass = 1, 70 do
	for i = 1, n do
		local row = rows[i]
		-- Three safe hops and a fallback, as one expression.
		sum = sum + ((row.mid and row.mid.inner and row.mid.inner.v) or -1)
		-- Safe index into a table that may not be there.
		tagged = tagged + ((row.mid and row.mid.tags and row.mid.tags[1]) or 0)
		-- The path already known to be whole.
		if row.mid ~= nil and row.mid.inner ~= nil and row.mid.inner.v ~= nil then
			forced = forced + row.mid.inner.v
		end
	end
end

print(sum .. " " .. tagged .. " " .. forced)
