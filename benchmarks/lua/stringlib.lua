-- The `String` module, across the calls everyday text handling makes.
--
-- `strings` measures `..` and `#`; this measures the library — a
-- field-extraction pipeline run over 60,000 records: trim, prefix test,
-- locate a separator, slice either side, split a list, rejoin it, case-fold,
-- substitute, pad and format. Several of these have no Lua equivalent and
-- are written out here by hand, which is the comparison that matters: not
-- function against function but the whole job, each language doing it the
-- way it actually would.
--
-- Everything is ASCII, so Lua's byte indices and Saule's codepoint indices
-- describe the same positions and the two outputs are comparable.

-- Lua has no `split`; this loop is what the standard library leaves you to
-- write. `plain = true` on `find` keeps it a literal search, which is what
-- `String.split` does.
local function split(s, sep)
	local out = {}
	local from = 1
	while true do
		local a, b = string.find(s, sep, from, true)
		if not a then
			out[#out + 1] = string.sub(s, from)
			break
		end
		out[#out + 1] = string.sub(s, from, a - 1)
		from = b + 1
	end
	return out
end

local function padStart(s, width, fill)
	if #s >= width then
		return s
	end
	return string.rep(fill, width - #s) .. s
end

local function rows(n)
	local vocab = { "alpha", "beta", "gamma", "delta", "epsilon" }
	local out = {}
	local seed = 777
	for i = 1, n do
		seed = (seed * 1103515245 + 12345) % 2147483648
		local a = vocab[seed % 5 + 1]
		local b = vocab[(seed // 5) % 5 + 1]
		local c = vocab[(seed // 25) % 5 + 1]
		out[#out + 1] = "  user-" .. padStart("" .. (i % 10000), 4, "0") .. ":" .. a .. "," .. b .. "," .. c .. "  "
	end
	return out
end

local src = rows(2000)
local chars = 0
local users = 0
local betas = 0
local last = ""

for pass = 1, 240 do
	for i = 1, #src do
		-- `match` with a lazy middle capture is the usual Lua trim.
		local t = string.match(src[i], "^%s*(.-)%s*$")
		if string.sub(t, 1, 5) == "user-" then
			users = users + 1
		end

		local colon = string.find(t, ":", 1, true) or 0
		local name = string.sub(t, 1, colon - 1)
		local rest = string.sub(t, colon + 1)

		local parts = split(rest, ",")
		local joined = table.concat(parts, "|")
		-- "beta" holds no pattern metacharacters, so `gsub` is a literal
		-- replace here, as `String.replace` is.
		local marked = (string.gsub(joined, "beta", "BETA"))
		if string.find(marked, "BETA", 1, true) ~= nil then
			betas = betas + 1
		end

		chars = chars + #string.upper(name) + #string.lower(marked)
		if string.sub(marked, -5) == "delta" then
			chars = chars + 1
		end
		last = string.format("%s/%d/%s", padStart(name, 14, "."), #parts, marked)
	end
end

print(chars .. " " .. users .. " " .. betas .. " " .. last)
