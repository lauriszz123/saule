-- Throwing and catching, at the rate a parser or validator actually throws.
--
-- Roughly one call in five fails, which is the interesting case: a `try`
-- whose body usually succeeds measures only the cost of entering the block,
-- and one that always fails measures nothing else. Two `catch` clauses, so
-- the thrown value has to be type-tested in source order, and one of the
-- throws carries an object rather than a string.
--
-- `error(msg, 0)`, not `error(msg)`: level 1 prepends "file:line:" to the
-- message, and that string formatting is not what either side is being
-- measured on. Saule's `throw` hands over the value as written, so level 0
-- is the honest twin.
local ParseError = {}
ParseError.__index = ParseError

function ParseError.new(code)
	return setmetatable({ code = code }, ParseError)
end

local function parse(tok)
	if tok % 7 == 0 then
		error("bad token", 0)
	end
	if tok % 11 == 0 then
		error(ParseError.new(tok % 100), 0)
	end
	return tok * 2
end

local n = 3000000
local ok = 0
local strs = 0
local objs = 0

for i = 1, n do
	local good, res = pcall(parse, i)
	if good then
		ok = ok + res
	elseif type(res) == "string" then
		strs = strs + 1
	else
		objs = objs + res.code
	end
end

print(ok .. " " .. strs .. " " .. objs)
