-- A lazy pipeline: three coroutines, each pulling from the one before.
--
-- Where `coroutine` measures one switch, this measures them nested. Every
-- value the sink sees has crossed three suspension boundaries, and each
-- stage is itself suspended while the stage below it runs — so the runtime
-- is holding a stack of live coroutines and transferring control down and
-- back up the chain for each element.
local function naturals(n)
	return coroutine.wrap(function()
		for i = 1, n do
			coroutine.yield(i)
		end
	end)
end

-- Each combinator is a coroutine that drives its own source: a coroutine
-- resuming a coroutine, which is what makes the chain lazy — nothing runs
-- until the sink asks.
local function scaled(src, k)
	return coroutine.wrap(function()
		for v in src do
			coroutine.yield(v * k)
		end
	end)
end

local function sieved(src)
	return coroutine.wrap(function()
		for v in src do
			if v % 3 ~= 0 then
				coroutine.yield(v)
			end
		end
	end)
end

local total = 0
for pass = 1, 60 do
	for v in sieved(scaled(naturals(25000), 2)) do
		total = total + v
	end
end
print(total)
