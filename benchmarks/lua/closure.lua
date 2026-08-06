local n = 1000000
local add = function(a, b) return a + b end
local s = 0
for i = 1, n do
	s = add(s, i)
end
print(s)
