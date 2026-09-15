# Winter usage guide

How to drive Winter day to day: the modes, the full keymap, and the settings that change them.

For what Winter is and how to install it, see the [README](../README.md). For how it is built, see [`architecture.md`](architecture.md).

## Modes

A Winter pane is always in exactly one of five modes, and each pane keeps its own. The mode is shown in the status bar.

| Mode | Who owns the keyboard | Enter it with |
|---|---|---|
| **Insert** | The PTY. This is an ordinary terminal. | `i`, `a`, or `o` from Normal |
| **Normal** | Winter. Motions, operators, layout commands. | `Esc` from Insert |
| **Visual** | Normal, but motions extend a selection. | `v` `V` `Ctrl-v` from Normal |
| **Block-Focus** | A rich block's WebView. | `Enter` from Normal, on a block |
| **Page** | A tool page, such as Dir. | Opening a tool; the pane holds it until closed |

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
| `Ctrl-Shift-d` | Show Dir over the focused pane (toggle) |
| `Ctrl-Shift-g` | Show Git over the focused pane (toggle) |
| `Ctrl-Shift-p` or `Alt-x` | Command palette |
| `Ctrl-Shift-r` | History palette |
| `Ctrl-Shift-z` | Pane switcher (then press the digit shown on a pane) |
| `Ctrl-Shift-Up/Down` | Previous, next prompt block |
| `Ctrl-Backspace` | Delete the word before the cursor |

A single-chord binding whose action is not one of the built-in window actions is looked up against the command palette instead, so `"M+q" "mux_new_session"` works.

## Tools

A tool opens over the focused pane, covering it. The shell underneath keeps running and comes back the moment you close the tool with `q`, or by pressing the tool's own chord again. The window chords all still work while a tool is up: split, zoom, close, and `Alt-h/j/k/l` to move focus. For a tool beside your shell rather than over it, split first and open it in the new pane.

Every tool searches the same way. `/` asks for text and moves the cursor to the next row holding it, `n` and `N` step through the rest of the matches and wrap at the ends, and a search that finds nothing says so instead of sitting still. Matching is literal and pays no attention to case: a directory listing matches on the entry's name, every other tool on the whole row as it is painted, so a chord is as findable as the command beside it. A search moves the cursor and nothing else: marks, folds, and everything staged stay as they were. Grep is the one exception to the first part, since its rows already are a search: `/` there asks for a new one.

**Dir** (`Ctrl-Shift-d`, or `Dir: Open Working Directory`) lists a directory, rooted at the working directory of the pane it opened from.

| Key | Action |
|---|---|
| `j` `k` or `Down` `Up` | Move down, up |
| `Home` / `End` | First entry, last entry |
| `Enter` | Enter a directory, or open a file in `$EDITOR` in a new tab |
| `l` / `Right` | Open the directory under the cursor, in place |
| `h` / `Left` | Close it, else step out to the parent row, else leave the root |
| `Backspace` | Leave the root for its parent |
| `Alt-n` / `Alt-p` | Next, previous entry at the same level |
| `Alt-u` / `Alt-d` | Move to the parent, into an open directory |
| `Shift-Alt-b` / `Shift-Alt-f` | Back, forward through directories visited |
| `/` | Search the listing |
| `n` / `N` | Next, previous match |
| `Tab` / `Shift-Tab` | Fold the entry, fold everything |
| `z u` / `z f` / `z t` | Open, close, toggle the whole subtree |
| `z a` / `z c` | Toggle everything, close everything |
| `Ctrl-c 0` .. `Ctrl-c 9` | Open the tree to that depth |
| `.` | Show or hide dotfiles |
| `,` | Show or hide the size, age, and permission columns |
| `Shift-Alt-s` | Show directory sizes; pressing it again stops the walks |
| `s` | Cycle the sort: name, time, size |
| `G` | Re-read the listing |
| `m` / `u` | Mark, unmark the entry and step on |
| `M` / `U` | Mark everything listed, unmark everything |
| `_` / `+` | New file, new directory |
| `R` | Rename the entry under the cursor |
| `C` / `Alt-m` | Copy, move the targets |
| `x` | Delete the targets, after confirming |
| `*` | Set permission bits, as octal |
| `&` | Open the entry with the system handler |
| `q` | Close |

