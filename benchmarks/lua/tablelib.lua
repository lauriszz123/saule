-- The `Table` module: build, splice, search, reorder, join.
--
-- `array` measures raw `table.insert` and indexing. This measures the rest
-- of the module, in the mix a collection-heavy program produces: an insert
-- at a position (which shifts every element after it), removals from the
-- middle and the end, an in-place reverse, a slice and a copy, two linear
-- searches, a comparator sort, a join, and the keys of a map pulled out and
-- ordered.
--
-- Lua ships `insert`, `remove`, `concat` and `sort`; the other six are
-- written out below, because that is what Lua leaves you to do.
local function reverse(t)
	local i, j = 1, #t
	while i < j do
		t[i], t[j] = t[j], t[i]
		i = i + 1
		j = j - 1
	end
end

local function slice(t, from, to)
	local out = {}
	for i = from, to do
		out[#out + 1] = t[i]
	end
	return out
end

local function copy(t)
	local out = {}
	for i = 1, #t do
		out[i] = t[i]
	end
	return out
end

local function indexOf(t, v)
	for i = 1, #t do
		if t[i] == v then
			return i
		end
	end
	return nil
end

local function contains(t, v)
	return indexOf(t, v) ~= nil
end

local function keysOf(m)
	local out = {}
	for k in pairs(m) do
		out[#out + 1] = k
	end
	return out
end

local function valuesOf(m)
	local out = {}
	for _, v in pairs(m) do
		out[#out + 1] = v
	end
	return out
end

local passes = 3000
local n = 1000
local checksum = 0

for pass = 1, passes do
	local t = {}
	for i = 1, n do
		table.insert(t, (i * 7919 + pass) % 10007)
	end

	-- Insert at the front: every element moves up one. Note Lua takes the
	-- position before the value; Saule takes it after.
	table.insert(t, 1, 42)
	local mid = table.remove(t, n // 2) or 0
	local tail = table.remove(t) or 0
	checksum = (checksum + mid + tail) % 1000000007

	reverse(t)

	local head = slice(t, 1, 100)
	local dup = copy(head)

	if contains(t, 5000) then
		checksum = checksum + 1
	end
	checksum = (checksum + (indexOf(t, dup[1]) or 0)) % 1000000007

	table.sort(head, function(a, b) return a < b end)
	checksum = (checksum + #table.concat(head, ",")) % 1000000007
	checksum = (checksum + #t + #dup) % 1000000007
end

-- A map, its keys lifted out and ordered. Map iteration order is not
-- specified in either language, so the keys are sorted before anything
-- depends on them.
local m = {}
for i = 1, 2000 do
	m["k" .. (i % 500)] = i
end
local keys = keysOf(m)
table.sort(keys, function(a, b) return a < b end)
local vals = valuesOf(m)
checksum = (checksum + #keys + #vals + #keys[1]) % 1000000007

print(checksum)
