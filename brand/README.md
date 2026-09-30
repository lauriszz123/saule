# Brand

Three drawings, one idea. **Saulė** is Lithuanian for sun; the crescent riding
the orbit is **Lua**, whose name is Portuguese for moon. The language the
syntax came from is still up there — it is just daylight now.

Everything here is transparent. No asset carries a background, because the same
file has to sit on a dark README, a light marketplace card and an editor tab.
`build.mjs` asserts it on every export rather than trusting it.

## The three forms

| File | Drawn at | Use it |
|---|---|---|
| [`saule-logo.svg`](saule-logo.svg) | 512 | Standing alone — README headers, social cards, the VS Code tile. **≥128px**; below that the wordmark closes up. |
| [`saule-mark.svg`](saule-mark.svg) | 64 | Where the name is already on screen — a site header beside the title, a plugin tile. **48–512px**. |
| [`saule-mark-16.svg`](saule-mark-16.svg) | 16 | Favicons and an editor's file-type gutter. **≤32px**. |

They are redrawn, not scaled. At 64 units the sea drops from seven bands to
four and the orbit thins to stay a line rather than a stripe; at 16 the orbit
goes entirely — it reads as a stray scratch — the gradients flatten, and the
crescent is drawn fatter than scale would suggest, because at true proportion
its lit edge lands under a pixel and vanishes.

The wordmark is real outlines (Avenir Next Bold, converted to paths), not a
`<text>` element, so it renders identically everywhere and needs no font
installed. To change the word you have to regenerate the path, not edit a
string.

## Where each asset is installed

Copies, because the repositories are independent. Change a drawing here, then
push the copies out.

| Repository | Path | Source |
|---|---|---|
| saule | `www/src/assets/logo.svg` | `saule-mark.svg` — the header shows the title too (`replacesTitle: false`), so the wordmark would be doubled |
| saule | `www/public/favicon.svg` | `saule-mark-16.svg` |
| saule-vscode | `icon.png` | `png/saule-logo-128.png` — the Marketplace will not take an SVG |
| saule-intellij | `src/main/resources/icons/saule.svg` | `saule-mark-16.svg` — the file-type icon, loaded by `SauleIcons` |
| saule-intellij | `src/main/resources/META-INF/pluginIcon.svg` | `saule-mark.svg` at 40×40 — the Marketplace listing |
| saule-nvim | `assets/saule-logo.png` | `png/saule-logo-512.png` — no icon system, only the README |

## Rebuilding the PNGs

```sh
node brand/build.mjs
```

Writes `png/`. It borrows `sharp` from the website's dependencies rather than
adding a root devDependency for something that runs twice a year, so
`npm install` in `www/` has to have happened.

## Colour

| | |
|---|---|
| Sun | `#FFD84D` → `#FBAE34` → `#EF7F25` |
| Sea | `#FBAE34` → `#EF7F25`, fading out band by band |
| Orbit | `#F6A32C` |
| Moon | `#FFF6D8` → `#FFD98A` |
| Wordmark | `#2A1D0E` → `#0C0803`, with one pass of white across the top |
