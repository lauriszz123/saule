# md-viewer

A Markdown reader written in Saule, rendered with UIKit. Open a folder, pick a
file, read it — and click a `[link](other.md)` to go there, with back and
forward.

```sh
saule run examples/md-viewer            # opens on this project's docs/
saule run examples/md-viewer -- ~/notes # or any folder
```

**Barely started.** `src/` holds a `main.sau` and a `Screen.sau` that lists
the `.md` files in `docs/` beside an empty pane — no parser behind it yet. What
is finished is the design: a complete, cross-linked specification in
[`docs/`](docs/README.md), written so that the app's first job is rendering the
documents that describe it.

[Milestone 0](docs/06-build-order.md#milestone-0--make-uikit-a-package) *is*
done: `../uikit` is a real package, `UI Project` depends on it and still runs,
and `assets/` already holds the Material Icons font. Start at
[Milestone 1](docs/06-build-order.md#milestone-1--blocks-headless).

Start at **[docs/README.md](docs/README.md)**, or jump to:

* [Architecture](docs/02-architecture.md) — the UML: every class, who owns
  whom, what happens on a click
* [Build order](docs/06-build-order.md) — ten milestones, each independently
  runnable

It depends on two library packages:

* `../markdown` — the parser. Pure Saule, no UI, no filesystem. Config and a
  stub `src/` exist; `Markdown.parse` does not parse yet.
  [Design](docs/03-markdown-package.md).
* `../uikit` — the view toolkit, extracted from `examples/UI Project`. ✅ exists.
