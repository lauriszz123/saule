-- Word frequency: split a document, count into a map, rank the results.
--
-- The most common shape in everyday code and the one that touches the most
-- runtime at once — string slicing, string-keyed hashing, map update in a
-- loop, then a comparator sort over the collected pairs.
local function text(words)
	local vocab = {
		"alpha", "beta", "gamma", "delta", "epsilon",
		"zeta", "eta", "theta", "iota", "kappa",
		"lambda", "mu", "nu", "xi", "omicron",
	}
	local parts = {}
	local seed = 987654321
	for i = 1, words do
		seed = (seed * 1103515245 + 12345) % 2147483648
		table.insert(parts, vocab[seed % 15 + 1])
	end
	return table.concat(parts, " ")
end

local doc = text(120000)
local counts = {}
local n = #doc

local start = 1
local i = 1
while i <= n + 1 do
	local atEnd = i > n
	if atEnd or string.sub(doc, i, i) == " " then
		if i > start then
			local w = string.sub(doc, start, i - 1)
			counts[w] = (counts[w] or 0) + 1
		end
		start = i + 1
	end
	i = i + 1
end

local keys = {}
for k, v in pairs(counts) do
	table.insert(keys, k)
end
table.sort(keys, function(a, b) return a < b end)

local total = 0
local out = {}
for j = 1, #keys do
	local k = keys[j]
	local c = counts[k] or 0
	total = total + c
	table.insert(out, k .. "=" .. c)
end
print(total .. " " .. #keys .. " " .. out[1])
