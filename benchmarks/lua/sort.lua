local n = 200000
local t = {}
local seed = 12345
for i = 1, n do
	seed = (seed * 1103515245 + 12345) % 2147483648
	table.insert(t, seed)
end
table.sort(t, function(a, b) return a < b end)
print(t[1] .. " " .. t[n])
