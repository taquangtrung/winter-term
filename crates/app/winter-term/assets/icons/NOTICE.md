# Bundled icon artwork

Winter ships third-party icon artwork under `file/` and `git/`. `build.rs` packs
it into the binary; nothing here is Winter's own work except where noted.

## `file/`: file and folder type icons

1481 SVG icons, and the extension, filename, and folder-name tables generated
from them into `src/icons/map.rs`.

Most of these are from [vscode-icons](https://github.com/vscode-icons/vscode-icons), Copyright The vscode-icons Contributors, under the MIT License, whose text is in [`file/LICENSE`](file/LICENSE). They keep the project's naming convention: `file_*.svg`, with a `<title>file_type_*</title>` inside. The few that vscode-icons does not have were drawn by Winter's author for an earlier VS Code icon theme in the same style.

## `git/`: working-tree status icons

8 SVG icons, drawn by Winter's author for an earlier VS Code extension, as their Inkscape editing metadata shows.

## `brand/`: AI provider logos

3 SVG logos drawn before each provider's quota in the status bar: `claude.svg` and `antigravity.svg` taken from the app icons of Winter's author's earlier VS Code extension, and `zai.svg` from that extension's icon font sources with the editing metadata stripped and a neutral gray fill added so it reads on light and dark themes. They are the providers' trademarks, shown only to name the provider whose quota sits beside them.

## `gutter/`: gutter marks

`marked.svg`, drawn in a listing's gutter beside an entry marked for an operation. Winter's own work.

## `symbols/`: LSP and UI symbols

Codicons from `@vscode/codicons` by Microsoft Corporation, licensed under [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).

## `winter-terminal.svg`

Winter's own icon, also packed as `app/winter-terminal` and drawn on every terminal tab.

## Regenerating the lookup tables

```bash
scripts/gen-icon-map.py <icon-theme.json> > crates/app/winter-term/src/icons/map.rs
```
