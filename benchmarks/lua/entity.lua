-- An entity update loop over a mixed-subclass collection.
--
-- The shape most application code actually has: a heterogeneous list walked
-- every tick, one virtual call per element landing on a different override
-- each time, each override reading and writing its own fields. The call
-- sites here are genuinely polymorphic, so an inline cache that assumed
-- monomorphism would miss on them.
local Entity = {}
Entity.__index = Entity

function Entity.new(x, y, hp)
	return setmetatable({ x = x, y = y, hp = hp }, Entity)
end

function Entity:update(dt)
	self.x = self.x + dt
end

function Entity:score()
	return self.hp
end

local Walker = setmetatable({}, { __index = Entity })
Walker.__index = Walker

function Walker.new(x, y, hp)
	return setmetatable({ x = x, y = y, hp = hp }, Walker)
end

function Walker:update(dt)
	self.x = self.x + dt
	self.y = self.y + dt * 0.5
	self.hp = self.hp - 1
end

function Walker:score()
	return self.hp * 2
end

local Flyer = setmetatable({}, { __index = Entity })
Flyer.__index = Flyer

function Flyer.new(x, y, hp)
	return setmetatable({ x = x, y = y, hp = hp, alt = 1.0 }, Flyer)
end

function Flyer:update(dt)
	self.x = self.x + dt * 2.0
	self.alt = self.alt + dt
	if self.alt > 100.0 then
		self.alt = 0.0
		self.hp = self.hp - 2
	end
end

function Flyer:score()
	return self.hp * 3 + math.floor(self.alt)
end

local n = 6000
local ticks = 120
local es = {}
for i = 1, n do
	local m = i % 3
	if m == 0 then
		table.insert(es, Entity.new(i + 0.0, 0.0, 1000))
	elseif m == 1 then
		table.insert(es, Walker.new(i + 0.0, 0.0, 1000))
	else
		table.insert(es, Flyer.new(i + 0.0, 0.0, 1000))
	end
end

for t = 1, ticks do
	for i = 1, n do
		es[i]:update(0.25)
	end
end

local total = 0
for i = 1, n do
	total = total + es[i]:score()
end
print(total)
