-- The `Iter` module: the whole combinator set over one list, per pass.
--
-- Saule ships what Lua leaves to the user. Every one of these is a loop with
-- a callback per element, so this measures the thing functional collection
-- code lives on — the cost of calling a lambda a few hundred thousand times
-- and of allocating the intermediate table each stage hands to the next.
--
-- Against hand-written Lua loops this is the fairest comparison in the suite
-- and the least flattering: this twin fuses nothing either, but it pays no
-- call through a function-typed slot, so the gap is close to the price of
-- the callback itself.
local function map(t, f)
	local out = {}
	for i = 1, #t do
		out[i] = f(t[i])
	end
	return out
end

local function filter(t, pred)
	local out = {}
	for i = 1, #t do
		if pred(t[i]) then
			out[#out + 1] = t[i]
		end
	end
	return out
end

local function reduce(t, init, f)
	local acc = init
	for i = 1, #t do
		acc = f(acc, t[i])
	end
	return acc
end

local function any(t, pred)
	for i = 1, #t do
		if pred(t[i]) then
			return true
		end
	end
	return false
end

local function all(t, pred)
	for i = 1, #t do
		if not pred(t[i]) then
			return false
		end
	end
	return true
end

local function count(t, pred)
	local n = 0
	for i = 1, #t do
		if pred(t[i]) then
			n = n + 1
		end
	end
	return n
end

local function find(t, pred)
	for i = 1, #t do
		if pred(t[i]) then
			return t[i]
		end
	end
	return nil
end

local function findIndex(t, pred)
	for i = 1, #t do
		if pred(t[i]) then
			return i
		end
	end
	return nil
end

local function take(t, n)
	local out = {}
	for i = 1, math.min(n, #t) do
		out[i] = t[i]
	end
	return out
end

local function skip(t, n)
	local out = {}
	for i = n + 1, #t do
		out[#out + 1] = t[i]
	end
	return out
end

-- First appearance wins and the original order is kept, as `Iter.unique`
-- does, so the result is the same list and not merely the same set.
local function unique(t)
	local seen = {}
	local out = {}
	for i = 1, #t do
		local v = t[i]
		if not seen[v] then
			seen[v] = true
			out[#out + 1] = v
		end
	end
	return out
end

-- The key is unique within the input here, so `table.sort` not being stable
-- cannot change the answer and no decoration is needed.
local function sortBy(t, key)
	local out = {}
	for i = 1, #t do
		out[i] = t[i]
	end
	table.sort(out, function(a, b) return key(a) < key(b) end)
	return out
end

local function zipWith(a, b, f)
	local out = {}
	local n = math.min(#a, #b)
	for i = 1, n do
		out[i] = f(a[i], b[i])
	end
	return out
end

local function chunk(t, size)
	local out = {}
	local cur = {}
	for i = 1, #t do
		cur[#cur + 1] = t[i]
		if #cur == size then
			out[#out + 1] = cur
			cur = {}
		end
	end
	if #cur > 0 then
		out[#out + 1] = cur
	end
	return out
end

local function flatten(t)
	local out = {}
	for i = 1, #t do
		local inner = t[i]
		for j = 1, #inner do
			out[#out + 1] = inner[j]
		end
	end
	return out
end

local function groupBy(t, key)
	local out = {}
	for i = 1, #t do
		local k = key(t[i])
		local bucket = out[k]
		if bucket == nil then
			bucket = {}
			out[k] = bucket
		end
		bucket[#bucket + 1] = t[i]
	end
	return out
end

local nums = {}
for i = 1, 1000 do
	nums[i] = (i * 2654435761) % 100000
end

local empty = {}
local checksum = 0
for pass = 1, 600 do
	local doubled = map(nums, function(x) return x * 2 end)
	local kept = filter(doubled, function(x) return x % 3 ~= 0 end)
	checksum = (checksum + reduce(kept, 0, function(a, v) return a + v end)) % 1000000007

	if any(kept, function(x) return x > 199000 end) then
		checksum = checksum + 1
	end
	if all(kept, function(x) return x >= 0 end) then
		checksum = checksum + 2
	end
	checksum = (checksum + count(kept, function(x) return x % 5 == 0 end)) % 1000000007
	checksum = (checksum + (find(kept, function(x) return x % 2 == 1 end) or -1)) % 1000000007
	checksum = (checksum + (findIndex(kept, function(x) return x > 50000 end) or -1)) % 1000000007
	checksum = (checksum + (kept[1] or 0) + (kept[#kept] or 0)) % 1000000007

	local head = take(kept, 120)
	local rest = skip(kept, 500)
	checksum = (checksum + #head + #rest) % 1000000007

	local uniq = unique(map(nums, function(x) return x % 97 end))
	local sorted = sortBy(uniq, function(x) return x end)
	checksum = (checksum + #uniq + (sorted[1] or 0) + (sorted[#sorted] or 0)) % 1000000007

	checksum = (checksum + reduce(zipWith(head, head, function(a, b) return a + b end), 0, function(a, v) return a + v end)) % 1000000007

	local chunks = chunk(head, 7)
	local flat = flatten(chunks)
	checksum = (checksum + #chunks + #flat) % 1000000007

	-- Four buckets, read by key rather than by iteration order.
	local groups = groupBy(nums, function(x) return x % 4 end)
	for k = 0, 3 do
		checksum = (checksum + #(groups[k] or empty)) % 1000000007
	end
end

print(checksum)
