-- A tree of objects walked over and over, child list by child list.
--
-- What a user interface does every frame: each node holds a short list of
-- children, and laying out, ticking or drawing the tree is a loop over
-- every one of those lists, thousands of loops of a handful of steps each.
-- `entity` walks one long list and spends its time in the calls it makes;
-- here the loops themselves are the work — how much it costs to begin one,
-- and to take a step.
--
-- `ipairs` is the twin of Saule's `for … in` over a list: the idiomatic
-- loop over an array's values, in order.
local function Node(size)
	return { size = size, children = {} }
end

local function build(depth, fan, seed)
	local node = Node(seed % 17 + 1)

	if depth > 0 then
		for i = 1, fan do
			table.insert(node.children, build(depth - 1, fan, seed * 31 + i))
		end
	end

	return node
end

local function weigh(node)
	local total = node.size

	for _, child in ipairs(node.children) do
		total = total + weigh(child)
	end

	return total
end

-- 1 + 4 + 16 + … + 4096 nodes, each inner one with four children.
local root = build(6, 4, 7)
local sum = 0

for round = 1, 250 do
	sum = sum + weigh(root)
end

local row = {}

for i = 1, 48 do
	table.insert(row, Node(i % 9 + 1))
end

local weighted = 0

for round = 1, 60000 do
	for i, node in ipairs(row) do
		weighted = weighted + i * node.size
	end
end

print(sum .. " " .. weighted)
