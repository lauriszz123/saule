# UI Project

A Saule project. Run with:

```sh
saule run
```

The declarative, SwiftUI-flavoured view toolkit this renders with lives in its
own package, [`../uikit`](../uikit/README.md), listed in this project's
`dependencies:` and imported as `import * from "uikit"`. `src/Test.sau` is the
demo the entry point renders.

The whole kit leans on one Saule feature, the **trailing block**: when the last
argument to a call is a function, it can be written after the closing
parenthesis as `do … end`. That is what lets children be statements, callbacks
be blocks, and a window be declared rather than configured:

```saule
export class DemoApp extends App
    fn body() -> Scene
        return WindowGroup(title: "Demo", width: 420, height: 480) do
            VStack(spacing: 20.0) do
                Text("Hello").font(28.0)

                Button(label: "Press me") do
                    println("pressed")
                end
            end.padding(20.0).centered()
        end
    end
end

class Main
    static fn main()
        DemoApp().run()
    end
end
```

* `src/main.sau` — the `App`, its window, and `Main.main`
* `src/Test.sau` — the demo panel: controls, tabs, overlays, a live canvas
* `src/Counter.sau` — the smallest stateful view, using `context.state`
* [`../uikit/`](../uikit/) — the toolkit itself, as a `kind: "library"` package
