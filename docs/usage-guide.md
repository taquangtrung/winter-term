# Winter usage guide

How to drive Winter day to day: the modes, the full keymap, and the settings that change them.

For what Winter is and how to install it, see the [README](../README.md). For how it is built, see [`architecture.md`](architecture.md).

## Modes

A Winter pane is always in exactly one of four modes, and each pane keeps its own. The mode is shown in the status bar.

| Mode | Who owns the keyboard | Enter it with |
|---|---|---|
| **Insert** | The PTY. This is an ordinary terminal. | `i`, `a`, or `o` from Normal |
| **Normal** | Winter. Motions, operators, layout commands. | `Esc` from Insert |
| **Visual** | Normal, but motions extend a selection. | `v` `V` `Ctrl-v` from Normal |
| **Block-Focus** | A rich block's WebView. | `Enter` from Normal, on a block |

Insert is the default, so Winter behaves like a normal terminal until you ask it not to.

**Getting into Normal mode.** Press `Esc`. The one exception is when a full-screen program is running (vim, btop, less) or a tab completion is pending: there `Esc` belongs to that program, so Winter forwards it and you press `Esc` twice within 400ms to take the keyboard back instead.

**Getting out.** `i`, `a`, or `o` return to Insert. `Esc` in Normal stays in Normal, deliberately: the key that means "stop what I am doing" everywhere else must not drop your next keystroke into the shell mid-navigation.

Normal mode works against any shell, over `ssh`, and inside a Python REPL, because Winter owns the keyboard while it is active and nothing has to be installed on the far end.

## Keybinding reference

`normal`, `insert`, and `visual` use the built-in Vim-style bindings below and are not configurable yet. The `window` and `editing` chords further down are, in `keybindings.kdl`.

### Motions

Available in both Normal and Visual mode. Most take a count prefix, so `5j` moves down five lines and `3w` advances three words.

| Keys | Motion |
|---|---|
| `h` `j` `k` `l` | Left, down, up, right (arrow keys also work) |
| `w` `b` `e` | Word forward, word back, word end |
| `W` `B` `E` | Same, treating every non-blank run as one word |
| `ge` `gE` | Back to the end of the previous word |
| `0` or `\|` | First column |
| `^` or `_` | First non-blank character |
| `$` | End of line |
| `g_` | Last non-blank character |
| `{` `}` | Paragraph back, paragraph forward |
| `%` | Matching bracket |
| `H` `M` `L` | Top, middle, bottom of the screen |
| `gg` `G` | First line, last line |
| `PageUp` `PageDown` | Page up, page down |
| `Ctrl-u` `Ctrl-d` | Half page up, half page down |
| `zz` `zt` `zb` | Scroll current line to centre, top, bottom |

### Character search

| Keys | Action |
|---|---|
| `f{char}` `F{char}` | Jump to the next or previous `{char}` on the line |
| `t{char}` `T{char}` | Jump just before the next or previous `{char}` |
| `;` `,` | Repeat the last character search, forward or reversed |

When the target character occurs more than once, Winter shows a labelled overlay so you can pick a landing spot with one keystroke instead of repeating `;`.

### Operators and text objects

| Keys | Action |
|---|---|
| `d` `c` | Delete, change (followed by a motion or text object) |
| `D` `C` | Delete, change to end of line |
| `dd` `cc` | Delete, change the whole line |
| `S` | Change the whole line |
| `x` | Delete the character under the cursor |
| `s` | Substitute the character under the cursor |
| `r{char}` | Replace the character under the cursor |
| `~` | Toggle the case of the character under the cursor |
| `.` | Repeat the last change |

Text objects follow `i` (inner) or `a` (around) after an operator, so `diw` deletes a word and `ci"` changes the text inside quotes.

| Object | Selects |
|---|---|
| `w` `W` | Word, big word |
| `"` `'` `` ` `` | The quoted run, using that quote character |
| `(` `)` `b` | The parenthesised run |
| `[` `]` | The bracketed run |
| `{` `}` `B` | The braced run |
| `<` `>` | The angle-bracketed run |

### Surround

| Keys | Action |
|---|---|
| `ys{object}{char}` | Surround the object with `{char}` |
| `cs{from}{to}` | Change the surrounding `{from}` to `{to}` |
| `ds{char}` | Delete the surrounding `{char}` |

### Visual mode

| Keys | Action |
|---|---|
| `v` `V` `Ctrl-v` | Charwise, linewise, blockwise Visual |
| `Alt-i` | Select the paragraph under the cursor, linewise (vim's `vip`) |
| `o` | Move the cursor to the other end of the selection |
| `gv` | Restore the last Visual selection |
| `y` | Yank the selection and leave Visual |
| `"{reg}y` | Yank the selection into register `{reg}` |

