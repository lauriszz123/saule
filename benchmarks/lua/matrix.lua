-- Dense float matrix multiply, the textbook triple loop.
--
-- Nested table indexing and float arithmetic in the innermost loop, which
-- is where a numeric workload actually spends itself: `c[i][j]` is two
-- bounds-checked reads before any multiply happens.
local function build(n, seedIn)
	local m = {}
	local seed = seedIn
	for i = 1, n do
		local row = {}
		for j = 1, n do
			seed = (seed * 1103515245 + 12345) % 2147483648
			table.insert(row, (seed % 1000) / 1000.0)
		end
		table.insert(m, row)
	end
	return m
end

local n = 110
local a = build(n, 12345)
local b = build(n, 67890)
local c = {}

for i = 1, n do
	local row = {}
	for j = 1, n do
		table.insert(row, 0.0)
	end
	table.insert(c, row)
end

for i = 1, n do
	local ai = a[i]
	local ci = c[i]
	for k = 1, n do
		local aik = ai[k]
		local bk = b[k]
		for j = 1, n do
			ci[j] = ci[j] + aik * bk[j]
		end
	end
end

local total = 0.0
for i = 1, n do
	total = total + c[i][i]
end
print(string.format("%.6f", total))