A directory's size is not something the filesystem knows, so `Shift-Alt-s` walks each one in the background, a directory at a time, showing `...` until a total arrives. Turning it off stops whatever is still walking. Totals are kept while you stay in the same directory and dropped when the root moves.

An operation acts on the marked entries, or on the entry under the cursor when nothing is marked, never both: the header shows how many are marked and what the last operation reported. A name typed into a prompt is a name, so `../elsewhere` is refused rather than reaching outside the listing, and nothing overwrites an existing entry. Copy and move take a new name for one target and a destination directory for several. Moving the root clears the marks, since a mark held over would count toward an operation in a listing it was never part of.

Directories sort before files whatever the key, and moving the root, toggling a view option, or folding keeps the cursor on the entry it was already on. A block cursor sits on the active row's first non-blank column and follows it as you move, so a tool pane shows where a selection would start before you ask for one. `v` starts selecting from there, for copying out of a listing: `hjkl` and the arrows move and extend, `w` and `b` step words, `0`, `$`, `g` and `G` jump, `V` switches to whole lines, a second `v` drops the selection, `y` or `Enter` copies and leaves, and `Esc` cancels. While selecting it answers every key ahead of the tool, so the motions mean what they do in Vim rather than what the listing binds them to; chorded keys still reach the window, so splitting, zooming, and moving focus work from inside it. Dragging with the mouse selects the same rows.

Each entry carries an icon for what it is. The `icons` setting chooses where it comes from: `"svg"` (the default) draws bundled artwork keyed by extension, exact filename, or directory name, which needs nothing of the terminal font; `"font"` draws a Nerd Font glyph for the entry's broad category, which scales, themes, and copies like any other character but needs a patched font, as the status bar's own mode glyphs already do; `"none"` draws neither.

**Git** (`Ctrl-Shift-g`, or `Git: Status`) shows the working tree of the repository the pane's directory sits in, as foldable sections. A file changed both in the index and in the working tree appears in both, which is what lets one half be staged without the other. Every command runs from the repository root, off the event-loop thread, and the view re-reads the tree after anything that changed it; a failure is reported in the header rather than swallowed.

| Key | Action |
|---|---|
| `j` `k` or `Down` `Up` | Move down, up |
| `Alt-n` / `Alt-p` | Next, previous entry, skipping blank lines |
| `g t` / `g u` / `g s` / `g r` | Jump to untracked, unstaged, staged, recent |
| `/` | Search the view, or the log or listing filling it |
| `n` / `N` | Next, previous match |
| `Tab` / `Shift-Tab` | Fold the section, fold everything |
| `Enter` | Open the file under the cursor in `$EDITOR` |
| `Enter` on a commit | Read the commit: message, files changed, then the patch |
| `Tab` on a file | Show its diff, hunk by hunk |
| `s` / `S` | Stage the target, stage everything |
| `u` / `U` | Unstage the target, unstage everything |
| `x` | Discard the target, after confirming |
| `a` / `-` | Apply, reverse the hunk under the cursor |
| `y` | Copy the hash or path under the cursor |
| `P` / `F` / `f` | Push, pull with rebase, fetch all |
| `G` | Re-read the working tree |
| `Ctrl-Shift-c` | Commit what is staged, with a one-line message |
| `Ctrl-Shift-s` | Stage all: untracked files when the cursor is in that section, tracked changes otherwise |
| `!` | Run any git command |
| `Alt-b` | Blame the file under the cursor |
| `Alt-y` | Show every ref |
| `Alt-g` | Open the remote in a browser |
| `q` | Close |

