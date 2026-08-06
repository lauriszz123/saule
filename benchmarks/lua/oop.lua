local Point = {}
Point.__index = Point

function Point.new(x, y)
	return setmetatable({ x = x, y = y }, Point)
end

function Point:getX()
	return self.x
end

function Point:getY()
	return self.y
end

function Point:move(dx, dy)
	self.x = self.x + dx
	self.y = self.y + dy
end

local n = 1000000
local p = Point.new(0.0, 0.0)
for i = 1, n do
	p:move(1.0, 2.0)
end
print(p:getX() + p:getY())
