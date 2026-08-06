local n = 1000000
local t = {}
for i = 1, n do
	table.insert(t, i)
end
local s = 0
for i = 1, n do
	s = s + t[i]
end
print(s)
