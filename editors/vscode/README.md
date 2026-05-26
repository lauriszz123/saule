# Saule for VS Code

TextMate grammar + language config for `.sau` files.

## Try it locally

```powershell
# from the repo root
cd editors\vscode
# install once, then "Run Extension" from the Run & Debug panel,
# or package it:
npm install -g @vscode/vsce
vsce package
code --install-extension saule-0.1.0.vsix
```

Or, for zero-build dev: copy the `editors/vscode` folder into
`%USERPROFILE%\.vscode\extensions\saule-0.1.0\` and reload VS Code.

Colours come from the user's active theme via these scopes:
`keyword.control`, `keyword.declaration`, `entity.name.type`,
`entity.name.function`, `string.quoted.double`, `comment.line`,
`constant.numeric`, `constant.language`, `variable.language`.
