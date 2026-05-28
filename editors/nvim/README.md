# Saule for Neovim / NvChad

This folder ships two things, both consumable directly from this repo —
no copying into `~/.config/nvim/`:

1. **Syntax + filetype detection** — `ftdetect/`, `ftplugin/`, `syntax/`.
2. **LSP client glue** — `lua/saule/lsp.lua`, which registers
   `saule-lsp` with `nvim-lspconfig`. The defaults point straight at
   this checkout's `target/release/saule-lsp` binary, so you build
   once and never have to touch `$PATH`.

## 1. Build the language server (one time)

From the repo root:

```bash
cargo build --release -p saule-lsp
```

That's it — the Lua helper finds `target/release/saule-lsp` itself by
introspecting its own file path.

## 2. Load the plugin from this folder

Use whichever plugin manager you already have configured. The point is
to add this folder to Neovim's runtimepath so the `saule` filetype, the
syntax file, and `require("saule.lsp")` all resolve.

### lazy.nvim

In your NvChad `lua/plugins/init.lua` (or wherever you list extra plugins):

```lua
return {
  -- existing entries …
  {
    dir = "/mnt/c/Users/lauri/Documents/Codai/rust/saule/editors/nvim",
    name = "saule.vim",
    ft = "saule",
  },
}
```

Adjust the `dir` path if your Neovim runs on the Windows side rather
than inside WSL (use `C:\\Users\\lauri\\Documents\\Codai\\rust\\saule\\editors\\nvim`
or `~/Documents/Codai/rust/saule/editors/nvim` as appropriate).

## 3. Enable the LSP in NvChad

Edit `~/.config/nvim/lua/configs/lspconfig.lua` and add a single line:

```lua
require("nvchad.configs.lspconfig").defaults()

-- your other servers …

require("saule.lsp").setup()
```

The helper will:

* Auto-locate the server binary at
  `<this-repo>/target/release/saule-lsp` (use `{ profile = "debug" }`
  if you've only built debug).
* Pick up NvChad's shared `on_attach` / `capabilities` so the buffer
  gets the same keymaps and completion source as the rest of your LSP
  setup.
* Detect the project root via `saule.config` / `Cargo.toml` / `.git`,
  with `single_file_support = true` for scratch files.

### Override the binary path

```lua
require("saule.lsp").setup({
  -- Different repo location:
  repo = "/some/other/clone/of/saule",
  -- Or fully custom command:
  -- cmd = { "/abs/path/to/saule-lsp" },
})
```

## 4. What you get

* **`vim.diagnostic`** — lex, parse, semantic, and type errors with
  spans pointing at the offending source. Use `]d` / `[d` (NvChad
  defaults) to jump between them and watch the gutter signs.
* **Live re-analysis** on every change (full document sync; the server
  is small enough that incremental sync isn't needed yet).

Not yet wired up (will land server-side first, then surface here):
formatting, hover, goto-definition, completion.

## Future: tree-sitter

For more accurate highlighting (and folding/indent), a
`tree-sitter-saule` grammar can later replace the Vim regex highlighter;
nvim-treesitter would then auto-pick it up.