### Search

| Keys | Action |
|---|---|
| `/` `?` | Search forward, search backward |
| `n` `N` | Next match, previous match |
| `*` `#` | Search forward, backward for the word under the cursor |
| `Esc` | Put the search away, keeping the cursor on the match it found |

`n` and `N` still work after the search bar is dismissed: the pattern and its direction survive, the way vim keeps them across `:nohlsearch`.

### Marks, registers, and history

| Keys | Action |
|---|---|
| `m{a-z}` | Set a mark |
| `` `{a-z} `` | Jump to a mark, exact column |
| `'{a-z}` | Jump to a mark, first non-blank |
| `"{reg}` | Use register `{reg}` for the next yank or paste |
| `p` | Paste |
| `Ctrl-o` `Ctrl-i` | Jump backward, forward through the jumplist |
| `g;` `g,` | Step back, forward through the changelist |

Marks and the jumplist are per pane. Registers are shared across every pane, so a yank in one pastes in another.

### `g` commands

| Keys | Action |
|---|---|
| `gt` `gT` | Next tab, previous tab |
| `g<` `g>` | Move the current tab left, right |
| `gx` | Open the URL or path under the cursor |
| `gs` | Buffer swoop: fuzzy line search over the pane |
| `gn` `gN` | Select the next, previous search match |
| `gv` | Restore the last Visual selection |
| `gp` `gP` | Jump to the prompt, to the previous prompt |
| `g;` `g,` | Step back, forward through the changelist |

### Blocks

