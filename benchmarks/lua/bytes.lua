-- The `Bytes` module: a binary frame scanner.
--
-- Saule separates `bytes` from `string` — a `string` is always valid UTF-8
-- and is measured in codepoints, so binary data gets its own type with its
-- own module. Lua has no such split and uses a `string` as its byte buffer,
-- which is what this twin does. The workload is the one that type exists
-- for: walk a buffer byte by byte, find a marker, slice the frames between
-- markers, splice some back together and hex-dump a header.
local function payload(records)
	local parts = {}
	local seed = 4242
	for i = 1, records do
		seed = (seed * 1103515245 + 12345) % 2147483648
		-- "FR" marks a frame; six ASCII bytes follow it.
		parts[#parts + 1] = "FR"
		parts[#parts + 1] = string.char(
			65 + seed % 26,
			65 + (seed // 26) % 26,
			97 + (seed // 676) % 26,
			48 + (seed // 7) % 10,
			48 + (seed // 70) % 10,
			45)
	end
	return table.concat(parts, "")
end

local function toHex(s)
	local out = {}
	for i = 1, #s do
		out[i] = string.format("%02x", string.byte(s, i))
	end
	return table.concat(out, "")
end

local buf = payload(6000)
local n = #buf

local total = 0
local frames = 0
local spanned = 0
local hex = ""

for pass = 1, 100 do
	-- Every byte, one at a time.
	for i = 1, n do
		total = (total + string.byte(buf, i)) % 1000000007
	end

	-- Every marker, then the frame that follows it.
	local at = string.find(buf, "FR", 1, true)
	local prev = 0
	while at ~= nil do
		frames = frames + 1
		if prev > 0 then
			local frame = string.sub(buf, prev, at - 1)
			spanned = spanned + #frame
		end
		prev = at
		at = string.find(buf, "FR", at + 1, true)
	end

	-- Splice the head back together, and hex-dump it.
	local head = string.sub(buf, 1, 8) .. string.sub(buf, 9, 16)
	hex = toHex(head)

	-- Round-trip a slice through a byte array and back to text.
	local slice = string.sub(buf, 3, 8)
	local arr = {}
	for i = 1, #slice do
		arr[i] = string.byte(slice, i)
	end
	local back = string.char(table.unpack(arr))
	spanned = spanned + #back
end

print(total .. " " .. frames .. " " .. spanned .. " " .. hex)
