-- A stack machine, executed by a dispatch over an integer opcode.
--
-- The inner loop of any interpreter, rule engine or state machine written in
-- the language itself: fetch an instruction object, dispatch on its tag,
-- mutate a stack. Lua has no enum or `match`, so the idiomatic equivalent is
-- integer tags and an if-chain — which is exactly the comparison being made.
local Op = {
	Push = 1,
	Add = 2,
	Sub = 3,
	Mul = 4,
	Dup = 5,
	Drop = 6,
	Loop = 7,
	Halt = 8,
}

local Instr = {}
Instr.__index = Instr

function Instr.new(o, a)
	return setmetatable({ op = o, arg = a }, Instr)
end

local Machine = {}
Machine.__index = Machine

function Machine.new()
	return setmetatable({ stack = {}, sp = 0 }, Machine)
end

function Machine:push(v)
	self.sp = self.sp + 1
	self.stack[self.sp] = v
end

function Machine:pop()
	local v = self.stack[self.sp] or 0
	self.sp = self.sp - 1
	return v
end

function Machine:run(prog)
	local pc = 1
	local steps = 0
	local n = #prog
	while pc <= n do
		local ins = prog[pc]
		steps = steps + 1
		local o = ins.op
		if o == Op.Push then
			self:push(ins.arg)
		elseif o == Op.Add then
			local b = self:pop()
			local a = self:pop()
			self:push(a + b)
		elseif o == Op.Sub then
			local b = self:pop()
			local a = self:pop()
			self:push(a - b)
		elseif o == Op.Mul then
			local b = self:pop()
			local a = self:pop()
			self:push((a * b) % 1000003)
		elseif o == Op.Dup then
			local a = self:pop()
			self:push(a)
			self:push(a)
		elseif o == Op.Drop then
			local d = self:pop()
		elseif o == Op.Loop then
			local c = self:pop()
			if c > 0 then
				self:push(c - 1)
				pc = ins.arg
			end
		elseif o == Op.Halt then
			return steps
		end
		pc = pc + 1
	end
	return steps
end

-- Counted loop body: push a counter, then grind arithmetic on it
-- until it runs out.
local prog = {}
table.insert(prog, Instr.new(Op.Push, 400000))
table.insert(prog, Instr.new(Op.Dup, 0))
table.insert(prog, Instr.new(Op.Push, 7))
table.insert(prog, Instr.new(Op.Mul, 0))
table.insert(prog, Instr.new(Op.Push, 3))
table.insert(prog, Instr.new(Op.Add, 0))
table.insert(prog, Instr.new(Op.Push, 5))
table.insert(prog, Instr.new(Op.Sub, 0))
table.insert(prog, Instr.new(Op.Drop, 0))
table.insert(prog, Instr.new(Op.Loop, 1))
table.insert(prog, Instr.new(Op.Halt, 0))

local m = Machine.new()
print(m:run(prog))