Blocks come from [shell integration](#shell-integration); without it the whole session is one rolling block.

| Keys | Action |
|---|---|
| `]b` `[b` | Next block, previous block |
| `y` | Yank the block under the cursor |
| `q` | Quick-select: label every block, press a label to act on it |
| `Enter` | Focus a rich block, handing keys to its WebView |

### Prompt line editing

Normal-mode operators aimed at the line the shell is currently editing are translated into the readline keystrokes that produce the same result, so `dw` on the prompt really does delete a word in your shell.

| Keys | Action |
|---|---|
| `Ctrl-/` | Undo your prompt edits |
| `Ctrl-\` | Redo them |

This assumes your shell's default emacs-mode bindings. **If your shell is in vi mode** (`bindkey -v`, `set editing-mode vi`) set `prompt-edit-bindings "none"`, so Winter declines these operators and leaves the line to the shell, which gives you vim editing there anyway. Everything else, all navigation over the screen and scrollback, works the same either way.

### Window, pane, and tab chords

These work in any mode and are configurable in `keybindings.kdl`. `C` is Ctrl, `S` is Shift, `M` is Meta/Alt.

| Chord | Action |
|---|---|
| `Shift-Alt--` / `Shift-Alt-\` | Split horizontally, vertically |
| `Shift-Alt-=` / `Ctrl-Shift-m` | Zoom the focused pane (toggle) |
| `Ctrl-Shift-q` | Close the focused pane |
| `Shift-Alt-o` | Close every other pane |
| `Alt-h/j/k/l` | Move focus between panes |
| `Alt-1` .. `Alt-9` | Focus pane N |
| `Ctrl-Alt-1` .. `Ctrl-Alt-9` | Close pane N |
| `Shift-Alt-h/l` | Scroll page up, page down |
| `Shift-Alt-k/j` | Scroll line up, line down |
| `Shift-Alt-a` / `Shift-Alt-e` | Scroll to top, to bottom |
| `Ctrl-Shift-t` / `Ctrl-Shift-w` | New tab, close tab |
| `Ctrl-Tab` / `Ctrl-Shift-Tab` | Next tab, previous tab |
| `Ctrl-PageUp` / `Ctrl-PageDown` | Previous tab, next tab |
| `Ctrl-1` .. `Ctrl-9` | Go to tab N |
| `Ctrl-Shift-c` / `Ctrl-Shift-v` | Copy selection, paste |
| `Ctrl-,` | Open settings |
| `Ctrl-=` / `Ctrl--` / `Ctrl-0` | Font bigger, smaller, reset |
| `Ctrl-Shift-p` or `Alt-x` | Command palette |
| `Ctrl-Shift-r` | History palette |
| `Ctrl-Shift-z` | Pane switcher (then press the digit shown on a pane) |
| `Ctrl-Shift-Up/Down` | Previous, next prompt block |
| `Ctrl-Backspace` | Delete the word before the cursor |

A single-chord binding whose action is not one of the built-in window actions is looked up against the command palette instead, so `"M+q" "mux_new_session"` works.

## Configuration

Config lives in `~/.config/winter-term/` (`%APPDATA%\winter-term` on Windows), split in two:

- `settings.kdl` for appearance and behaviour
- `keybindings.kdl` for the `window` and `editing` chords

Both are written in [KDL](https://kdl.dev/). Winter installs a reference copy of each, and the shipped copies in `crates/winter-term/samples/` are the real defaults rather than examples: anything you do not mention keeps its default, anything you do mention replaces it.

Changes apply on save, without a restart. `winter --reload` asks a running instance to reload as well.

### Settings

| Key | Meaning |
|---|---|
| `font` / `font-size` | Font family and size |
| `font-weight` | Named weight for normal text |
| `font-weight-bold` | Named weight for bold text; falls back to `font-weight` |
| `ligatures` | OpenType ligatures, so `->` and `=>` render as arrows |
| `theme` | `"dark"`, `"light"`, `"auto"`, or the name of a theme file |
| `opacity` | Window opacity, clamped to `0.1` .. `1.0` |
| `menu-style` | `"modern"` (hamburger) or `"classic"` (menu bar) |
| `title-bar-style` | Native or Winter-drawn title bar |
| `window-controls-side` | Which side the window buttons sit on |
| `pane-border-width` | Divider thickness between panes |
| `dim-inactive` | Dim panes that are not focused |
| `url-underline` | Underline detected URLs and OSC 8 links |
| `palette-match-underline` | Underline matched characters in the palette |
| `paste-on-right-click` | Right click pastes instead of opening a menu |
| `restore-session` | Reopen the previous layout on launch |
| `scrollback-lines` | Scrollback ceiling |
| `shell` | Shell to spawn, instead of the system default |
| `shell-linux` / `shell-macos` / `shell-windows` | Per-platform override of `shell` |
| `window-title-template` | Window title, e.g. `"{{ app_name }} - {{ pane_title }}"` |
| `rainbow-parens` | Color brackets by nesting depth |
| `sentence-highlight` | Alternating bands over sentences, as a reading aid |
| `wrap-indent` | Hang the continuation of a soft-wrapped line under its start |
| `prompt-edit-bindings` | `"emacs"` (default) or `"none"`, see above |
| `cursor { ... }` | `blink`, plus the shape per mode: `insert`, `normal`, `visual` |
| `status-bar { ... }` | `show`, `show-mode`, and the per-mode icons |
| `clipboard-read` | Let programs read the clipboard through OSC 52 |
| `security { ... }` | `block-max-trust`, `block-remote-assets` |

Themes are separate KDL files under `themes/`, each an optional `base "dark"|"light"` plus a `colors` block layered over it. A `colors` block written directly in `settings.kdl` works too, as does a `keybindings` block, which is what the legacy single-file `winter.kdl` used before the split.

## Shell integration

Blocks, exit codes, folding, and the working directory Winter shows all come from OSC 133 and OSC 7 marks that your shell has to emit. That is one line in your rc file:

```bash
# bash
source /usr/share/winter-term/shell-integration/winter.bash

# zsh
source /usr/share/winter-term/shell-integration/winter.zsh

# fish
source /usr/share/winter-term/shell-integration/winter.fish
```

Without it Winter still works, but the whole session is one rolling block: no per-command boundaries, no exit-code tags, no folding.

## Rich blocks

Programs can emit typed, MIME-tagged content that Winter renders inline: tables, charts, math, images, PDFs. Every block carries a `text/plain` fallback, so the same program stays readable under `tmux`, `ssh`, or in CI.

The protocol is documented in [`terminal-block-protocol-spec.md`](terminal-block-protocol-spec.md), and client libraries live under `clients/`.

Content from a block is untrusted by default. See the README's [security model](../README.md#security-model-for-rich-blocks) for what each trust tier grants and how to change the ceiling.

## Multiplexer

`winter mux` manages headless PTY sessions that outlive the window:

```bash
winter mux serve            # run the session server
winter mux list             # list sessions
winter mux new <name>       # create a session
winter mux attach <name>    # attach to one
winter mux kill <name>      # terminate one
```

Sessions are reachable over a Unix socket locally, or over an SSH tunnel to a remote server. PTY geometry is arbitrated at the smallest attached client, so a larger pane letterboxes rather than rendering a stream wrapped for the wrong width.
