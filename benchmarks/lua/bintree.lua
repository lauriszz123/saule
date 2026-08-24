-- Binary search tree: build one, then walk it.
--
-- Allocation and pointer chasing rather than arithmetic — every `insert`
-- descends through instances, and every step is a nullable field read, a
-- comparison and a method call on a fresh receiver.
local Node = {}
Node.__index = Node

function Node.new(k)
	return setmetatable({ key = k, left = nil, right = nil }, Node)
end

function Node:insert(k)
	if k < self.key then
		if self.left == nil then
			self.left = Node.new(k)
		else
			self.left:insert(k)
		end
	else
		if self.right == nil then
			self.right = Node.new(k)
		else
			self.right:insert(k)
		end
	end
end

function Node:sum()
	local s = self.key
	if self.left ~= nil then
		s = s + self.left:sum()
	end
	if self.right ~= nil then
		s = s + self.right:sum()
	end
	return s
end

function Node:depth()
	local l = 0
	local r = 0
	if self.left ~= nil then
		l = self.left:depth()
	end
	if self.right ~= nil then
		r = self.right:depth()
	end
	if l > r then
		return l + 1
	end
	return r + 1
end

local n = 200000
local seed = 12345
seed = (seed * 1103515245 + 12345) % 2147483648
local root = Node.new(seed % 1000000)
for i = 2, n do
	seed = (seed * 1103515245 + 12345) % 2147483648
	root:insert(seed % 1000000)
end
print(root:sum() .. " " .. root:depth())
