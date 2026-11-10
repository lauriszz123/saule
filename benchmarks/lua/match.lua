-- `match` over an enum that carries payloads, with guards.
--
-- `interp` dispatches on a bare tag; this is the other half of pattern
-- matching — arms that destructure a variant's fields into locals, plus two
-- arms for the same variant separated only by a `when` guard, so the match
-- cannot be a jump table and has to fall through to the next arm when a
-- guard fails.
--
-- Lua has neither enums nor `match`: a table with a `tag` field and an
-- if/elseif chain that reads the payload fields by name is the same program
-- written the way Lua writes it.
local function handle(e)
	local t = e.tag
	if t == 0 then
		return e.x + e.y * 2
	elseif t == 1 then
		-- The guard, and the fall-through when it fails.
		if e.code > 100 then
			return e.code * 3
		end
		return e.code
	elseif t == 2 then
		if e.dy < 0 then
			return -e.dy
		end
		return e.dy
	end
	return 0
end

local n = 200000
local events = {}
local seed = 13579
for i = 1, n do
	seed = (seed * 1103515245 + 12345) % 2147483648
	local kind = seed % 4
	if kind == 0 then
		table.insert(events, { tag = 0, x = seed % 640, y = seed % 480 })
	elseif kind == 1 then
		table.insert(events, { tag = 1, code = seed % 200 })
	elseif kind == 2 then
		table.insert(events, { tag = 2, dy = seed % 21 - 10 })
	else
		table.insert(events, { tag = 3 })
	end
end

local total = 0
for pass = 1, 13 do
	for i = 1, n do
		total = total + handle(events[i])
	end
end
print(total)
