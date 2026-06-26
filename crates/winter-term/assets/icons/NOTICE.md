# Bundled icon artwork

Winter ships third-party icon artwork under `file/` and `git/`. `build.rs` packs
it into the binary; nothing here is Winter's own work except where noted.

## `file/` — file and folder type icons

1481 SVG icons, and the extension, filename, and folder-name tables generated
from them into `src/icons/map.rs`.

These were imported from the `magic-vscode` VS Code extension's icon theme. The
naming convention (`file_*.svg`, with a `<title>file_type_*</title>` inside)
matches the [vscode-icons](https://github.com/vscode-icons/vscode-icons)
project, which is MIT licensed.

> **Provenance is unconfirmed.** The repository these were copied from carries
> no license file or attribution for them, so the chain of custody above is
> inferred from the naming convention rather than established. Confirm the
> upstream source and its license, record it here, and add the upstream license
> text before distributing a release that includes these files. Re-vendoring
> directly from upstream is the cleaner fix.

## `git/` — working-tree status icons

8 SVG icons, imported from the same extension. The files carry Inkscape editing
metadata rather than an upstream project's naming convention, which suggests
they are original to that extension rather than vendored into it. The same
caveat applies: confirm before distributing.

## Regenerating the lookup tables

```bash
scripts/gen-icon-map.py <icon-theme.json> > crates/winter-term/src/icons/map.rs
```
