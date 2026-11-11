-- Lua-style patterns: `match` with captures, `gmatch`, `gsub`, `find`.
--
-- Log-line scanning, the job patterns exist for. Each line is matched
-- against an anchored three-capture date, scanned word by word, searched for
-- a keyed field and then rewritten with every number masked — four passes of
-- the pattern engine over the same string.
--
-- Lua's patterns run over bytes where Saule's run over codepoints, and
-- Saule's `%a` is Unicode-aware. The input here is pure ASCII so the two
-- agree exactly; on non-ASCII text they are answering different questions
-- and would not be comparable.
local function padStart(s, width, fill)
	if #s >= width then
		return s
	end
	return string.rep(fill, width - #s) .. s
end

local function lines(n)
	local levels = { "INFO", "WARN", "ERROR", "DEBUG" }
	local users = { "ada", "grace", "alan", "edsger", "barbara" }
	local out = {}
	local seed = 24601
	for i = 1, n do
		seed = (seed * 1103515245 + 12345) % 2147483648
		local lvl = levels[seed % 4 + 1]
		local who = users[(seed // 4) % 5 + 1]
		out[i] = "2026-11-" .. padStart("" .. (seed % 28 + 1), 2, "0")
			.. " " .. lvl .. " user=" .. who
			.. " code=" .. (seed % 10000) .. " took=" .. (seed % 500) .. "ms"
	end
	return out
end

local src = lines(1000)
local days = 0
local words = 0
local letters = 0
local codes = 0
local masked = 0
local sample = ""

for pass = 1, 140 do
	for i = 1, #src do
		local line = src[i]

		-- Anchored, three captures.
		local y, m, d = string.match(line, "^(%d+)-(%d+)-(%d+)")
		if y ~= nil then
			days = days + (tonumber(d) or 0)
		end

		-- Every word, one at a time.
		for w in string.gmatch(line, "%a+") do
			words = words + 1
			letters = letters + #w
		end

		-- One keyed field, by capture.
		local code = string.match(line, "code=(%d+)")
		if code ~= nil then
			codes = (codes + (tonumber(code) or 0)) % 1000000007
		end

		-- Rewrite: mask every run of digits.
		local out, hits = string.gsub(line, "%d+", "#")
		masked = masked + hits
		sample = out
	end
end

print(days .. " " .. words .. " " .. letters .. " " .. codes .. " " .. masked)
print(sample)
