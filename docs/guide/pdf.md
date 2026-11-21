# PDF

A `.pdf` file opens in Winter's PDF viewer wherever you open it: Dir, Git, the file browser, PDF text search, or `gx`. Winter draws the document itself, so nothing needs installing. A rebuilt PDF reloads by itself, at the same spot.

## Reading

| Key | Action |
|---|---|
| `j` `k` / `h` `l` | Scroll down, up / sideways |
| `C-d` `C-u` / `C-f` `C-b`, `Space` `b` | Half a screen, a screen |
| `gg` / `G` | First, last page |
| `5G` or `:` | Go to page five, ask for a page |
| `]` / `[` | Next, previous page |
| `+` / `-` / `=` | Zoom in, out, fit the width |
| `P` / `T` | Fit the whole page, fit the text |
| `C` | Crop pages to their text |
| `V` | Page thumbnails |
| `O` | The outline |
| `I` | Invert the colors |
| `f` / `F` | Label the links, follow or preview one |
| `C-o` / `C-i` | Back, forward over jumps |
| `/` `?`, `n` `N` | Search |
| `M-S-i` | Mark misspelled words |
| `C-g` | Show the page and how far through |

The toolbar along the top shows the section you are reading (click for the outline), the zoom, the page number, and a button to invert the colors.

## Selecting text

Drag to select and copy text. `i` puts a cursor in the text, so Vim motions move it, `v` and `V` select, and `y` copies (`yw`, `y$`, `y/text`). `Esc` takes the cursor out.

## LaTeX and SyncTeX

For a document built with `-synctex=1`, which latexmk does:

- From the PDF, `S` or a double click goes to the LaTeX source of that spot.
- From a `.tex` file, `F6` shows the cursor's line in the PDF; `S-F6` or `F7` shows it in the other pane.
- `s` labels the lines in view; type a label to go to that line's source.
- `M-S-s` tints each sentence for proofreading.
