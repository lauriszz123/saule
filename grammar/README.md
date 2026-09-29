# TextMate grammar

`saule.tmLanguage.json` is the one copy of Saule's TextMate grammar. It lives
here rather than in an editor plugin because more than one thing reads it, and
a second copy drifts the moment a keyword is added:

| Reader | How |
|---|---|
| The documentation site | `www/src/lib/saule-grammar.mjs` reads this file at build time and hands it to Shiki, so every code fence on the site is highlighted by it |
| The VS Code extension | ships a copy in `syntaxes/`, because a .vsix has to carry the files it serves — refreshed with `npm run sync:grammar` in [saule-vscode](https://github.com/lauriszz123/saule-vscode) |

The IntelliJ plugin and the Neovim plugin do not use it: IntelliJ highlights
with a hand-written lexer mirroring `crates/saule-lexer`, and Neovim with a Vim
regex syntax file.

It sits next to the crates so that adding a keyword to `crates/saule-lexer` and
teaching the grammar about it are one change in one repository. After editing
it, run `npm run sync:grammar` in a `saule-vscode` checkout and commit the copy
there.
