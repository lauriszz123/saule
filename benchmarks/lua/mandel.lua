local w = 200
local h = 200
local maxIter = 50
local count = 0
for py = 0, h - 1 do
	local y0 = py / h * 2.0 - 1.0
	for px = 0, w - 1 do
		local x0 = px / w * 3.0 - 2.0
		local x = 0.0
		local y = 0.0
		local it = 0
		while x * x + y * y <= 4.0 and it < maxIter do
			local xt = x * x - y * y + x0
			y = 2.0 * x * y + y0
			x = xt
			it = it + 1
		end
		if it == maxIter then
			count = count + 1
		end
	end
end
print(count)
