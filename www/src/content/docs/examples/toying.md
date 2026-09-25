---
title: "Graphics Window"
description: "Opens a window and draws a rectangle that follows the mouse, using the Love2D-style Shine2D native package. Shows how Saule calls into a dynamically-loaded Rust library."
sidebar:
  order: 9
---

<!-- Generated from examples/toying by `npm run sync-docs`. Edit the example, not this file. -->

Opens a window and draws a rectangle that follows the mouse, using the Love2D-style Shine2D native package. Shows how Saule calls into a dynamically-loaded Rust library.

[Browse this example on GitHub](https://github.com/lauriszz123/saule/tree/main/examples/toying)

:::caution[Prerequisite]
This one needs the `shine` native package installed first. Shine2D lives in its own repository — clone [saule-shine](https://github.com/lauriszz123/saule-shine), run `cargo build --release`, and copy the built library into `~/.saule/native_packages/`.
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
import * from "shine"

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
