-- Generator throughput: one coroutine, drained to exhaustion.
--
-- Isolates the context switch itself. The body does almost nothing, so what
-- the clock measures is `resume` entering the coroutine, `yield` handing a
-- value back out and the loop picking it up — a million round trips, with no
-- other work to hide them behind.
--
-- `coroutine.wrap` is the twin of Saule's `for v in gen do`: both drive the
-- coroutine to the next yield and stop at the end without an explicit check.
local function counter(n)
	return coroutine.wrap(function()
		for i = 1, n do
			coroutine.yield(i)
		end
	end)
end

local total = 0
-- Re-created each pass, so coroutine setup and teardown are in the
-- measurement too rather than amortised over one very long run.
for pass = 1, 90 do
	for v in counter(50000) do
		total = total + v
	end
end
print(total)
