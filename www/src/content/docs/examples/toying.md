---
title: "Graphics Window"
description: "Opens a window and draws a rectangle that follows the mouse, using the Love2D-style engine native package. Shows how Saule calls into a dynamically-loaded Rust library."
sidebar:
  order: 9
---

<!-- Generated from examples/toying by `npm run sync-docs`. Edit the example, not this file. -->

Opens a window and draws a rectangle that follows the mouse, using the Love2D-style `engine` native package. Shows how Saule calls into a dynamically-loaded Rust library.

[Browse this example on GitHub](https://github.com/lauriszz123/saule/tree/main/examples/toying)

:::caution[Prerequisite]
This one needs the `engine` native package installed first — run the `install_mac.sh` / `install_wsl.sh` / `install_windows.ps1` script for your platform from `scripts/`.
:::

## Run it

```sh
git clone https://github.com/lauriszz123/saule.git
cd saule/examples/toying
saule run
```

## `saule.config`

```
name: "toying"
version: "0.1.0"
entry: "src/main.sau"
src_dirs: ["src"]
min_saule_version: "26.1"
```

## `src/main.sau`

```saule title="src/main.sau"
import * from "engine"

class Main
	static fn main()
		Window.create(800, 600, "Sau Engine")
		while Window.isOpen() do
			Window.pollEvents()

			Graphics.clear(0.0, 0.0, 0.0)

			local x, y = Mouse.getPos()

			Graphics.rectangle("fill", x, y, 50.0, 50.0)

			Graphics.present()
		end
	end
end
```
