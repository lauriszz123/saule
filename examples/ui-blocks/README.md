# ui-blocks

A declarative, SwiftUI-shaped UI toolkit built on trailing blocks, drawn to the
terminal.

```sh
saule run
```

```
+-Saule UI--------------------------+
|                                   |
| Trailing blocks, drawn.           |
| +-Session----------------+        |
| | player             ada |        |
| | region        eu-north |        |
| | build             26.1 |        |
| +------------------------+        |
| +-Scoreboard-------------+        |
| | ada              120 * |        |
| | grace            340 * |        |
| | linus               90 |        |
| | ---------------------- |        |
| | total              550 |        |
| +------------------------+        |
| [ Play ]   [ Options ]   [ Quit ] |
|                                   |
+-----------------------------------+
```

Every widget is a class, and constructing one draws it. Containers declare
their children as the **last** initialiser parameter, so callers pass them as a
trailing block:

```saule
local screen: Canvas = Canvas() do
    Panel(title: "Session") do
        Field("player", "ada")
    end
end

println(screen.render())
```

`Panel(title: "Session") do … end` is exactly
`Panel(title: "Session", fn() Field("player", "ada") end)` — the same call,
with less ceremony. Because `body` is the last parameter, anything in front of
it can be named, positional, or defaulted (`Panel`'s `spacing` is skipped
above) and the block still lands on `body`.

Nothing is assembled as data: the block runs, the widgets inside it draw
themselves, and the container frames whatever they produced. That is why a
block beats a table of children — `if` and `for` work inside one.

`src/canvas.sau` is the drawing surface, `src/widgets.sau` the widget set, and
`src/main.sau` the screen above.
