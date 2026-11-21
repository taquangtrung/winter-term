# Viewers

## Image

PNG, JPEG, GIF, WebP, BMP, ICO, and SVG files open in the image viewer, fitted to the pane.

| Key | Action |
|---|---|
| `+` / `-` | Zoom in, out |
| `=` / `1` | Fit the pane, actual size |
| `h` `j` `k` `l`, the wheel | Scroll a zoomed image |
| `C-g` | Show the size and scale |

A GIF shows its first frame. An SVG stays sharp at any zoom.

## Hex

Hex shows a file's bytes, sixteen to a row, beside the characters they read as. It opens on a binary file, and from Open With on any file. An inspector card beside the rows (below them in a narrow pane) reads the byte under the cursor as hex, decimal, binary, and a character, and the 16- and 32-bit values starting there.

| Key | Action |
|---|---|
| `h` / `l` | The byte before, after |
| `j` `k` and the Vim motions | Move by rows |
| `0` / `$` | The row's first, last byte |
| `g o` | Go to an offset, decimal or `0x` hex |
| A click on a byte | Go to it |
| `C-x C-q` | Turn editing on or off |
| `0`-`9`, `a`-`f` | While editing, write half of the byte |
| `u` | While editing, take the last byte back |
| `C-s` or `w` | Write the file |

## Markdown preview

`F7` (or `Markdown: Preview Beside`) shows the Markdown file in front rendered in the pane beside it, with code colored, math typeset, and Mermaid diagrams drawn.

- It follows your typing, unsaved, and keeps the cursor's block in view; `f` stops following.
- `j` `k`, `C-d` `C-u`, `gg` `G`, and the wheel scroll it.
- A click on a link opens it; `RET` or a double click goes to the block's source line.
- `/` finds text.

Nothing is fetched from the network, so remote images are left out.

## Open With

`Open: This File With…` opens the focused tab's file in another viewer or editor that can show it, such as an SVG as an image or as markup.
