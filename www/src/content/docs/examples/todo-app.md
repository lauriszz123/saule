---
title: "Todo App"
description: "A complete command-line application: argument parsing with Os.args(), JSON persistence through the json library declared in dependencies:, match over subcommands, and rendering split into its own module."
sidebar:
  order: 4
---

<!-- Generated from examples/todo-app by `npm run sync-docs`. Edit the example, not this file. -->

A complete command-line application: argument parsing with `Os.args()`, JSON persistence through the `json` library declared in `dependencies:`, `match` over subcommands, and rendering split into its own module.

[Browse this example on GitHub](https://github.com/lauriszz123/saule/tree/main/examples/todo-app)

## Run it

```sh
git clone https://github.com/lauriszz123/saule.git
cd saule/examples/todo-app
saule run -- add "write some Saule"
saule run -- list
```

## `saule.config`

```
name: "todo-app"
version: "0.1.0"
entry: "src/main.sau"
src_dirs: ["src"]
dependencies: ["../json"]
min_saule_version: "26.1"
```

## `src/main.sau`

```saule title="src/main.sau"
import * from storage
import * from render

class Main
	static fn help()
		println("TODO App Usage:")
		println("\ttodo list")
		println("\ttodo add [task] [optional-due-date YYYY-MM-DD]")
		println("\ttodo remove [number-of-task]")
		println("\ttodo done [number-of-task]")
		println("\ttodo due [number-of-task] [due-date YYYY-MM-DD]")
	end

	static fn main()
		local storage = Storage("todo.json")
		local args = Os.args()

		if not Os.exists("todo.json") then
			storage.save()
		elseif not storage.load() then
			println("error: todo.json is corrupt; refusing to overwrite")
			return
		end

		if #args == 0 then
			help()
			return
		end

		match args[1]
			case "list" then Renderer.print(storage.getAll())

			case "add" then
				local due: integer? = nil

				if args[3] != nil then
					due = Os.parsedate(args[3])
					if due == nil then
						println("invalid date: " .. args[3] .. " (expected YYYY-MM-DD)")
						return
					end
				end

				storage.add(args[2], dueDate: due)
				Renderer.print(storage.getAll())

			case "done" then
				local index: integer? = tointeger(args[2])
				if index == nil then
					println("error: invalid task number")
					return
				end

				storage.update(index, isDone: true)

			case "due" then
				local index: integer? = tointeger(args[2])

				if index == nil then
					println("error: invalid task number")
					return
				end

				if args[3] == nil then
					println("error: missing due date")
					return
				end

				local due: integer? = Os.parsedate(args[3])
				if due == nil then
					println("invalid date: " .. args[3] .. " (expected YYYY-MM-DD)")
					return
				end

				storage.update(index, dueDate: due)

			case "remove" then
				local index: integer? = tointeger(args[2])
				if index == nil then
					println("error: invalid task number")
					return
				end

				match storage.remove(index)
					case nil then printf("Task id %d does not exist\n", index)

					case task then printf("Removed task: %s\n", task)
				end

				Renderer.print(storage.getAll())

			case _ then Main.help()
		end

		storage.save()
	end
end
```

## `src/storage.sau`

```saule title="src/storage.sau"
import Json from json

export class Entry
	local todo: string
	local done: boolean
	local dueDate: integer?

	fn init(todo: string, dueDate: integer?)
		self.todo = todo
		self.done = false
		self.dueDate = dueDate
	end

	fn setTodo(todo: string)
		self.todo = todo
	end

	fn getTodo() -> string
		return self.todo
	end

	fn setDone(isDone: boolean)
		self.done = isDone
	end

	fn isDone() -> boolean
		return self.done
	end

	fn setDueDate(dueDate: integer)
		self.dueDate = dueDate
	end

	fn getDueDate() -> integer?
		return self.dueDate
	end

	fn isDue() -> boolean?
		if self.dueDate == nil then
			return
		end

		return Os.time() >= self.dueDate
	end
end

export class Storage
	local path: string
	local storage: table<Entry>

	fn init(path: string)
		self.path = path
		self.storage = {}
	end

	fn load() -> boolean
		if not Os.exists(self.path) then
			return false
		end

		local file = Io.open(self.path, IoMode.Read)

		-- Decoded JSON is plain tables, not `Entry` instances — the rows
		-- below are validated field by field and fed through `Entry(...)`.
		local data = Json.decode(file?.read()) as table<any>
		file?.close()

		if not data then
			return false
		end

		if type(data) != "table" then
			return false
		end

		for _, entry in data do
			if type(entry) != "table" then
				return false
			end

			-- `as` is the checked cast out of `any`: it yields nil when the
			-- stored value has the wrong shape, so a nil here means the file
			-- is malformed.
			local todo = entry.todo as string
			if todo == nil then
				return false
			end

			local done = entry.done as boolean
			if done == nil then
				return false
			end

			local newEntry = Entry(todo, entry.dueDate as integer)
			newEntry.setDone(done)
			Table.insert(self.storage, newEntry)
		end

		return true
	end

	fn save()
		local data: table = {}

		for _, entry in self.storage do
			Table.insert(data, {todo: entry.getTodo(), done: entry.isDone(), dueDate: entry.getDueDate()})
		end

		local jsonData: string = Json.encode(data)
		local file = Io.open(self.path, IoMode.Write)
		file?.write(jsonData)
		file?.close()
	end

	fn add(item: string, dueDate: integer?)
		Table.insert(self.storage, Entry(item, dueDate))
	end

	fn remove(id: integer) -> string?
		return Table.remove(self.storage, id)?.getTodo()
	end

	fn update(id: integer, dueDate: integer?, isDone: boolean?)
		if self.storage[id] == nil then
			return
		end

		if dueDate != nil then
			self.storage[id].setDueDate(dueDate)
		end

		if isDone != nil then
			self.storage[id].setDone(isDone)
		end
	end

	fn getAll() -> table<Entry>
		return self.storage
	end
end
```

## `src/render.sau`

```saule title="src/render.sau"
import Entry from storage

export class Renderer
	static fn print(tasks: table<Entry>)
		if #tasks == 0 then
			println("No tasks found.")
			return
		end

		for i, task in tasks do
			printf(
				"%s %d.%s %s\n",
				match task.isDone()
					case true then "[x]"

					case false then "[-]"
				end,
				i,
				match task.getDueDate()
					case nil then ""

					case dueDate then String.format(" (due %s)", Os.date("%Y-%m-%d", dueDate))
				end,
				task.getTodo()
			)
		end
	end
end
```
