-- Stackful yield: an in-order tree walk that suspends mid-recursion.
--
-- The property that separates a real coroutine from a callback-driven
-- generator. `yield` fires seventeen frames deep inside `walk`, so the
-- runtime has to park that whole call stack and restore it on the next
-- resume. A generator built on callbacks cannot do this at all; one built on
-- an explicit worklist has to reify the stack by hand.
local Node = {}
Node.__index = Node

function Node.new(v, left, right)
	return setmetatable({ v = v, left = left, right = right }, Node)
end

-- Lua's `coroutine.yield` suspends whichever coroutine is running, so the
-- recursion needs nothing passed down; Saule's typed yielder travels as an
-- argument. Either way the whole call stack is parked.
function Node:walk()
	if self.left then
		self.left:walk()
	end
	coroutine.yield(self.v)
	if self.right then
		self.right:walk()
	end
end

local function build(lo, hi)
	if lo > hi then
		return nil
	end
	local mid = (lo + hi) // 2
	return Node.new(mid, build(lo, mid - 1), build(mid + 1, hi))
end

local root = build(1, 120000)
local total = 0
for pass = 1, 16 do
	local walked = coroutine.wrap(function()
		root:walk()
	end)
	for v in walked do
		total = total + v
	end
end
print(total)
