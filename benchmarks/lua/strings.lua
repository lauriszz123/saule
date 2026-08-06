local n = 200000
local total = 0
for i = 1, n do
	local s = "item-" .. i .. "-tail"
	total = total + #s
end
print(total)
