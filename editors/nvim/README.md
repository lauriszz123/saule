# Saule for Neovim / Vim

Classic Vim regex highlighter. Drop-in install:

## Manual

Copy the three files into your runtimepath:

```bash
mkdir -p ~/.config/nvim/{syntax,ftdetect,ftplugin}
cp syntax/saule.vim    ~/.config/nvim/syntax/
cp ftdetect/saule.vim  ~/.config/nvim/ftdetect/
cp ftplugin/saule.vim  ~/.config/nvim/ftplugin/
```

## Via a plugin manager

Point lazy.nvim / packer at this folder, e.g.:

```lua
-- lazy.nvim
{ dir = "~/Codai/rust/saule/editors/nvim", name = "saule.vim", ft = "saule" }
```

Open any `*.sau` file and `:set ft?` should report `saule`. Colours follow
your active colorscheme via standard groups (`Keyword`, `Type`, `Function`,
`String`, `Comment`, …).

## Future: tree-sitter

For more accurate highlighting (and folding/indent), a `tree-sitter-saule`
grammar can later replace this; nvim-treesitter would then auto-pick it up.
