-- A recursive-descent JSON scanner.
--
-- Parsing is the branchiest everyday workload there is: one character read
-- and a multi-way dispatch per step, method calls on a receiver carrying the
-- cursor, and deep recursion through nested structure. It leans on
-- single-character `string.sub`, which is what any hand-written scanner does.
local Parser = {}
Parser.__index = Parser

function Parser.new(s)
	return setmetatable({ src = s, pos = 1, n = #s }, Parser)
end

function Parser:peek()
	if self.pos > self.n then
		return ""
	end
	return string.sub(self.src, self.pos, self.pos)
end

function Parser:skipWs()
	while self.pos <= self.n and string.sub(self.src, self.pos, self.pos) == " " do
		self.pos = self.pos + 1
	end
end

-- Every value folds into one running checksum, so the whole parse has a
-- single comparable result without building a tree the benchmark would
-- then have to walk.
function Parser:value()
	self:skipWs()
	local c = self:peek()
	if c == "{" then
		return self:object()
	elseif c == "[" then
		return self:array()
	elseif c == "\"" then
		return self:str()
	end
	return self:number()
end

function Parser:object()
	self.pos = self.pos + 1
	local acc = 1.0
	while true do
		self:skipWs()
		local c = self:peek()
		if c == "}" then
			self.pos = self.pos + 1
			return acc
		end
		if c == "," then
			self.pos = self.pos + 1
		else
			acc = acc + self:str()
			self:skipWs()
			self.pos = self.pos + 1
			acc = acc + self:value()
		end
	end
	return acc
end

function Parser:array()
	self.pos = self.pos + 1
	local acc = 2.0
	while true do
		self:skipWs()
		local c = self:peek()
		if c == "]" then
			self.pos = self.pos + 1
			return acc
		end
		if c == "," then
			self.pos = self.pos + 1
		else
			acc = acc + self:value()
		end
	end
	return acc
end

function Parser:str()
	self.pos = self.pos + 1
	local len = 0
	while self.pos <= self.n and string.sub(self.src, self.pos, self.pos) ~= "\"" do
		len = len + 1
		self.pos = self.pos + 1
	end
	self.pos = self.pos + 1
	return len + 0.0
end

function Parser:number()
	local neg = false
	if self:peek() == "-" then
		neg = true
		self.pos = self.pos + 1
	end
	local acc = 0
	local digits = 0
	while self.pos <= self.n do
		local b = string.byte(self.src, self.pos) or 0
		if b >= 48 and b <= 57 then
			acc = acc * 10 + (b - 48)
			digits = digits + 1
			self.pos = self.pos + 1
		else
			break
		end
	end
	if digits == 0 then
		self.pos = self.pos + 1
		return 0.0
	end
	if neg then
		acc = -acc
	end
	return acc + 0.0
end

local function doc(records)
	local parts = {}
	table.insert(parts, "[")
	local seed = 24680
	for i = 1, records do
		if i > 1 then
			table.insert(parts, ",")
		end
		seed = (seed * 1103515245 + 12345) % 2147483648
		table.insert(parts, "{\"id\": ")
		table.insert(parts, "" .. (seed % 10000))
		table.insert(parts, ", \"name\": \"item")
		table.insert(parts, "" .. (seed % 97))
		table.insert(parts, "\", \"tags\": [")
		table.insert(parts, "" .. (seed % 5))
		table.insert(parts, ", ")
		table.insert(parts, "" .. (seed % 7))
		table.insert(parts, "], \"ok\": ")
		table.insert(parts, "" .. (seed % 2))
		table.insert(parts, "}")
	end
	table.insert(parts, "]")
	return table.concat(parts, "")
end

local text = doc(20000)
local total = 0.0
for pass = 1, 3 do
	local p = Parser.new(text)
	total = total + p:value()
end
print(string.format("%.1f %d", total, #text))
