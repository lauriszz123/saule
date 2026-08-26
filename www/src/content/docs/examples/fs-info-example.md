---
title: "Filesystem Info"
description: "Inspects one path with Os.fsInfo and reports what it is. Shows nullable returns for paths that may not exist, and match over an enum with a _ fallback."
sidebar:
  order: 2
---

<!-- Generated from examples/fs-info-example by `npm run sync-docs`. Edit the example, not this file. -->

Inspects one path with `Os.fsInfo` and reports what it is. Shows nullable returns for paths that may not exist, and `match` over an enum with a `_` fallback.

[Browse this example on GitLab](https://gitlab.com/lauriszz123/saule/-/tree/main/examples/fs-info-example)

## Run it

```sh
git clone https://gitlab.com/lauriszz123/saule.git
cd saule/examples/fs-info-example
saule run -- .
```

## `saule.config`

```
name: "log-cleaner"
version: "0.1.0"
entry: "src/main.sau"
src_dirs: ["src"]
min_saule_version: "26.1"
```

## `src/main.sau`

```saule title="src/main.sau"
class Main
	static fn usage()
		println("Usage:")
		println("  saule run -- <path>")
	end

	static fn main()
		local args = Os.args()

		match args[1]
			case nil then Main.usage()

			case v then Main.run(v)
		end
	end

	static fn run(path: string)
		local info = Os.fsInfo(path)

		if info == nil then
			printf("Error: Path not found: %s\n", path)
			return
		end

		match info.kind
			case FsKind.File then
				printf("File: %s\n", info.path)
				printf("Size: %d bytes\n", info.size)
				printf("Modified at: %d\n", info.modifiedAt)

			case FsKind.Dir then
				printf("Directory: %s\n", info.path)
				printf("Size: %d bytes\n", info.size)
				printf("Modified at: %s\n", info.modifiedAt)

			case _ then println("Something else")
		end
	end
end
```