On a section heading, `s`, `u`, and `x` act on every file in that section. On a hunk, or on any line inside it, they act on that hunk alone: the patch is built from the hunk and fed to `git apply` on standard input, so the rest of the file is untouched. Discarding an untracked file deletes it, since `git restore` cannot reach a path that is not in the index.

These keys open a menu, and the next key picks from it:

| Key | Menu |
|---|---|
| `b` | Branch: checkout, create, delete |
| `c` | Commit: in an editor, one line, amend |
| `d` | Diff: working tree, staged, against a revision |
| `i` | Ignore: this path, this extension |
| `m` | Merge: merge, continue, abort |
| `r` | Rebase: interactive, onto upstream, continue, skip, abort |
| `t` | Tag: create, delete, list |
| `z` | Stash: push, pop, apply, drop, list |
| `A` | Cherry-pick: pick, continue, abort |
| `B` | Bisect: start, good, bad, reset |
| `M` | Remote: list, add, remove, prune |
| `O` | Reset: mixed, soft, hard |
| `V` | Revert: revert, continue, abort |
| `Z` | Worktree: list, add, remove |
| `Ctrl-l` | Log: this branch, all refs, this file |

Anything that wants a terminal of its own goes to a new tab: commit and amend in `$EDITOR`, and interactive rebase with its todo list. `o` resets to the commit under the cursor, or asks which one when the cursor is elsewhere.

A log, a listing, a blame, or a diff fills the view; `j`/`k` move through it, `/` searches it, `+` asks for more of a log, `y` copies the first field of a line, which is the commit hash in a log or a blame, and `q` goes back to the status.

**Grep** (`Ctrl-Shift-f`, or `Grep: Search Files`) finds the lines under the pane's working directory that hold some text, grouped under one heading per file.

| Key | Action |
|---|---|
| `/` | Search for text, starting from what you searched for last |
| `j` `k` or `Down` `Up` | Move down, up |
| `n` / `N` | Next, previous match, skipping the file names between them |
| `Home` / `End` | First row, last row |
| `Alt-n` / `Alt-p` | Next, previous file |
| `Enter` | Open the file in `$EDITOR` at the line that matched |
| `y` | Copy the match's `path:line` |
| `G` | Run the same search again |
| `Esc` | Stop a running search; close the tool when none is running |
| `q` | Close |

The walk runs off the event loop, so the pane stays live while a large tree is read, and `Esc` abandons it. `/` here asks for a new search rather than searching the rows, since the rows already are one, and `n` and `N` step through what it found. Matching is literal text, whatever the case, the same as every other tool's search.

The walk skips what would swamp the results rather than reading everything: `.git`, `.hg`, `.svn`, `node_modules`, and `target`, files over a megabyte, anything that does not read as text, and symlinks, which are never followed. It stops at 500 matches and says so in the header, because a query loose enough to pass that is one to narrow.

**Keys** (`Keys: Show Every Command`) lists every command and the chord bound to it, read from the keymap in force, so it cannot disagree with what the keys actually do. `/`, `n`, and `N` search it, over both columns, so a chord you pressed by accident is as easy to look up as a command you are hunting for. `Enter` runs the command under the cursor: the page hands the pane back first, so a command that opens a tool of its own has somewhere to open.

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
| `icons` | Tool-pane entry icons: `"svg"` (default), `"font"`, or `"none"` |
| `cursor { ... }` | `blink`, `hide-in-inactive`, plus the shape per mode: `insert`, `normal`, `visual`, `block-focus` |
| `status-bar { ... }` | `show`, `show-mode`, and the per-mode icons |
| `clipboard-read` | Let programs read the clipboard through OSC 52 |
| `security { ... }` | `block-max-trust`, `block-remote-assets` |

Every setting above is also editable in the settings page (`Ctrl-,`), which writes the same file: a colour table and the keybindings are the two that need the file or their own page instead of a row.

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
