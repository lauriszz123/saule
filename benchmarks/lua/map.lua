local n = 300000
local m = {}
for i = 1, n do
	m["key" .. i] = i
end
local s = 0
for i = 1, n do
	s = s + (m["key" .. i] or 0)
end
print(s)
