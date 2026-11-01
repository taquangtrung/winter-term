# Winter usage guide

How to use Winter, a terminal-first IDE, day to day: the modes, the full keymap, and the settings that change them.

For what Winter is and how to install it, see the [README](../README.md). For how it is built, see [`architecture.md`](architecture.md).

## Modes

A window is split into panes. Each pane is a strip of tabs, and every tab is a terminal, a page, or an editor. A terminal runs a program. A page shows Winter's own state: Settings, Keys, Notifications, Extensions, Processes, System, Docker, and an extension's views. A window has one of each page, so opening one again goes to it wherever it is, and closing it keeps nothing: it is built afresh when opened again, and an open page comes back with the session. An editor shows content (a file, a directory, a repository, a build) held in a buffer, which every tab on it shares and which closing its last tab buries rather than loses. A terminal tab is always in exactly one of three modes, and each tab keeps its own. A page or an editor owns the keys it binds.

The status bar belongs to the window, not the terminal. While an editor has the keyboard, the status bar shows:

- the editor's own mode where it has one (the text editor's Insert or Normal, a PDF's caret or selection, a listing whose names are being edited), and `P` where it has none;
- what the editor has to report;
- at its right edge, where the cursor is, such as `12:5  340 lines` in the text editor or `page 3 / 18` in a PDF.

The status bar is shown by default; `status-bar { show #false }` hides it and frees its row. The mode indicator is on by default; `status-bar { show-mode #false }` hides it.

The status bar's left edge shows where the focused terminal's commands run: a desktop glyph for this machine, the remote glyph and host name while `ssh` (or a remote mux session) is in the foreground, and the Docker whale and container name while `docker exec`, `docker compose exec`, or `podman exec` is. Its right edge ends in a bell.

Before the right edge, the status bar shows AI quotas for Antigravity, Claude Code, and Z.ai. Each appears under its logo as its short window and its weekly window, with usage and reset time in local time (`42% 14:05 | 10% Mon 09:00`). Each is fetched the same way its own tools fetch it, so no setup is needed beyond those tools.

- Claude Code: from Anthropic's usage endpoint, with the token Claude Code signed in with (the macOS keychain, else `.credentials.json` in `~/.claude` or `$CLAUDE_CONFIG_DIR`).
- Antigravity: from the language server a running Antigravity editor starts, else from the statusline file the `agy` CLI writes while it runs.
- Z.ai: needs an API key. `Quota: Set Z.ai API Key` in the command picker asks for it without showing it and stores it in the system's secret store (the Secret Service on Linux, the Keychain on macOS, the Credential Manager on Windows), never in the config. A blank answer forgets the key.
- Claude Code and Z.ai refresh every five minutes, Antigravity every minute. `Quota: Refresh` refreshes all of them at once.
- A provider that cannot be queried (not signed in, not running, no key) is hidden. One whose request fails keeps its last value.
- Requests go through `curl`, with tokens on its standard input, not its command line. Nothing is fetched while the status bar is hidden.

Messages from the app and its editors are shown by importance:

- A confirmation (`pushed`, `wrote 120 lines`) appears in the status bar after the mode for three seconds.
- A warning or error appears as a card in the window's top-right corner, newest on top, at most three at once. Each card has an icon and colored edge for its kind (yellow warning, red error), the text wrapped to a few lines, and the editor that sent it.
- A warning stays six seconds, an error eight. A card stays while the pointer rests on it; its `×` dismisses it, and a click anywhere else on it opens the history.
- `notifications { toasts #false }` shows warnings and errors in the status bar too. While the bar is hidden, every notice, confirmations included, is a card.
- The same message repeated in a row counts on one card, `(3x)`, instead of stacking copies.

Every notice is kept for the session. Unread errors and warnings put a dot, a count, and a color on the bell. Click the bell, press `C-M-m`, or run `Notifications: Show History` in the picker to open them in a Notifications tab beside the focused one (or go to the pane's existing one). This marks them read and takes the cards down. In the Notifications tab:

- Each notice is a card, newest first, grouped under Today, Yesterday, and Earlier. The top line shows its icon (red error, yellow warning), the sender, the repeat count (`×3`), and how long ago; the message is below; unread notices have a dot.
- A multi-line message is folded to its first line. `Enter`, or a second click on the card, unfolds it, and again folds it.
- The toolbar filters to errors, warnings, or the rest, each with its count; `Tab` and `S-Tab` step through the filters. `Clear all` at its right forgets them all, as `C` does.
- The tab updates as new notices arrive. `y` copies the message under the cursor, and `/` finds one.

`Notifications: Clear` forgets all notices from anywhere.

| Mode | Who owns the keyboard | Enter it with |
|---|---|---|
| **Insert** | The PTY. An ordinary terminal. | `i`, `a`, or `o` from Normal |
| **Normal** | Winter. Motions, operators, layout commands. | `Esc` or `M-ESC` from Insert |
| **Visual** | Normal, but motions extend a selection. | `v` `V` `C-v` from Normal |

Insert is the default, so Winter acts like a normal terminal until you leave it.

**Getting into Normal mode.** Press `Esc`, or `M-ESC` anywhere. The first `Esc` goes to Winter, even with a program like Claude Code running; press it again to send it to the program (see below). Programs with their own modes on `Esc` keep it: in vim, Neovim, Helix, Kakoune, Snow, and Emacs, or while a shell tab completion is open, `Esc` goes straight to the program and only `M-ESC` enters Normal mode.

**Getting out.** `M-ESC`, `i`, `a`, or `o` return to Insert. `Esc` in Normal stays in Normal and first clears Winter's own state: a half-typed command or count, a search, a quick-select, a selection. Only when there is nothing left to clear does `Esc` go to the program underneath (if one is running or a tab completion is pending). For example, to send `Esc` to Claude Code, press `Esc` twice.

Normal mode works with any shell, over `ssh`, and inside a Python REPL, with nothing installed on the far end.

## Keybinding reference

`normal`, `insert`, and `visual` use the built-in Vim-style bindings below and are not configurable yet. The `window` and `editing` chords further down are configurable in `keybindings.kdl`.

### Motions

Available in Normal and Visual mode. Most take a count prefix: `5j` moves down five lines, `3w` advances three words.

| Keys | Motion |
|---|---|
| `h` `j` `k` `l` | Left, down, up, right (arrow keys also work) |
| `w` `b` `e` | Word forward, word back, word end, over any punctuation between |
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
| `PageUp` `PageDown` or `C-b` `C-f` | Page up, page down |
| `C-u` `C-d` | Half page up, half page down |
| `C-y` `C-e` | Scroll one line up, one line down |
| `zz` `zt` `zb` | Scroll current line to centre, top, bottom |

### Character search

| Keys | Action |
|---|---|
| `f{char}` `F{char}` | Jump to the next or previous `{char}` on the line |
| `t{char}` `T{char}` | Jump just before the next or previous `{char}` |
| `;` `,` | Repeat the last character search, forward or reversed |

When the target character occurs more than once, Winter shows labels so you can pick a landing spot with one keystroke instead of repeating `;`.

### Operators and text objects

| Keys | Action |
|---|---|
| `d` `c` `y` | Delete, change, yank (followed by a motion or text object) |
| `yy` | Yank the whole line |
| `Y` | Yank from the cursor to the end of the line (as `y$`) |
| `"{reg}y{motion}` | Yank into register `{reg}` |
| `D` `C` | Delete, change to end of line |
| `dd` `cc` | Delete, change the whole line |
| `S` | Change the whole line |
| `x` | Delete the character under the cursor |
| `s` | Substitute the character under the cursor |
| `r{char}` | Replace the character under the cursor |
| `~` | Toggle the case of the character under the cursor |
| `.` | Repeat the last change |

Text objects follow `i` (inner) or `a` (around) after an operator: `diw` deletes a word and `ci"` changes the text inside quotes.

- `d` and `c` reach only the line the shell is editing (see [prompt line editing](#prompt-line-editing)).
- `y` reads the whole buffer, so `yiw`, `yap` and `yy` copy from the scrollback without entering Visual first. The yanked text stays highlighted for a moment.
- `ap` also takes the blank lines after the paragraph; over command output, that is one block plus the gap to the next.
- A sentence is read within its own row, and only a stop followed by a blank or the row's end ends it, so a version number or file name stays in one piece.

| Object | Selects |
|---|---|
| `w` `W` | Word, big word |
| `p` | Paragraph: the run of lines around the cursor that are all blank or all not |
| `s` | Sentence: up to a `.`, `!` or `?` and the quotes or brackets closing after it |
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
| `v` `V` `C-v` | Charwise, linewise, blockwise Visual |
| `M-i` | Select the paragraph under the cursor, linewise (vim's `vip`) |
| `o` | Move the cursor to the other end of the selection |
| `gv` | Restore the last Visual selection |
| `y` | Yank the selection and leave Visual |
| `Y` | Yank the whole lines the selection touches and leave Visual |
| `"{reg}y` | Yank the selection into register `{reg}` |

### Search

| Keys | Action |
|---|---|
| `/` `?` | Search forward, backward, typed in a find box at the pane's top-right |
| `n` `N` | Next match, previous match |
| `*` `#` | Search forward, backward for the word under the cursor |
| `Esc` | Close the search, keeping the cursor on the found match |

`n` and `N` still work after the find box is closed: the pattern and direction are kept, as in vim after `:nohlsearch`.

### Marks, registers, and history

| Keys | Action |
|---|---|
| `m{a-z}` | Set a mark |
| `` `{a-z} `` | Jump to a mark, exact column |
| `'{a-z}` | Jump to a mark, first non-blank |
| `` `` `` or `''` | Back to where the last jump started |
| `` `. `` | The last change |
| `` `^ `` | Where Insert was last left |
| `` `[ `` `` `] `` | The start and end of the last yank |
| `"{reg}` | Use register `{reg}` for the next yank or paste |
| `p` | Paste |
| `C-o` `C-i` | Jump backward, forward through the jumplist |
| `g;` `g,` | Step back, forward through the changelist |

Punctuation marks are set automatically, not by hand. Marks and the jumplist are per tab. Registers are shared across all tabs, so a yank in one tab pastes in another.

### `g` commands

| Keys | Action |
|---|---|
| `gt` `gT` | Next tab, previous tab |
| `g<` `g>` | Move the current tab left, right |
| `gx` | Open what is under the cursor: a URL in the browser, a directory in Dir, a path in the text editor over this pane, at the line a `path:line` reference names. `M-.` does the same from any mode (see the file browser below for where it looks without a cursor) |
| `gy` | Yank the block under the cursor |
| `gs` | Scout over the pane, as `C-j` does (see Pickers) |
| `gn` `gN` | Select the next, previous search match |
| `gv` | Restore the last Visual selection |
| `gp` `gP` | Jump to the prompt, to the previous prompt |
| `g;` `g,` | Step back, forward through the changelist |

### Blocks

Blocks come from [shell integration](#shell-integration); without it, the whole session is one rolling block.

| Keys | Action |
|---|---|
| `]b` `[b` | Next block, previous block |
| `gy` | Yank the block under the cursor |
| `q` | Quick-select: label every block, press a label to act on it |
| `F8` / `S-F8` | Open the next, previous file reference the last command or build printed |
| `C-S-b` | Choose one of the project's builds and run it in its Compile tab |
| `M-S-b` | Run the newest build again |
| `F6` | From the `.tex` or `.bib` file being edited, show where the cursor's line appears in its PDF (SyncTeX), in this pane |
| `S-F6`, `F7` | The same, with the PDF in the other pane |

`F8` finds the newest command output with a `path:line` or `path:line:col` reference to an existing file (compiler errors, test failures, linter findings). It resolves each against the directory the command ran in and opens each in turn in the text editor at its line (like Emacs' `next-error`).

- It keeps reading the same terminal while the text editor has focus, and starts over when newer output arrives.
- A build run in a [Compile](#editors) tab takes over: `F8` then walks the places that build reported, parsed in each tool's own format, until you press `F8` in a terminal, which switches back to that terminal's output.

### Prompt line editing

Normal-mode operators on the line the shell is editing are translated into the matching readline keystrokes, so `dw` on the prompt really deletes a word in your shell.

| Keys | Action |
|---|---|
| `C-/` | Undo your prompt edits |
| `C-\` | Redo them |

This assumes your shell's default emacs-mode bindings. **If your shell is in vi mode** (`bindkey -v`, `set editing-mode vi`), set `prompt-edit-bindings "none"`; Winter then leaves these operators to the shell, which gives you vim editing there anyway. All navigation over the screen and scrollback works the same either way.

### Window, pane, and tab chords

These work in any mode and are configurable in `keybindings.kdl`. Chords are written in Emacs notation:

- `C-` is Ctrl, `M-` is Meta/Alt, `S-` is Shift, `s-` is Super (the logo key), always in that order (`C-M-S-s-n`).
- Other keys use `RET`, `TAB`, `SPC`, `ESC`, `DEL` and `<up>`-style names.
- A letter is always lowercase, so Shift-s is `S-s`.

A binding may also be a sequence of up to four chords separated by spaces, such as `M-g M-h`.

- Sequences work the same in terminal and editor tabs, and several can share a first chord; after it, a hint lists what can follow.
- `ESC` cancels a sequence. A key that continues nothing cancels it with a notice (`M-g x is undefined`).
- An editor's own block (`git { "M-g r" "open_remote" }`) binds commands that apply only while that editor has focus, ahead of the window's. Keys lists those commands too.

You can rebind every editor's own keys the same way, by the id Keys lists beside each one: `commands "dir" { "toggle_archive" "M-z" }` makes `M-z` extract or compress in Dir instead of `Z`.

- Each part of an editor with its own keys has its own block, named after the editor and then the part: `dir` and `dir.edit` (renaming in place), `git`, `git.output`, and `git.search`, `editor` and `editor.markdown`, `editor.latex`, `editor.conflicts`, and the rest, each listed in Keys.
- A binding replaces every default key of the entry; `"<id>" "none"` removes its keys entirely.
- Vim's motions (`j`, `k`, `w`, `gg`, and the rest) are listed in Keys for reference but are built in and cannot be rebound.
- When `keybindings.kdl` has a mistake (a chord that does not parse, a command or editor block that does not exist, or two overlapping bindings where only one can run), the status bar reports it when the file loads.

| Chord | Action |
|---|---|
| `M-S--` / `M-S-\` | Split horizontally, vertically: the new pane opens a launcher (`RET` or `t` a terminal, `f` a file, `d` a directory, `g` git) in the current directory; the commands `Split Horizontal: Terminal` and `Split Vertical: Terminal` split straight to a shell |
| `M-S-=` | Zoom the focused pane (toggle) |
| `M-S-w` | Close the focused pane |
| `C-S-q` / `M-S-q` | Close the focused tab, close every other tab in its pane |
| `M-S-o` | Close every other pane |
| `M-h/j/k/l` | Move focus between panes |
| `M-1` .. `M-9` | Focus pane N |
| `C-M-1` .. `C-M-9` | Close pane N |
| `M-S-h/l` | Scroll page up, page down |
| `M-S-k/j` | Scroll line up, line down |
| `M-S-a` / `M-S-e` | Scroll to top, to bottom |
| `C-M-S-n` | New window |
| `C-S-n` / `C-S-w` | New tab, close tab |
| `C-<prior>` / `C-<next>` | Previous tab, next tab |
| `C-[` / `C-]` | Previous tab, next tab |
| `C-M-[` / `C-M-]` | Back to less recently used tab, forward to more recently used |
| `C-M-S-[` / `C-M-S-]` | Previous / next tab of the focused pane of the same kind as the focused one (terminal to terminal, text editor to text editor, Dir to Dir), wrapping around |
| `C-1` .. `C-9` | Go to tab N |
| `C-S-c` / `C-S-v` | Copy selection, paste |
| `C-,` | Open settings |
| `C-=` / `C--` / `C-0` | Font bigger, smaller, reset |
| `C-S-d` | Open Dir as a tab in the focused pane (toggle) |
| `C-S-g` | Open Git as a tab beside the work (toggle) |
| `C-S-k` | Kill the focused tab's buffer (closing a tab only buries it) |
| `C-x C-b` | Pick from every buffer: terminals, then buried buffers, then shown ones |
| `C-M-S-e` | Open the focused tab's file in another editor that can show it |
| `M-x` | Command picker |
| `C-S-r` | History picker |
| `C-S-j` | The dock along the bottom of the window: open it with a shell, hide it when it has the keyboard, else focus it |
| `C-S-z` | Go back to a recent tab, buried buffer, file, or directory |
| `C-M-z` | Find a file in the project and open it (`C-z` still goes to the shell) |
| `M-S-z` | Find a file under the focused tab's working directory and open it |
| `C-M-g` | Search the project as you type, and open a found line |
| `C-j` | Scout: search the tab's text as you type, each match shown in place |
| `C-M-r` | Resume the last picker as it was closed |
| `C-S-<up>/Down` | Previous, next prompt block |
| `C-DEL` | Delete the word before the cursor |

A single-chord binding whose action is not a built-in window action is looked up in the command picker instead, so `"M+q" "mux_new_session"` works.

## Pickers

Every list Winter offers is a picker: the command palette, the file pickers, project search, the recent list, the buffer list, the file browser, Scout, the outline, and every editor's question with a list of answers. A picker opens at once and fills as its rows arrive; the right of its title bar says where the selected row stands among those that match, `2/10`, with `…` while more are coming. Typing narrows the rows without ever making you wait.

The query is words separated by spaces, matched in any order: `pick rs` finds `src/picker/mod.rs`.

- A word matches where it appears, in any case.
- A word written `/like this/` is a regular expression.
- A word starting with `!` leaves out the rows it matches: `!test` hides every row holding `test`.
- Text in double quotes is matched as written, spaces included: `"fn main"`.

Project search and the file browser read their query their own way, as their sections say.

| Key | Action |
|---|---|
| `Enter` | Run the selected row's first action |
| `F4` or `C-Enter` | Show the selected row's other actions (open beside, kill a buffer); typing filters them |
| `F6` | Show or hide the side panel, a preview of the selected row beside or under the list; hidden until shown, and each picker remembers |
| `C-S-s` | Save the rows found to a Grep buffer (project search and the Todo picker) |
| `C-n` `C-p`, `Down` `Up` | Move the selection |
| `M-p` `M-n` | An earlier, a later query of this picker; each picker keeps its own |
| `Esc` or `C-g` | Close the picker, or the action menu over it |
| `C-M-r` (anywhere) | Resume the last picker closed, with its query and selection |

A dim line under the rows names the keys the picker offers there. The row under the selection shows its actions that are buttons, such as `×` to kill a buffer; a click runs one. The command palette and the file pickers rank what you use often and lately first. Every picker key, and every action a picker offers, is listed in the Keys editor and can be rebound in `keybindings.kdl` under the `picker` block (`picker.buffers`, `picker.files`, `picker.search`, `picker.browser` for the actions): `commands "picker.buffers" { "kill_buffer" "C-k" }` kills the selected buffer without the menu.

**Scout** (`C-j`, or `gs` in a terminal's Normal mode) searches the tab in front as you type: a terminal's lines and scrollback, a file's lines, a listing's entries. It sits along the bottom of the pane, each row a line's number and its text, and the pane above shows the selected match in place, every match on screen lit. `Enter` goes there for good, and `C-o` comes back; `Esc` puts the pane back where it was. Scout and the outline always preview this way, so `F6` does nothing in them.

**Todo picker** (`Search: Project TODOs and FIXMEs`) lists the comment keywords across the project as a picker, each row the place, then the comment with its keyword lit. Typing filters the comments; `C-S-s` saves them to a Todo list.

## Editors

An editor is a tab Winter paints itself: Dir, Git, Grep, the text editor, a PDF, an image, Processes, System, and Keys. Where an editor shows a place on disk, the top of its pane names it as breadcrumbs, so the title bar just reads Winter Term. Each opens as its own tab in a pane's strip, beside the terminal tabs, and the terminal it was opened from keeps running one tab away. Close an editor's tab with `C-S-q`. Pressing an editor's own chord while it is in front does nothing; from elsewhere in the pane it goes to the editor's tab. All window chords still work from an editor: split, zoom, close, and `M-h/j/k/l` to move focus.

`C-M-z` (`File: Find in Project`) lists every file in the focused tab's project so you can fuzzy-find one by its path. A project is the nearest folder at or above the tab's directory that holds a `.winter` folder or a git repository, else the directory itself. Every project-wide command (this list, project search, Grep, Todo, and the build) uses the same project. Its files are the ones git lists, minus what git ignores, else what `rg --files` lists. `Enter` opens the file in the editor that shows it, and `C-M-z` again closes the list. The window holds the chord, so it never reaches the shell, where it would arrive as `ESC C-z` and suspend the job like `C-z`; `C-z` itself still goes to the shell.

`M-S-z` (`File: Find File`) is the same picker limited to the focused tab's working directory: the files under the terminal's directory, or the one an editor is looking at, not the whole repository. `M-S-z` again closes it.

`C-M-g` (`Search: Project as You Type`) searches the same project for what you type. Once typing pauses on two or more characters, ripgrep runs and the matching lines stream into the list, each beside its file and line; the title counts how many are shown of how many found.

- The pattern is searched as written, except that `*` stands for anything.
- Case is ignored unless the pattern has a capital in it.
- A ` @ ` after the pattern starts a file filter: `foo @ src/*.rs` keeps lines in files matching the glob, and `foo @ recent` those whose path contains `recent`. Changing only the filter narrows the results without searching again.
- Each line found takes two rows: the file's name, line, and column (`mod.rs:184:13`) with its directory after them, then the line itself, cut so the match stays in view.
- `Enter` opens the file at the line, `M-p` and `M-n` bring back earlier queries, and `Esc` or `C-M-g` again stops the search and closes it.
- `C-S-s` keeps the lines found in a Grep buffer of their own, named after the query, where `C-x C-q` edits them in place and `C-r` searches again by the same rules. `C-M-r` brings the search back as it was, going on past where it stopped.

`C-S-z` (`Go To Recent: Tabs, Files, Directories…`) opens one list of everything to go back to: the tabs open in every pane, then the buried buffers, then recently opened files, then directories, newest first within each, each with its icon. `Enter` goes to a tab, brings a buried buffer back, opens a file in the editor that shows it, or lists a directory in Dir. A file held by a buried buffer is listed once, as that buffer. A file is remembered when an editor, the file browser, a click, or `$EDITOR` opens it; a directory is remembered when Dir lists it or a shell has been in it. Files and directories carry over between sessions, and ones that no longer exist are left out. The pane switcher is `Go to Pane` in the command picker.

`C-S-j` (`View: Toggle the Dock (Bottom Pane)`) toggles the dock.

- The first press opens a pane across the whole bottom of the window, a third of its height, with a shell started where the focused tab was working.
- The dock is a pane like any other: it takes tabs, splits, and editors, and keeps its height when the panes above it are split or balanced.
- Pressed while the dock has the keyboard, it hides the dock and gives the keyboard back to the pane that had it; the dock's tabs keep running unseen.
- Pressed anywhere else, it shows the dock, or focuses it if it is showing. Going to a tab that lives in a hidden dock also shows it.
- Closing the dock's last tab removes it.

An editor's contents are a buffer, kept separate from the tab showing it. Closing an editor's tab, alone or with its pane, buries the buffer instead of dropping it: a build keeps running, a listing keeps its place, a file keeps its edits and its undo, and a language server keeps the file open. Nothing is lost, so nothing is asked. `C-S-k` (`Kill Buffer`) lets a buffer go: it asks first when the buffer holds unsaved edits or a build or search is still running, then closes its tab. Quitting or reloading asks about unsaved edits in buried buffers as well as shown ones. A tab a program is waiting on, such as a `git commit` message, is killed when it closes instead of buried, so the program is not left waiting.

A terminal's tab is its shell, so it is never buried: closing it ends the shell, asking first when a program is running in it. To keep a shell alive after its tab closes, run it in a mux session.

`C-x C-b` (`Switch to Buffer…`) lists every buffer in a picker: the terminal tabs in pane order, then the buried buffers (most recently buried first), then the buffers a pane shows. Each row names the kind of editor, what it is showing, a `[+]` if it holds unsaved edits, and the directory it is looking at, so typing `main` finds the text editor on `main.rs`. `Enter` goes to a tab a pane already shows, or brings a buried buffer back into the focused pane as it was. `Esc` changes nothing.

`F1 h` (`Help: Keys of This Tab`) lists the keys of the tab in front, each beside what it does, in a picker. For an editor, it lists the editor's own keys for its current state (the Git status or the commit it is reading, a PDF with or without a cursor, Dir while renaming), then the motions all editors share. Over a terminal, it lists every window command bound to a key. Type to filter on what the keys do; `Esc` closes it. Editors do not list their keys in their header row. This list is read from the same table each editor looks its keys up in, so it never names a key the editor does not answer.

Every editor asks questions the same way, in the middle of the window. The status bar keeps showing what it was showing.

- A question with a known set of answers opens a picker: what the list is of on top, a filter line under it, and the matches below, narrowing as you type. `Up`/`Down` or `C-p`/`C-n` move through them, `Enter` takes the highlighted one, and `Esc` answers nothing.
- A question with no list, or whose answer is a name that does not exist yet, opens the same panel without the list: the question, the line you answer on, and how to answer it.
- A question that cannot be undone is answered by a single key instead of a line, and says so instead of drawing a caret.

Every editor answers to `M-S-,` and `M-S-.` for the top and bottom of what it shows (emacs' `M-<` and `M->`); Vim's `gg` and `G` reach the same two rows. The other Vim motions (`H` `M` `L`, `C-d` `C-u` `C-f` `C-b`) work in every editor too, and `C-r` re-reads or reruns whatever an editor shows. The Git view is the one exception: it binds some of these keys to its own commands.

Every editor whose cursor is a whole row (Git, Dir, Grep, Processes, System, Notifications) also has a column cursor along that row, so the Vim motions work there as in a terminal's Normal mode.

- `h` `l`, `0` `^` `$`, `w` `b` `e` and `W` `B` `E` (running onto the next or previous row at either end), and `f` `F` `t` `T` with `;` `,` move the column cursor.
- The editor's rows stay its own: `j` `k`, `gg` `G`, the paging chords, and `H` `M` `L` move its cursor through what it lists, and `Home` and `End` reach its first and last rows.
- A click puts the column cursor on the clicked cell and moves the editor's cursor to that row.
- `v` and `V` select from the column cursor.
- A key an editor binds for itself comes first: Dir's `h` and `l` still close and open a directory, Processes' fold its tree, and Git's `f`, `F`, and `t` still fetch, pull, and tag.
- Git's commands that would sit on Vim motion letters use modifiers: re-read on `C-r`, the branch menu on `C-M-b`, and the bisect, remote, and revert menus on `M-S-b`, `M-S-m`, and `M-S-v`.

Opening a file never opens it twice in one pane. `Enter` on a Git hunk, a Grep match, a Dir entry, a file reference, or a pick from the recent list, project files, or Open With first looks through the tabs of its pane, most recently shown first, for one that already has the file open: a text editor holding it in any of its buffers, or a viewer showing it. That tab is shown, with the buffer brought forward at the requested line (a PDF at the page). Only a pane with no such tab gets a new one.

A file is one buffer however many tabs show it. A text editor tab in another pane that opens a file already open elsewhere shows the same buffer instead of reading the file again.

- The text, its undo history, and whether it is saved belong to the buffer: an edit in one tab appears in the other at once (its cursor stays on the line it was on), `u` in either undoes the last change made in both, and a save in one is a save in all.
- Each tab keeps its own cursor, scroll, and selection.
- A language server hears of the file once, and hears that it is closed only when the last tab showing it closes.
- A buffer is a file's text; a terminal's screen and scrollback belong to that terminal.

Right-clicking inside an editor opens that editor's own menu, not the terminal's. The click first moves the cursor to the clicked row, so the chosen entry acts on what was clicked. The entries depend on that row: a commit cannot be staged, and a section heading has no path to copy. Each entry is a key the editor already binds, or a few keys typed in order, shown by name with those keys at its right, so the menu also teaches the keys. Entries come in groups separated by a line: reading or opening what was clicked, changing it, then copying what names it.

Every editor has a menu:

- Dir, Git, Grep, Processes, and System offer what can be done to the row under the pointer.
- The text editor follows its mode. Over a selection: Copy and Cut (through the clipboard register, so they reach other windows) and commenting. In Normal mode: Paste, Undo and Redo, commenting, Find, Save, and `$EDITOR`. In Insert mode: only entries that type nothing, plus entries that finish a commit message or a rebase plan. A right-click over a selection keeps it, so Copy has something to copy.
- The PDF viewer offers Copy over a selection (its right-click moves nothing), then Find, Go to Page, the outline, the zoom, and inverting the colors.
- The image viewer offers its zoom and what the image is.
- Keys runs or copies the command under the pointer, finds one, or closes.

Every editor searches the same way, and so does a terminal in Normal mode. `/` (or `?`, to search backward) opens a small find box at the top-right corner of the pane being searched. Type with the same keys every field takes; the pane's matches show as you type. `Enter` keeps the search and `Esc` drops it.

- `/` moves the cursor to the next row holding the text, and `n` and `N` step through the rest of the matches, wrapping at the ends. A search that finds nothing says so.
- The status bar shows the query and, where the editor counts them, which match the cursor is on out of how many.
- Matching is literal. A query in lower case matches either case; a query with a capital in it matches exactly, the same rule the text editor and the PDF viewer use.
- A directory listing matches on the entry's name; every other editor matches the whole row as painted, so a chord is as findable as the command beside it.
- A search covers the focused tab only, never the whole window: other panes in view light nothing.
- Matches are lit as in a terminal, from the first letter typed: every match on screen in one color, the one the cursor is on in another. They stay lit after `Enter` until `Esc` clears them; `n` and `N` light them again.
- A PDF lights only the match the cursor lands on, since its pages are pictures, though it counts them all.
- A search moves only the cursor: marks, folds, and everything staged stay as they were.
- In Grep, whose rows already are a search, `/` asks for a new search.

**Dir** (`C-S-d`, or `Dir: Open Working Directory`) lists a directory, rooted at the working directory of the pane it opened from.

The listing starts with `./`, the directory listed, and `../`, its parent, as dired does: `Enter` on `../` goes up, and on `./` reads the listing again. Neither is ever marked, renamed, or acted on by an operation; with the cursor on one, an operation has nothing to act on. A directory opens with the cursor on its first entry.

The root's path runs along the top of the pane as breadcrumbs, one to a folder; click one to list that folder, with the folder you came up from selected. While sizes or details are on, the column names stand on the same row, each over its column, and stay put as the listing scrolls; a long path gives up its outer folders first to make room.

| Key | Action |
|---|---|
| `j` `k` or `Down` `Up` | Move down, up |
| `Home` / `End` | First entry, last entry |
| `Enter` | Enter a directory, or open a file in the text editor |
| `C-o` | Open the file under the cursor in `$EDITOR`, in a new tab |
| `l` / `Right` | Open the directory under the cursor, in place |
| `h` / `Left` | Close it, else step out to the parent row, else leave the root |
| `Backspace` | Leave the root for its parent |
| `M-n` / `M-p` | Next, previous entry at the same level |
| `M-u` / `M-d` | Move to the parent, into an open directory |
| `M-S-b` / `M-S-f` | Back, forward through directories visited |
| `/` | Search the listing |
| `n` / `N` | Next, previous match |
| `Tab` / `S-TAB` | Fold the entry, fold everything |
| `z u` / `z f` / `z t` | Open, close, toggle the whole subtree |
| `z a` / `z c` | Toggle everything, close everything |
| `C-c 0` .. `C-c 9` | Open the tree to that depth |
| `.` | Show or hide dotfiles |
| `,` | Show or hide the size, age, and permission columns |
| `M-S-s` | Show directory sizes; pressing it again stops the walks |
| `s` | Cycle the sort: name, time, size |
| `C-r` | Re-read the listing |
| `m` / `u` | Mark, unmark the entry and step on |
| `t` / `U` | Mark everything listed, unmark everything |
| `_` / `+` | New file, which opens in the text editor; new directory |
| `R` | Rename the entry under the cursor |
| `C` / `M-m` | Copy, move the targets: one takes a new name, several take a directory chosen from a list |
| `x` | Move the targets to the trash, after confirming |
| `X` | Delete the targets for good, after confirming |
| `*` | Set permission bits, as octal |
| `&` | Open the entry with the system handler |
| `=` | Compare the two marked entries, files or directories, in a diff beside the listing |

The filesystem does not know a directory's size, so `M-S-s` walks each directory in the background, one at a time, showing `…` until its total arrives. Turning it off stops any walks still running. Totals are kept while you stay in the same directory and dropped when the root moves.

An operation acts on the marked entries, or on the entry under the cursor when nothing is marked, never both. The status bar shows how many are marked and what the last operation reported.

- A name typed into a prompt must be a plain name: `../elsewhere` is refused, and nothing overwrites an existing entry.
- Copy and move take a new name for one target and a destination directory for several.
- When another pane on screen shows a directory (a listing, or an editor rooted somewhere), it is offered first: the prompt for one target starts out holding its path, and the list for several puts it at the top.
- A full path typed out is taken as a directory to copy or move into, keeping the entries' own names.

`x` sends what it removes to the desktop trash (the freedesktop.org one, in `~/.local/share/Trash`) with a record of where each entry came from, so a file manager can restore it; `X` deletes for good. Where there is no trash, `x` says so instead of deleting.

Every row has a gutter, one column at the left edge (or the right, with `gutter-side "right"`), holding an icon: a check on an entry marked for an operation, else, in a git checkout, the entry's state: modified, added, deleted, renamed, untracked, in conflict, or ignored. A directory shows the most pressing change inside it, and entries inside an untracked or ignored directory show that directory's state. Clean, unmarked entries leave it blank. The column is kept whether or not anything is marked or changed, so the names never move when the first mark arrives. The git states are read in the background whenever the listing is, so they follow edits and stages made anywhere. Moving the root clears the operation marks, so they never count toward an operation in a different listing.

`C-x C-q` turns every listed name into editable text, like Emacs' `wdired`, to rename many entries at once.

- The names take the text editor's keys: Vim in Normal and Visual mode, the Emacs chords while typing.
- Each line stands for its entry, so nothing adds or removes one: `dd` empties a name instead of pulling the names below up, and `o`, `J`, and a line-joining `C-k` do nothing.
- `ZZ`, `C-s`, `C-S-c`, or `Enter` while typing applies the changes as renames. Before anything moves, the whole batch is refused if any name is not a valid name or two land on the same one.
- A batch of more than one rename is listed for a yes first.
- Swapping two names, or a longer cycle, works: entries waiting for a name another entry still holds are parked under a temporary name until it is free.
- `C-S-k`, `q` with nothing part-typed, `ZQ`, or a second `C-x C-q` throws the changes away.

The same Emacs chords edit the answer in any prompt an editor asks, such as a search or a new name. `Up` and `Down` (or `C-p` and `C-n`) bring back earlier answers to the same question, newest first, like a shell's history.

Directories sort before files whatever the sort key. Moving the root, toggling a view option, or folding keeps the cursor on the entry it was on. A block cursor sits on the active row's first non-blank column and follows it as you move, showing where a selection would start. `v` starts selecting from there, for copying out of a listing, and `V` does the same by whole lines:

- `hjkl` and the arrows move and extend, `w` and `b` step words, and `0`, `$`, `g` and `G` jump.
- `V` and `v` switch between whole lines and characters; a second press of the same one drops the selection.
- `y` or `Enter` copies and leaves, and `Esc` cancels.
- While selecting, keys go to the selection before the editor, so the motions mean what they do in Vim, not what the listing binds them to. Chorded keys still reach the window, so splitting, zooming, and moving focus work from inside it.
- Dragging with the mouse selects the same rows.

Each entry has an icon for what it is, from bundled artwork keyed by extension, exact filename, or directory name; it needs nothing from the terminal font. Popups that list files, such as the file browser, the recent directories, and the text editor's open buffers, draw the same icons.

**Actions menu.** Press `a` in Dir (or choose Actions… on the right-click menu) to list every action for the marked entries, or for the entry under the cursor. Each action has one letter:

- `o` open, `e` open in its own application, `h` open in the hex editor.
- `r` rename, `c` copy, `m` move.
- `d` move to the trash, `D` delete for good.
- `p` change permissions.
- `u` extract (offered for an archive), `z` compress into an archive.
- `=` compare (offered for two or three marked files).
- Inside a git repository: `M` move through git (`git mv`, the index following), `a` add though ignored (`git add -f`).

Only the actions that apply are offered; opening needs a single entry, and the git actions a repository. The menu is a transient window, as Git's are: a letter runs its action at once, or `Down` and `Up` (`C-n`, `C-p`) move to one and `RET` runs it, and a click on a row runs that one. Any other key says it is no action here and leaves the menu open; `Esc` (or `C-g`) closes it. The letters are the `dir.actions` block of `keybindings.kdl`, listed in Keys.

**Archives and two panes in Dir.** `Z` extracts an archive in place (`.tar.gz`, `.tar.xz`, `.zip`, `.7z`, `.rar`, `.gz`, …), or compresses a file (`gzip`, keeping the original) or a directory (`.tar.gz`) beside itself. `c` puts the marked entries into one archive whose name shows its kind. `Dir: Open Two-Pane File Manager` opens two listings side by side: each copies and moves to the other's directory by default, and `=` with nothing marked compares the two directories.

**Git** (`C-S-g`, or `Git: Status`) shows the working tree of the repository the pane's directory is in, as foldable sections: the changes, then the stashes, then what the upstream has that you do not, then the recent commits. A file changed both in the index and in the working tree appears in both, so you can stage one half without the other. Every command runs from the repository root, off the event-loop thread, and the view re-reads the tree after anything that changed it. What a command reports, including a failure, shows in the status bar.

| Key | Action |
|---|---|
| `j` `k` or `Down` `Up` | Move down, up |
| `]` / `[` | Next, previous entry, skipping blank lines |
| `g` | Jump: `s` staged, `u` unstaged, `n` untracked, `z` stashes, `c` recent commits, `g` top, `j`/`k` next, previous entry; `r` re-reads the working tree |
| `z` | Fold: `a` fold or unfold what the cursor is on, `o` unfold it, `c` fold it; `z` `t` `b` scroll its row to the middle, top, bottom |
| `/` | Search the view, or the log or listing filling it |
| `n` / `N` | Next, previous match |
| `Tab` / `S-TAB` | Fold the section, show or hide every file's diff in every section |
| `Enter` | Open the file under the cursor in the text editor; from a hunk, at the line it changes |
| `C-o` | Open it in `$EDITOR` instead, in a new tab |
| `Enter` on a commit | Read the commit: message, then the files it changed, each opening to its patch |
| `Enter` in that patch | Open the file the row belongs to, at the line its hunk changes |
| `Enter` on a stash | Read the stash as a diff |
| `.` | File: what can be done with the file under the cursor |
| `Tab` on a file | Show its diff, hunk by hunk |
| `s` / `S` | Stage the target, stage everything |
| `u` / `U` | Unstage the target, unstage everything |
| `x` | Discard the target, after confirming; on a stash, drop it, and on the Stashes heading, drop them all |
| `a` / `-` | Apply, reverse the hunk under the cursor; `a` on a stash applies it with what it had staged, and on a commit brings its changes in uncommitted, which `-` takes back out |
| `y` | Yank: `s` the hash or path under the cursor, `y` the line as it reads, `r` the branches and tags (below) |
| `P` or `p` | Push menu: `p` to the push remote and `u` to the upstream (each set first where it is not), `e` elsewhere, `o` another branch or commit, `T` a tag, `t` every tag; `-` then `f` `F` `i` `h` `d` toggles force-with-lease, force, force-if-includes, no hooks, dry run, and `RET` goes back |
| `F` | Pull menu: `u` the upstream with rebase, `m` the upstream with a merge, `e` a remote branch picked from a list, with rebase |
| `f` | Fetch menu: `u` the upstream's remote, `e` a remote picked from a list, `a` every remote, `s` every remote with its submodules |
| `C-S-p` | Quick push: the current branch to the remote it tracks (else `origin`), setting it to track, with no questions |
| `C-S-M-f` | Quick pull with rebase: from the branch the current one tracks, else the same name on `origin`, with no questions |
| `+` | Show more of the recent commits, 50 at a time, while the list runs on |
| `C-S-c` | Commit what is staged, writing the message in the commit editor |
| `C-S-s` | Stage all: untracked files when the cursor is in that section, tracked changes otherwise |
| `C-S-a` | Stage every untracked file, wherever the cursor is |
| `!` | Run any git command |
| `M-b` | Blame the file under the cursor |
| `y r` | The branches and tags, in a tab of their own: local, remote, then tags; `Enter` reads one's log, `x` deletes it after confirming (a remote branch by pushing its deletion), `y` copies its name |
| `M-g r` | Open the remote in a browser (rebindable in the `git` block of `keybindings.kdl`) |
| `C-g` | Stop the git command running (a push, a fetch, a hook), with everything it started; git commands have no time limit. Works in any editor tab with work running |

The view uses color as well as names:

- A section heading takes the hue of what sits under it: cyan staged, yellow unstaged, green untracked, red contested.
- Every other heading and the header block's own labels are blue.
- A file shows what changed in it in that change's own hue.
- A branch is magenta, a remote branch green, and a tag cyan.
- Anything the upstream has not seen yet (the commits ahead of it, and the count of them) is red.

When git is part-way through a merge, a rebase, a cherry-pick, a revert, or a bisect, a `State:` line in the header says so: what it is working on, how far along it is (where git counts the steps), and the keys that finish it. A rebase, and a cherry-pick or a revert of several commits, also get their own section at the top of the view listing the commits they work through: the ones done, the one stopped on, and the ones still to come. Each row is its commit, so `Enter` reads it, `y` copies its hash, and the other keys that act on a commit act on it. All of this is read from the files git writes under the repository's own directory.

Closing a tab, a pane, the other tabs or panes, or the whole window asks first when a terminal has a program running in it (an agent such as Claude Code, an editor, a build; a shell sitting at its prompt does not count), or when an editor tab holds unsaved edits. Press `y` to close; any other key cancels. Naming the running program uses the process list, so on platforms without `/proc` the question says "a program" instead, and a program that does not own the screen may go unnoticed.

With more than one pane, the status view and the commit editor open in another pane, not the one you are working in: the neighbor in the same row (right first, else left) if there is one, otherwise the next pane in order. Asking for the status view again goes to the one already open there. With a single pane, a new pane is split off to its right for them.

The status view watches the repository through the platform's file notifications. It reads git's status again three seconds after the changes go quiet, and redraws only when the status differs: an edit, a stage from another terminal, a commit, a fetch.

- Build output and dependency directories at the top of the tree (`target`, `node_modules`, `dist`, `build`, and the like) are not watched.
- Inside `.git`, only the index, `HEAD`, the references, and the merge state count.
- Where the platform cannot be watched, it reads on a three-second timer instead.
- A log, a diff, or any other output view is left alone while you read it.
- An open file diff is re-read on each pass, since the status cannot say that a file changed again.

A triangle marks every row a fold key acts on, pointing down while its contents show and right while they are hidden: section headings, file rows, and a commit's hunk headers all carry one. A hunk in the status view has none, since `Tab` there folds the file it belongs to. Opening a row brings what it opened into view: a block that would run past the pane's bottom pulls its own row to the top, while one that already shows in full leaves the view where it was.

On a section heading, `s`, `u`, and `x` act on every file in that section. On a hunk, or any line inside it, they act on that hunk alone: the patch is built from the hunk and fed to `git apply` on standard input, so the rest of the file is untouched. A `v` or `V` selection works on single lines: `s`, `u`, or `x` over it stages, unstages, or discards just the lines it covers. The changes left out are written so the patch still applies (an unselected addition is dropped, an unselected removal stays as context), and the header's counts are recomputed. Discarding asks first. A selection reaching past one hunk acts on every file it covers instead. Either way, the selection ends.

On a file in conflict, `.` then `o` or `t` resolves it by taking one side whole, ours or theirs, and stages the result, which marks the conflict resolved. Discarding an untracked file deletes it, since `git restore` cannot reach a path that is not in the index.

A command that takes a name offers a list instead of a blank line: checking out, merging, or rebasing onto a branch offers every branch; deleting a branch offers the local branches; deleting a tag offers the tags; removing or pruning a remote offers the remotes. Where there is nothing to list, you type the answer.

Every key that waits for a second key names what that key may be, on the same card the terminal's key hints use: centred over the window, above whatever is under it, so a long listing cannot push it off the bottom. It shows at once, without a pause. Any key the menu does not offer closes it without doing anything, so a mistyped second key is harmless.

These keys open a menu, and the next key picks from it:

| Key | Menu |
|---|---|
| `b` | Branch: checkout, create, delete, rename this branch (`m`), or spin off (`s`): a new branch where HEAD is, checked out, with this one put back where its upstream is |
| `c` | Commit: in an editor, amend, amend keeping the message, reword the last message (`c w`, leaving anything staged out of it), a fixup of the commit at the cursor (`c f`), or one squashed in at once (`c F`); `-` then `a` `e` `s` `n` `S` turns on `--all`, `--allow-empty`, `--signoff`, `--no-verify`, `--gpg-sign`, which stay on until turned off |
| `d` | Diff: working tree, staged, against a revision, a range `a..b` (`R`), this branch against another as a pull request shows it (`b`), or a commit (`c`) |
| `i` | Ignore: this path, this extension |
| `m` | Merge: merge, merge editing the message (`e`), merge without committing (`n`), squash in (`s`), continue, abort; `-` then `f` `n` turns on `--ff-only`, `--no-ff` |
| `r` | Rebase: interactive from the commit under the cursor (that commit and every one after it; off a commit, onto a branch picked from a list), onto upstream, continue, skip, abort |
| `t` | Tag: create, delete, list |
| `Z` | Stash: push everything, the index alone (`i`), keeping the index (`x`), untracked files too (`u`); pop, apply, or drop the stash at the cursor, else the newest; list |
| `A` | Cherry-pick: pick, continue, abort |
| `B` | Bisect: start, good, bad, reset |
| `M` | Remote: list, add, remove, prune |
| `O` | Reset: mixed, soft, hard, the index alone (`i`), the working tree alone (`w`) |
| `_` | Revert: revert, continue, abort |
| `%` | Worktree: list, add, remove |
| `'` | Submodule: list, update (init, recursive), add, sync the URLs, deinit |
| `C-l` | Log: this branch (`l`), another branch or revision (`o`), every local branch (`L`), every branch (`b`), all refs (`a`), this file (`f`), or this branch's commits whose changes match a regex (`g`, `-G`) or add or remove a string (`s`, `-S`); `-` then `f` `m` `d` `r` turns on `--first-parent`, `--no-merges`, `--simplify-by-decoration`, `--reverse`. `+` loads more of whichever log is up |

Anything that needs a terminal of its own opens in a new tab: commit and amend in `$EDITOR`, and interactive rebase with its todo list. `o` resets to the commit under the cursor, or asks which one when the cursor is elsewhere.

A commit, a stash, a log, a listing, a blame, or a diff opens in a Git tab of its own beside the status, which stays as it is; asking for one already open goes to its tab. In these tabs:

- `j`/`k` move through it, and `/` searches it.
- `+` asks for more of a log.
- `Enter` on a log's commit opens that commit in a tab of its own.
- `y` copies the first field of a line, which is the commit hash in a log or a blame.
- `q` closes the tab and returns to the one it was opened from.
- In a blame, where every row is a line of one file, `Enter` opens that file at the row under the cursor, and `C-o` opens it there in `$EDITOR`.

A commit read with `Enter` opens folded, like the status view: the summary, then a `Changes` heading counting the files it touched, then one band per file. Each band counts the hunks it holds and says what the commit did to the file (`modified`, `new file`, `deleted`, `renamed`) after its fold triangle and before its icon and path, with the patch hidden behind it. It folds the same way too:

- `Tab` on the heading hides or shows the whole file list.
- `Tab` on a file band opens it to its hunk headers, or closes it again.
- `Tab` on a hunk opens that hunk's body.
- `S-TAB` acts on the whole commit at once: it opens every file and hunk, or, when none is folded, closes all of them, every hunk too.

In Git, `=` on a file opens it in the Diff editor, comparing the last commit's copy against the working tree's; you can edit it there and write it back.

**Compile** (`C-S-b`, or `Compile: Run a Task…`) runs a build in its own tab and turns every place its output points at into a link.

- If the project has only one task, `C-S-b` runs it at once. Its Compile tab goes into the focused pane's strip behind your work, and the status bar says how it ended.
- If the project has several tasks, `C-S-b` lists them first and opens nothing until you choose one. The task run last leads the list, marked. The last entry asks for a command line instead; `Compile: Run a Command…` goes straight there.
- By default the build's tab shows in the dock panel along the bottom of the window, and the focus stays where it was. Set `compile { dock #false }` to split beside instead.
- Every file being edited in the text editor is saved before a build starts.
- Each task has its own tab, so a test run never replaces a build still going. Running a task again reuses its tab.
- `M-S-b` (`Compile: Run the Last Build Again`) reruns the newest build from any tab.

The output streams in while the build runs, in the tools' own colors. Progress lines that a tool redraws in place stay one line each. The cursor follows the end of the output until you move it, and the tab spins on the strip while the build runs. When a build ends, Winter tells you wherever you are: a failure shows as a toast with its error and warning counts, and by default a failed build opens its first error in the file you are in. The build runs under a hidden terminal in the project's root (the nearest directory with a `.winter` folder or a `.git`), passed to `sh -c` (`cmd /C` on Windows), with pagers turned off.

| Key | Action |
|---|---|
| `Enter`, or a double-click | Open the place on this line beside the tab, at its line and column |
| `C-o` | Open it in `$EDITOR` instead |
| `y` | Copy the place's `path:line` |
| `M-n` / `M-p` | Next, previous error or warning |
| `e` | Show only errors and warnings (each with its message line), or everything again |
| `C-r` | Run the build again |
| `c` | Run another command in this tab, starting from this one |
| `i` | Type a line to the running build, for a tool that asks |
| `C-g` | Stop the build and everything it started |
| `C-c C-v` | Open the PDF a LaTeX build made |
| `/`, `n` / `N` | Search the output |

The tasks offered come in this order:

1. The document a `.tex` file belongs to, when one is focused.
2. The tasks listed in `.winter/tasks.kdl` in the project's root.
3. Tasks suggested by the project's files: `cargo build`, `cargo test`, and `cargo clippy` beside a `Cargo.toml`; `make -k` beside a makefile; `go build` and `go test` beside a `go.mod`; the `build`, `test`, and `lint` scripts a `package.json` defines; `latexmk` beside a `latexmkrc`.

Winter remembers the command each project was last built with. A task list names one shell command per task, run in the root or in the directory `cwd` names. `default=#true` marks the task a project builds with until you choose another. `on-save=#true` marks a task that runs whenever a file of the project is saved in Winter. `env` children set variables for the task:

```kdl
task "build" "cargo build --workspace" default=#true
task "check" "cargo check" on-save=#true
task "docs" "make html" cwd="docs" {
    env SPHINXOPTS="-W"
}
```

Winter reads a place in the format each tool prints:

- GNU's `path:line:col: error: message` (gcc, clang, go, pytest, most linters, and TeX's `-file-line-error`).
- rustc's `-->` under its `error:` or `warning:` line.
- MSVC's `path(line,col): error`.
- Python's `File "path", line N`.

Errors are red, warnings yellow, and notes blue. The place `F8` or `Enter` opened last is marked in the output. A path is read relative to the directory `make` says it entered; otherwise relative to where the build ran; otherwise relative to a directory up to four levels above that (where cargo prints a workspace member's paths from).

To build LaTeX, use `LaTeX: Compile Document`, or the first task `C-S-b` offers from a `.tex` file. It runs `latexmk -pdf -interaction=nonstopmode -file-line-error -synctex=1` over the document's root (found the way `F6` finds it), with TeX told not to fold its lines. After the build, Winter reads TeX's log for what it did not print as a place: warnings from LaTeX, its packages, and its classes, and the boxes it could not set. Each is placed in the file TeX was reading and listed under a count after the build's own output. A PDF tab showing the document reloads the new one by itself; where none does, `C-c C-v` opens it. The `compile` block of `settings.kdl` sets:

- `latex-command`: the LaTeX command.
- `latex-on-save`: whether saving a `.tex` file in Winter rebuilds its document (off by default).
- `jump-to-error`: whether a failed build opens its first error (on by default).

**Grep** (`Grep: Search Files` in the command picker, or `C-S-s` in project search to keep what it found) finds the lines in the focused tab's project that contain some text, grouped under one heading per file.

| Key | Action |
|---|---|
| `/` | Search for text, starting from your last search |
| `j` `k` or `Down` `Up` | Move down, up; the vim motions (`gg`, `G`, `C-d`, `C-u`) work too |
| `n` / `N` | Next, previous match, skipping the file names between them |
| `Home` / `End` | First row, last row |
| `M-n` / `M-p` | Next, previous file |
| `Enter` | Open the file in the text editor, at the matching line |
| `C-o` | Open it in `$EDITOR` instead, at the same line |
| `y` | Copy the match's `path:line` |
| `C-r` | Run the same search again |
| `Esc` | Stop a running search |
| `C-x C-q` | Edit the matched lines in place and write them back to their files |
| `M-c` / `M-r` / `M-w` | Toggle match case, regular expression, whole word, and search again |
| `M-f` | Only the files a glob matches (`*.rs`); empty for every file |
| `M-t` | Only files of the type under the cursor, or every file again |
| `M-h` | An earlier search, newest first |

If [ripgrep](https://github.com/BurntSushi/ripgrep) is installed, the search runs through it, which reads the tree's `.gitignore` files and is faster on a large tree. Otherwise Winter walks the tree itself. Either way the rules are the same: literal text in any case, hidden files read, and version-control and build directories skipped.

`C-x C-q` turns the matched lines into editable text, like `wgrep`, with the text editor's keys.

- `ZZ`, `C-s`, `C-S-c`, or `Enter` while typing writes every changed line back to its file. Each file is read and written once through the text editor's own save, so line endings and permissions survive.
- A line whose file no longer says what the search found there (changed since, or cut short in the list) is left alone and counted in the report.
- `ZQ`, `C-S-k`, or `q` with nothing part-typed throws the edits away.
- As in every edit mode, `C-S-c` finishes and `C-S-k` throws away.

The walk runs off the event loop, so the pane stays live while a large tree is read, and `Esc` abandons it. Here `/` asks for a new search instead of searching the rows, and `n` and `N` step through what it found. Matching is literal text, in any case.

The walk skips `.git`, `.hg`, `.svn`, `node_modules`, and `target`, files over a megabyte, anything that does not read as text, and symlinks, which are never followed. It stops at 500 matches and says so in the header; narrow the query if you hit that.

**Todo** (`Todo: List Keywords in Project` in the command picker; no default chord) is a Grep over the focused tab's project for the comment keywords that `todo-keywords` names in `settings.kdl` (`TODO`, `FIXME`, `XXX`, `HACK`, and `BUG` by default). It runs as soon as it opens. A keyword matches as a whole word in its own case, so `TODO` finds `// TODO:` and `TODO(me)` but never `todos` or `TODOS`. Its keys are Grep's: `Enter` opens the line, `C-r` searches again, `C-x C-q` edits the lines in place, and `/` asks for other keywords, separated by spaces, for this list only.

**Processes** (`C-S-,`, or `Processes: Monitor`) lists every process on the machine, refreshed every two seconds.

- CPU use is a share of one core, so a busy multi-threaded process can pass 100.
- The columns are PID, NAME, CPU%, MEM%, RSS, VRAM, STATE, USER, and the command line.
- By default the list is a tree of which process started which, sorted by CPU within each level. The tree shows in the NAME column as an indent and a fold marker, so the command stays whole. `=` flattens it into one sorted list.
- Linux reads `/proc`, macOS and the BSDs read `ps`, and Windows asks PowerShell. On Windows the working-directory column is missing and the state is always `run`.
- A `VRAM` column of per-process video memory appears on any platform when `nvidia-smi` is on the path and answers. AMD and Intel GPUs show no column.

| Key | Action |
|---|---|
| `j` `k` or `Down` `Up` | Move down, up; the vim motions (`gg`, `G`, `C-d`, `C-u`) work too |
| `s` / `S` | Sort by the next column, reverse the direction |
| Click a heading | Sort by that column; click the sorted one again to reverse it |
| Click a process | Move the cursor onto it |
| `=` | Switch between the tree and one sorted list |
| `Enter` `Space` `Tab` | Fold or unfold the children of the process under the cursor |
| `h` / `l` | Fold, unfold |
| `/` / `n` `N` | Search by name, command, pid, or user, moving the cursor to the match; next, previous match |
| `f` | Filter by name, command, pid, or user; a match keeps its ancestor processes |
| `Esc` | Clear the filter |
| `x` / `X` | Terminate, kill (asks first) |
| `t` / `T` | Terminate, kill the process and everything under it (asks first) |
| `z` / `Z` | Suspend, resume (not on Windows) |
| `R` | Renice: a nice value from -20 to 19, or a priority class on Windows |
| `r` or `C-r` / `p` | Refresh now, pause or resume refreshing |
| `y` / `Y` | Copy the pid, the command line |

A strip under the list describes the process under the cursor: pid, parent, owner, state, CPU, memory and video memory shares, executable path, working directory, and the full command line. It is hidden in a pane shorter than twelve rows.

Signals go through the system's own `kill`, `renice`, and `taskkill`, so Winter needs no special privilege and is refused exactly where those are: another user's process needs `sudo` from a terminal. On Windows, terminate only asks a process's windows to close, which a console or background process declines; `X` forces it.

The toggles change how the query is read. By default it is text found anywhere, in any case. `M-c` makes case count, `M-r` reads it as a regular expression (Rust's syntax, which ripgrep shares), and `M-w` matches it as a whole word. The header shows how the search is reading when that is not the default. A regular expression that does not parse is reported in the status bar. The Todo list always reads its keywords as whole words in their own case, so the toggles do nothing there.

**Docker** (`Docker: Containers, Images, Volumes, Networks` in the command picker; no default chord) lists what the docker daemon knows, one kind at a time, refreshed every three seconds: containers (running or not, with their image, state, status, ports, and compose project), images, volumes, and networks. `Tab` and `S-TAB` step between them. Everything goes through the `docker` command, so the view needs no more access to the daemon than the shell it started from. When it cannot reach the daemon, it shows what docker said.

| Key | Action |
|---|---|
| `Tab` / `S-TAB` | Next, previous section |
| `Enter` | Inspect the object under the cursor |
| `r` / `C-r` | Refresh now |
| `/`, `n` / `N` | Search, next and previous match |
| `y` / `Y` | Copy the name, the id |
| `D` | Remove the object, after confirming |
| `C` | Prune: remove every stopped container, dangling image, or unused volume or network, after confirming |
| `s` / `x` / `R` | Start, stop, restart the container |
| `p` / `K` | Pause or unpause, kill (after confirming) the container |
| `e` / `a` / `L` | Open a shell in the container, attach to it, follow its logs, each in a new terminal tab |
| `o` / `t` / `S` | Read its last 500 log lines, its processes, its CPU and memory, in the view |
| `P` / `H` | Pull the image again, read its layers |

Read-only output (an inspect, a page of logs, a container's processes) replaces the list in the view, with search and the motions over it; `r` reads it again and `q` goes back. A shell opens bash if the image has it, and `sh` otherwise.

**System** (`System: Monitor` in the command picker; no default chord) shows the whole machine, refreshed every two seconds:

- Host, operating system, uptime, and load average.
- Processor use as a total and a grid of cores with their clocks.
- Memory and swap.
- Each NVIDIA GPU's video memory, load, and temperature. This section needs `nvidia-smi` on the path and is hidden otherwise.
- The local disks, with a bar for each.

Bars turn yellow at 75 percent and red at 90. Linux reads `/proc` and `df`, macOS and the BSDs read `sysctl`, `vm_stat`, `ps` and `df`, and Windows asks PowerShell. macOS shows only the total processor use, worked out from what the processes are using, since it has no per-core counter without a system library.

| Key | Action |
|---|---|
| `j` `k` or `Down` `Up` | Scroll down, up; the vim motions (`gg`, `G`, `C-d`, `C-u`, `C-f`, `C-b`) work too |
| `C-e` / `C-y` | Scroll one line down, up |
| `n` / `N` | Jump to the next, previous section |
| `r` or `C-r` / `p` | Refresh now, pause or resume refreshing |

**Text Editor** opens the file an editor was pointing at, as editable text. It has no chord of its own: `Enter` on a file in Dir, Git, or Grep opens it, at the line that editor knew about. It opens as a tab right after the editor you came from, so closing it puts the listing back where you left it. The path to the file runs along the top of the pane as breadcrumbs, the file's name last; click a folder to list it in Dir.

| Key | Action |
|---|---|
| `h` `j` `k` `l` or the arrows | Move by character and line |
| `w` `W` / `b` `B` / `e` `E` | Word starts and ends, crossing lines |
| `ge` / `gE` | Back to the end of the previous word |
| `0` / `^` / `$` / `g_` | Column one, first non-blank, end of line, last non-blank |
| `{` / `}` | Previous, next paragraph: the blank line past the text |
| `%` | The bracket matching the first one at or right of the cursor |
| `f` `F` `t` `T` / `;` `,` | To a character on this line; repeat, or repeat backward |
| `gg` / `G` / `20G` | First line, last line, line twenty |
| `C-d` / `C-u` / `C-f` / `C-b` | Half a screen, a screen, either way |
| `H` / `M` / `L` | Top, middle, bottom of the screen |
| `zz` / `zt` / `zb` | Scroll this line to the middle, top, bottom of the pane |
| `m{a-z}` / `'{a-z}` / `` `{a-z} `` | Mark this place; back to it, by line or exactly |
| `''` | Back to where the last jump started |
| `/` `?` / `n` `N` / `*` `#` | Search forward, back; next, previous match; the word under the cursor |
| `i` `a` / `I` `A` | Type here, after here; at the first non-blank, at the end |
| `o` / `O` | Open a line below, above, keeping the indent |
| `v` / `V` | Select from here, by character or by whole lines |
| `R` | Type over existing text instead of inserting |
| `x` `X` / `r` / `~` | Delete a character here, before here; replace one; flip its case |
| `s` `S` / `C` `D` `Y` | Change a character, a line; change, delete, yank to the line's end |
| `d` `c` `y` + motion | Delete, change, yank over it; doubled, over whole lines |
| `d` `c` `y` + `i`/`a` + object | The same over a word, paragraph, sentence, quoted or bracketed run |
| `>>` / `<<` / `>` `<` + motion | Indent, outdent whole lines |
| `p` / `P` | Put back what was taken, after or before the cursor |
| `"{a-z}` / `"+` | Use that register for the next yank, delete, or put |
| `J` | Join the next line onto this one |
| `C-a` / `C-x` | Add one to the number at the cursor, subtract one |
| `.` | Repeat the last change |
| `u` / `C-r` | Undo, redo |
| `C-s` | Write the file |
| `Z Z` / `Z Q` | Write and close; close, discarding edits |
| `C-o` | Hand the file to `$EDITOR` in a new tab, at the cursor's line |
| `]b` / `[b` | Next, previous tab of the pane |
| `C-M-[` / `C-M-]` | Back through the tabs you last worked in, and forward again |
| `gb` | The buffer list (`C-x C-b`) |
| `C-/` | Comment out the line or selected lines; uncomment when they all are |
| `M-/` (typing) | Complete the word from nearby words, then other buffers; again for the next |

A text editor holds one file, its buffer.

- Opening another file (from the file browser, `gx` on a path, or a jump to a definition, reference, or problem in another file) shows that file's buffer: the pane's tab on it, a buried one brought back, or a new tab.
- Opening a file that is already open goes to it.
- `]b` and `[b` step through the pane's tabs, `C-M-[` and `C-M-]` walk them by how recently you worked in them, and `gb` opens the buffer list.
- The jump history belongs to the window, so going back after a jump into another file returns to the file and place you left.
- A rename or fix a language server makes across files edits each file in its own buffer. A file that was not open comes in as a buried, unsaved buffer for `C-x C-b` to find.
- Closing the tab buries the buffer; `C-S-k` kills it, asking first about unsaved edits.
### Commit messages

In a Winter pane, `git commit` edits its message in the built-in text editor instead of `$EDITOR`. Winter sets `GIT_EDITOR` to `winter --edit-wait`, which opens `COMMIT_EDITMSG` in a new text editor tab, in Insert mode, and holds git until that tab closes. The staged diff opens as a second tab in the same pane, one tab away, and closes with the text editor.

- `C-c C-c` or `C-S-c` (or `ZZ`) saves and commits.
- `C-c C-k` or `C-S-k` (or `ZQ`, or discarding when asked) empties the message, so git stops with "Aborting commit due to empty commit message".
- In this tab, these keys win over the window's own `C-S-c` (copy) and `C-S-k` (close the buffer). The header lists the two chords.
- `C-M-p` replaces the message above git's `#` comment lines with an earlier one from the repository's history (the last ten distinct messages), going further back each press. `C-M-n` comes forward again to what you typed.
- With no running window to ask, `winter --edit-wait` runs `$VISUAL` / `$EDITOR` instead, so git always has an editor.

An interactive rebase's plan (`git-rebase-todo`) opens the same way, as editable text, with extra keys for the plan:

- In Normal mode, `p`, `r`, `e`, `s`, `f`, or `d` alone (or after `C-c`, in any mode) sets the line's action to pick, reword, edit, squash, fixup, or drop and moves to the next line. In this file only, these replace the Vim keys of those letters.
- `C-c j` and `C-c k` move the line down and up.
- `C-c C-c` or `C-S-c` (or `ZZ`) finishes.
- `C-c C-k` or `C-S-k` empties the plan, so git aborts the rebase.
- A plan git would refuse (one whose first commit squashes or fixes up into nothing) is not finished; Winter says what is wrong with it.

**Acting on what is at the cursor.** `M-a` (`Editor: Act on What Is at the Cursor…`) lists everything that can be done with the thing the cursor is on, whatever kind of thing it is, like Emacs's Embark. Choosing one does it.

- A web address: open it in the browser, or copy it.
- A file's path, with a line where it gives one (`src/main.rs:12`), found beside the file or under its project: open it here or beside, open it with its own app, or copy it.
- A commit hash, in a git repository: show the commit, or copy it.
- A name: with a language server, go to its definition, peek at it, find or peek at where it is used, rename it, or describe it; change every occurrence of it at once; copy it.

**The command line.** `:` in Normal mode, or over a selection, opens a Vim command line for the commands that act on many lines at once. Whatever a command changes, one `u` takes back.

- `:g/pattern/command` runs the command on every line matching the pattern; `:v/pattern/command` (or `:g!`) on every line that does not. `:g/TODO/normal A!` puts `!` at the end of every line holding `TODO`; `:v/\S/d` deletes the blank lines.
- `:normal keys` types the keys on each line of the range, in Normal mode, as if typed there: `:'<,'>normal I# ` comments the selected lines. `<esc>`, `<cr>`, `<tab>`, `<bs>`, and `<lt>` spell those keys.
- `:d` deletes the range's lines; `:s/pattern/replacement/` replaces the first match on each line, `g` after it every match, `i` in any case.
- `:w` saves.
- A range is `%` (the whole file), `'<,'>` (the selection, which `:` from Visual mode starts with), a line number or two (`3,8`), `.` (the cursor's line), or `$` (the last line). Without one, a command acts on the cursor's line, and `:g` on the whole file.
- Patterns are regular expressions in Rust's syntax (`\d`, `(a|b)`, `$1` in a replacement), not Vim's.

**Expanding the selection.** `S-M-<right>` (`Edit: Expand Selection`) grows the selection to the next piece of code holding it, starting from the character under the cursor: the word, a quoted string's text and then its quotes, what brackets hold and then the brackets, the line, the paragraph, the file. With a language server serving the file, its pieces are used instead: an expression, an argument, a statement, a block, a function. `S-M-<left>` gives the last growth back. The selection is an ordinary Visual one, so `d`, `c`, `y`, and the rest act on it.

**Undone branches.** `u` and `C-r` work as Vim's do. Typing after an undo doesn't throw away what was undone: it is kept as a branch of the undo tree. `C-M-/` (`Edit: Restore an Undone Branch…`) lists those branches, newest first, each with how many changes it holds, the line it starts on, and when it was left. Choosing one brings that text back, and what it replaced becomes a branch in turn, so nothing is lost either way. A file keeps up to 50 branches.

Every tab of the window shares the clipboard, the named registers, the kill ring, the last search, and the last change `.` repeats. The last char search stays with the editor. Each file keeps its own marks, undo history, cursor and scroll position, and its last known contents on disk.

A count typed before a command applies to it: `3x`, `2dd`, `5j`, `2d3w`. Insert mode uses Emacs-style keys:

- Moving: `C-a`, `C-e`, `C-b`, `C-f`, `C-n`, `C-p`; `M-b` and `M-f` by word; `M-m` to the first non-blank; `C-v` and `M-v` by page; `M-<` and `M->` to the start and end; `C-l` centres the cursor.
- Deleting: `C-d`, `C-h`; `M-d` and `M-DEL` (or `C-DEL`) by word; `C-k` to the end of the line (at the end, the line break); `C-u` to the start; `C-w` back a word.
- Kill ring: deleted text goes on a kill ring. Consecutive kills join into one entry, so `C-k C-k` takes a whole line and `C-y` brings it back whole. `M-y` right after a `C-y` swaps the pasted text for the previous kill. Deletes and yanks made in Normal mode also go on the ring, so `C-y` reaches them too.
- Editing: `C-t` and `M-t` transpose characters and words; `M-u` and `M-c` upper-case and capitalise the word ahead; `C-o` opens a line without moving the cursor.

A word here is a run of letters and digits. The window keeps its own `M-` chords (`M-h` `j` `k` `l`, the digits, `M-x`), so there is no `M-l` for lower-casing.

Every input field takes the same keys: a dialog's answer (an editor's question, the sudo password, the Z.ai key, a theme name), the picker's query, a tab's new name, a settings text row, and the find box for a `/` search. A field holds the keyboard while it is up, so it adds keys a file leaves to the window:

- `C-g` cancels, like `Esc`.
- `C-/` and `C-_` undo (a word at a time while typing); `C-\` redoes.
- `M-l` lower-cases the word ahead.
- `M-\` deletes the spaces around the caret.
- `C-S-v` pastes into whichever field is up.

A picker keeps its own list keys (see Pickers): `C-n` and `C-p` move the selection, and `M-p` and `M-n` recall earlier queries. In the file browser, `Left` and `Right` go up and into directories, so `C-b` and `C-f` move the caret there.

Selecting with `v` or `V` makes operators act on the selection instead of a motion. `d`, `c`, `y`, `>`, `<`, `J`, `~`, `r`, and `p` act on the selection and end it. `i` and `a` grow it to a text object, and `o` moves to its other end so you can grow it from either side. `Esc` drops it.

The mouse moves the cursor too: a click puts it on the character under the pointer, dragging selects from where the button went down, and the wheel scrolls, carrying the cursor with it.

There is one clipboard, the system's.

- With no register named, `y`, `d`, `c`, and `x` put what they took on it (a yank says so, a delete is silent), and `p` and `P` paste whatever is on it, copied in Winter or anywhere else. `"+` and `"*` name the same clipboard.
- Every copy is also kept in the clipboard ring, which a paste falls back on where the system has no clipboard.
- `"_` is the black hole: it takes nothing, so `"_dd` deletes a line without replacing what `p` would paste.
- `"a` through `"z` name window registers, kept off the clipboard and shared with the terminals' Normal mode, so `"ayy` in a file and `"ap` at a prompt move a line between them. `"A` through `"Z` append to one.

Search is literal text, not a pattern. A search typed in lower case matches either case; one with a capital matches exactly. `*` and `#` search for the word under the cursor.

The editor colors syntax with TextMate grammars, over 200 of them: every common programming language, LaTeX and BibTeX, Markdown (code in a fenced block in its own language), diffs, commit messages and rebase plans, and configuration formats (JSON, YAML, TOML, KDL, INI, `.env`, Nix, Terraform, Protocol Buffers, Dockerfile, Makefile, CMake, and the git dotfiles).

- The grammar is chosen by the file's name (`Makefile`, `.bashrc`, `COMMIT_EDITMSG`), else its extension, else its first line, so a script with a shebang and no extension is colored too.
- The dark preset gives each kind of thing its own hue: functions, types, macros, attributes, constants and members, and in LaTeX each section level, environment names, math, labels, citations, and escapes. The light preset keeps the same hues, darkened to read on the light background.
- Recolor any of them by TextMate scope in the `colors` block (`syntax { entity.name.function "#bf73df" }`; `comment`, `string`, `keyword`, `number`, and `type` still work). A name in the block that matches no scope is reported when the settings are read.
- `Developer: Inspect Tokens and Scopes` (`F1 M-f`) shows which scopes the character under the cursor is in and which of them color it.
- A theme that sets no syntax colors colors comments, strings, numbers, keywords, and types from its own palette.
- In Rust and Lean, whose grammars leave most names without a scope, a name is colored by its context: in Rust a type, an enum's variant, a call, a path's module, a field, a constant, and a variable where it is bound; in Lean a namespace, a tactic, a type, a constructor, an operator, and a name where a binder binds it. A variable is colored where it is bound and left plain where it is used.
- In a commit message, the conventional type (`feat:`, `fix(ui)!:`) is colored as a keyword and the summary is marked past 72 characters. Git's `#` comment lines, in a commit message and in a rebase plan, are colored as comments throughout, including the branch, changed files, and headings in them.
- A file in an unknown language is shown as plain text.
- A long file is colored from the top down over a few frames, without blocking the window.
- A line over 20,000 characters (a minified script, a data dump) is left plain.
- Coloring reflects only the characters; it never reports errors in the code.

If the file changes on disk while open with nothing unsaved, it is reloaded the next time the text editor comes to the front. With unsaved edits, your work is kept and saving asks before overwriting.

**Merge conflicts.** In a file holding the conflict blocks a merge, rebase, or cherry-pick leaves (`<<<<<<<`, `=======`, `>>>>>>>`, and `|||||||` with `merge.conflictStyle diff3`), each marker is drawn on a band, each side keeps its own colors, and the status bar counts the conflicts. `M-n` and `M-p` step between them, wrapping round the file. With the cursor in a conflict:

- `C-c o` keeps ours (the checked-out side).
- `C-c t` keeps theirs.
- `C-c b` keeps both, ours first.
- `C-c r` keeps both, theirs first.
- `C-c a` keeps the ancestor's version.
- `C-c d` keeps neither.
- `C-c m` (or `Merge: Resolve Conflict…`, also on the right-click menu) lists every choice, including keeping one side in every conflict of the file at once.

Each resolution is one change that one `u` undoes. The status bar shows how many are left, then says to save and stage.

**Comparing.** Each of these opens in the Diff editor below, where the buffer's side is written back to its file:

- `Diff: Compare Buffer with File on Disk` shows what the unsaved edits change.
- `Diff: Compare Buffer with Clipboard` shows what pasting the clipboard over the buffer would change.
- `Diff: Compare Buffer with Another Open Buffer…` compares two open buffers, edits included.
- `Diff: Compare Buffer with File…`, `…with Revision…` (`HEAD` asked first), and `…with Two Files…` cover the rest.

**Writing Markdown and LaTeX.** A `.md` file and a `.tex` file have their own chords; each is also in the picker as `Markdown: …` or `LaTeX: …`. A chord that wraps text wraps the word under the cursor, or a selection within one line, and removes the wrapping if it is already there.

| Markdown | Action |
|---|---|
| `C-c C-n` / `C-c C-p` | Next, previous heading |
| `C-c C-f` / `C-c C-b` | Next, previous heading at the same level |
| `C-c C-u` | Up to the parent heading |
| `C-c -` / `C-c =` | Promote, demote the heading or list item (or every line selected) |
| `C-c [` / `C-c ]` | Promote, demote the heading and everything under it |
| `C-c b` / `C-c i` / ``C-c ` `` / `C-c $` / `C-c ~` | Toggle bold, italic, inline code, inline math, strikethrough |
| `C-c x` | Toggle the task box, giving a plain item one |
| `C-c n` | Number the ordered list again from its first item |
| `M-RET` | A new list item (the number one on, an empty box after a task) or a heading of the same level below, typing into it |
| `C-c w` | Count the words, of the selection or the file, outside code and front matter |

| LaTeX | Action |
|---|---|
| `C-c C-n` / `C-c C-p` | Next, previous sectioning command |
| `C-c C-e` | Start an environment, asking its name |
| `C-c e` | Rename the enclosing environment at both ends |
| `C-c d` | Turn the display the cursor is in from `\[…\]` into an `equation`, or back |
| `C-c b` / `C-c i` | Toggle `\textbf`, `\textit` |
| `C-c C-f` then `b` `i` `e` `t` `f` `c` | Toggle `\textbf`, `\textit`, `\emph`, `\texttt`, `\textsf`, `\textsc` (with or without `C-`) |
| `C-c w` | Count the words with `texcount`: text, headings, captions, following `\input` and `\include` |

**Outline.** `C-S-o` (`Navigate: Outline of This File…`) lists the file's headings in Markdown, its sectioning commands in LaTeX, and in a program every line its grammar marks as a definition (a function, a type, an `impl`, a class, a module, a TOML table), each indented by depth. It is a live picker like Scout: it starts on the entry the cursor is under, the cursor follows the entry you pick, and the cursor goes back if you dismiss the list. Like coloring, it uses only the grammar and never analyzes the code.

**Colors and rich text.** A color written as `#rrggbb` or `#rrggbbaa` is drawn on that color, in black or white text to stay readable; `Editor: Toggle Color Swatches` turns this off. `Editor: Copy as Rich Text (Syntax Colors)` copies the selected lines, or the whole file, to the clipboard as HTML in the theme's colors, with a plain text version alongside.

**Changes made elsewhere.** The editor checks the file it shows once a second. When another program has written to it (another editor, a formatter, a `git checkout`) and you have no edits, the file is reloaded with the cursor where it was. With unsaved edits, they are kept, the status bar says once that the file changed, and saving asks before overwriting. A file on a remote host is reloaded on `C-r` instead.

**Wrapped lines.** Long lines wrap at the pane's edge, at the last word that fits. `M-z` turns wrapping off and on for the tab.

- `editor-wrap-indent` (Wrapped line indent on the settings page) sets how continuation rows are indented: `"semantic"` (the default) starts them under the line's text, past what opens it (a Markdown list item's bullet and task box, a heading's `#`s, a quote's `>`s, a LaTeX `\item`, or a comment's marker), else under the line's indentation; `"same"` under its indentation alone; `"none"` at the gutter.
- An indent that would leave a row less than twelve columns of text is dropped.
- Over a wrapped line, motions go by screen rows (like Vim's `gj` and Emacs' `visual-line-mode`), keeping the column a run of them started at. In Normal and Visual mode this covers `j`, `k`, the arrows, `H`, `M`, `L`, `C-d`, `C-u`, `C-f`, and `C-b`; `g0`, `g^`, `gm`, and `g$` work on the screen row; `gj` and `gk` go by whole lines instead.
- While typing, `C-n`, `C-p`, the arrows, `C-v`, and `M-v` go by screen rows, and `C-a`, `C-e`, `Home`, and `End` go to the screen row's ends.
- `0`, `^`, `$`, and an operator's motion (`dj`, `d$`) still take whole lines.

**Sticky scroll.** When the top of the view is inside something that began above it, that thing's first line stays pinned over the pane's top rows, dim: in code, the functions, `impl`s, and classes whose bodies run on past the top (up to three, outermost first); in Markdown and LaTeX, the headings the text is filed under. Clicking a pinned line goes to it. A pinned line never covers the cursor's row. `Editor: Toggle Sticky Scroll` turns it off or on.

**Scroll margin.** The view keeps `scroll-margin` screen rows (5 by default; Scroll margin on the settings page) above and below the cursor (like Vim's `scrolloff`), in the text editor and in the Dir, Git, Grep, and other listings. A move into the margin scrolls the view a row instead of the cursor reaching the edge.

- `zt` and `zb` leave that many rows above or below the cursor's row, and `H` and `L` stop that far in, except at the very top and end, where there is nothing more to scroll to.
- The margin is held under half the pane.
- The wheel moves the view without the cursor and ignores the margin.
- Over wrapped lines it counts screen rows.

**Hidden markup markers.** The text editor hides markup markers and draws what they mark. The markers stay in the file and the cursor walks over them, but they take no room on screen, so the caret, clicks, and wrapping follow what shows. It hides:

- Markdown: the `**` of `**bold**` and the `_` of `_italic_` (text drawn bold or italic, nested ones both), the `~~` of `~~struck~~` (struck through), `<u>`, `<ins>`, `<del>`, and `<s>` tags (underlined or struck), the backticks of `` `code` ``, the `$` of `$math$`, a heading's `#`s, and a link's `[` and `](url)`, leaving its text underlined.
- LaTeX: `\textbf{` and its `}`, and likewise `\textit`, `\emph`, `\textsl`, `\texttt`, `\textsf`, `\textsc`, `\underline` and `\uline` (underlined), `\sout` (struck), and `{\bf ..}`, `{\it ..}`, `{\em ..}`.
- Org: `*bold*`, `/italic/`, `_underlined_`, `+struck+`, `=verbatim=`, and `~code~`.
- reStructuredText: `**bold**`, `*italic*`, and ` ``code`` `.
- AsciiDoc and Typst: `*bold*`, `_italic_`, and `` `code` ``.
- The Markdown in a `///`, `//!`, or `/** .. */` doc comment of a language commented with `//`.

Code blocks, math blocks, and LaTeX comments are left as written. `editor-markup-markers` (Markup markers on the settings page) sets when markers hide: `"always-hidden"` (the default); `"show-on-cursor"`, which shows a span's markers while the cursor or selection touches it, and a heading's on the cursor's line; or `"visible"`. `Editor: Toggle Hide Markup Markers` turns hiding off and back on, and `Editor: Toggle Always Hide Markup Markers` switches between always and on-cursor.

**Spelling.** The editor checks a file's prose against an American English dictionary plus the words you have added. It checks a LaTeX file's text but not its commands or math, a BibTeX file's titles and notes, a Markdown file's text but not its code, a plain text file whole, and a program's comments only.

- LaTeX, BibTeX, and Markdown are checked from the moment they open; `M-S-i` starts or stops checking any other file.
- A misspelled word is underlined like a language server's remark, beneath any server warning or error, and rechecked after every edit.
- In Normal mode, `]s` and `[s` go to the next and previous misspelling, wrapping past the file's ends, and suggest a spelling.
- `z=` offers suggestions for the word under the cursor; the one you choose replaces it, keeping its capital. `M-\` (or `C-.`) does the same on a misspelled word, in any file, with or without a language server, listing its fixes ahead of the server's.
- `zg` accepts the word as correct, in every editor from then on. Added words are kept one per line in `spelling/words.txt` beside `settings.kdl`.

**Completion.** While typing, `C-SPC` opens a popup of possible completions at the cursor. It also opens by itself once a word is three letters long or right after a `\`, `/`, `{`, or `#`. `completion { auto #false }` limits it to `C-SPC`, and `min-chars` sets the length.

- `C-n` and `C-p` (or `Down` and `Up`) move through it, and `Tab` takes the entry.
- `RET` also takes it once you asked for the popup or moved through it; otherwise `RET` starts a new line.
- `Esc` closes it and you keep typing.
- Each entry is marked with a letter for its kind and named with the back-end that offered it.

The back-ends are asked in order: the first with anything to offer decides where the completed text starts, and later ones answering at the same place are merged in below it.

- `words`: words in the open buffers, nearest the cursor first.
- `paths`: the files and directories a path being typed can continue to.
- `snippets`: each language's templates (`fn`, `test`, `fig`, `tab`, `code`), inserted with their fields' defaults and the cursor on the first field, plus your own, one file each in `snippets/<language>/` beside `settings.kdl`, named by the word that expands it.
- `keywords`: the language's reserved words.
- `latex`: commands after `\`, environments after `\begin{` (the one left open first after `\end{`), the project's `\label`s in a `\ref{`, and its `.bib` keys with their titles in a `\cite{`.
- `markdown`: the files a link's `](` can point at, and the document's headings by their anchors after `#`.

Set which back-ends a language asks, and in what order, with `completion { backends { rust "snippets" "keywords" "words" } }`. `M-/` still completes in place from the open buffers, one word per press.

**Language servers.** A language server reads the project and answers the editor's queries. None starts by itself. `LSP: Start the Language Server for This File` (picker) starts the server for the file in front, rooted at its project: the outermost `Cargo.toml` inside the repository for Rust, the nearest `go.mod`, `pyproject.toml`, `package.json`, and so on, else the repository, else the file's directory. Every open file of that project is sent to the server as it changes.

- Winter knows rust-analyzer, pyright (or basedpyright, pylsp, ruff), gopls, clangd, typescript-language-server, texlab, marksman, bash-language-server, ocamllsp, taplo, yaml-language-server, vscode-json-language-server, lua-language-server, haskell-language-server, zls, and nil, and uses the first installed one for a language.
- `lsp { servers { python "pylsp" } }` picks another server.
- `lsp { auto-start "rust" }` starts a language's server as soon as one of its files opens.
- `LSP: Stop`, `LSP: Restart`, and `LSP: Show Running Language Servers` do what they say.
- The status bar shows what a server is busy with (indexing, for example).

While a server serves the file:

- `M-.` (or `F12`, or `gd` in Normal mode) goes to the definition; `M-<left>` / `M-<right>` (or `S-M-b` / `S-M-f`) go back and forward over jumps.
- `C-c C-/` (or `gr`) lists the references.
- `M-F12` (`LSP: Peek Definition`) shows the definition in a card under the cursor, a few lines of it with its line marked, without leaving the file; `S-M-F12` (`LSP: Peek References`) does the same for every place the name is used, `M-n` and `M-p` stepping through them. `RET` goes to the place shown; any other key puts the card away.
- `C-c C-[`, `C-c C-]`, and `C-c C-\` go to the type's definition, the implementation, and the declaration.
- `M-,` or `M-0` (or `K`) opens a card describing the name under the cursor; resting the pointer on a name does too.
- `C-S-SPC` shows the signature of the call being typed.
- `C-c C-r` (or `F2`) renames the name everywhere, opening the changed files as unsaved buffers.
- `M-\` (or `C-.`) offers fixes and refactorings at the cursor in one list, after the spelling fixes for a misspelled word, and the code lenses of the cursor's line.
- `C-S-M-i` formats the file.

What the server says of the code that the text does not shows dim in the line, once typing pauses:

- Inlay hints: a binding's type (`let x: i32 = 1`), a call's parameter names, where the server gives them. The cursor steps over them as over text that is not there. `Editor: Toggle Inlay Hints` hides them in that file, or shows them again.
- Code lenses, at the end of the line they are about: `Run | Debug | 3 references` over a Rust test, say. `M-\` on the line offers them; choosing one runs it as the server says. rust-analyzer's Run and Debug run as a task in a Compile tab, and a references lens lists what it counts.
- Both keep showing while you type elsewhere, and come back for the line you type on once the server answers again.

Problems the server finds get a wavy underline in the hue of their severity, with the line number in the same hue, and the status bar counts errors and warnings.

In a file git tracks, a column to the left of the line numbers shows what the file has changed since its last commit, as you type: a green `+` beside each added line, a `~` beside each changed one, and a red `-` where lines were deleted (on the line below them, or on the last line). The committed text is read again every few seconds, so a commit or a checkout made elsewhere shows. The column is there in every file, blank in one git does not track or one on another machine, so the text starts in the same place in all of them.

- `S-M-n` and `S-M-p` (or `]d` and `[d`) step through the file's problems, with a card describing each.
- `M-e` goes to the project's first error.
- `M-g M-n` and `M-g M-p` step through the whole project's problems.
- `C-M-p` lists them all.

Completion asks the server too, first among the back-ends (`lsp`), as you type and after its trigger characters (`.` and `::` in Rust). An entry that needs an import adds it.

**GitHub Copilot.** Copilot comes from an extension Winter ships, installed beside it and loaded like any extension (so a build without the `wasm-extensions` feature has none). While `copilot { enable }` is on (the default), it has Winter start GitHub's `copilot-language-server` the first time a file opens, using the Copilot sign-in stored on this machine. Its settings are on the settings page under Copilot, and in the `copilot { … }` block of `settings.kdl`.

- `Copilot: Sign In` signs in through GitHub's device flow: the code is copied to the clipboard and the page opens in the browser. `Copilot: Sign Out` and `Copilot: Show Status` do what they say.
- The server is the one `copilot { server-path }` names, else the first found on the `PATH`, else the one `Copilot: Install Language Server` fetched into `extension-servers/copilot` in Winter's state directory: GitHub's own build for this machine, from its npm package `@github/copilot-language-server`.
- As you type, Copilot's suggestion shows dim at the cursor (ghost text), with lines past the first in a card below. The status bar says `Copilot`, with `2/3` when it offered several.
- `Tab` (or `C-RET`) accepts all of it; `M-f`, `M-<right>`, or `C-,` accepts its next word; `C-.` its next line.
- `M-n` / `M-p` show the next or previous suggestion, and `C-g` hides it.
- Typing what it shows keeps it showing, and `C-\` requests one when none came.
- After a change, Copilot may also suggest a next edit elsewhere (`copilot { next-edit }`). It shows in a card: `Tab` applies it and `Esc` dismisses it.
- When the completion popup is open, it takes `Tab` first.

**Sending code to a terminal.** `C-S-RET` (`Editor: Send Selection or Line to Terminal`) types the selection into a terminal beside the editor and runs it, for a REPL or a shell there. With nothing selected it sends the cursor's line and moves the cursor to the next line that holds anything, so pressing it again walks through a script. The keyboard stays in the editor. It goes to the terminal it last sent to while that one is shown, else the first terminal shown, else the dock's, else a new terminal split beside, in the file's directory. Lines go as a paste, so a REPL that takes pastes (Python 3.13, IPython, any shell) takes a block of them whole.

For anything else, `C-o` hands the file to `$EDITOR`, saving first so `$EDITOR` opens what you were looking at. The editor refuses to open a file it cannot handle as text, and says so: anything holding a NUL byte, anything not UTF-8, and anything over four megabytes. A UTF-8 byte order mark at the start of a file is hidden and written back on save.

Saving writes to a temporary file beside the original and renames it over the top, so an interrupted write leaves the previous contents intact. Undoing back to the last saved state leaves nothing to save, so closing does not ask about edits that are gone. Saving keeps a file's permission bits, line endings, and final newline (or its absence), so saving one edited line does not produce a whole-file diff. If something else has written to the file since it was opened, saving asks before overwriting.
**Diff** compares texts region by region in a tab of its own: two side by side, three against the middle one, or two merged into a third. It opens from:

- the text editor's `Diff:` commands;
- Dir's `=` on two or three marked files;
- Git's `=` on a file (the last commit against the working tree);
- `Merge: Merge Another File into This One…`;
- `Merge: Resolve Conflicts in a 3-Way Merge`, which sets a conflicted file's two sides (and the ancestor `diff3` keeps) against each other and writes the merge back over the file.

Each region is tinted in its text's hue (A red, B green, the third blue). Filler stands where a text has no line. The gutter marks the current region, and its changed words are drawn louder. Keys:

- `n` or `SPC` and `p` or `DEL` step through the regions; `J` jumps to one by number.
- `j`, `k`, `C-d`, `C-u`, `v`, `V`, `gg`, and `G` scroll every text together; `<` and `>` scroll sideways.
- In a comparison, `a` copies the region from A to B and `b` from B to A (`ab`, `ac`, `ba`, `bc`, `ca`, `cb` with three texts). `ra`, `rb`, `rc` put a region back as it was.
- In a merge, `a` and `b` keep one side, `+` keeps both between markers, `o` the ancestor's, and `r` what the merge first held. `$$` steps only through the conflicts left.
- `##` leaves whitespace out of the comparison and `#c` leaves out case. `!` works the regions out again.
- `@` turns the changed words off, `|` turns the columns into bands stacked one above another, `~` swaps A and B, and `/` also shows a merge's ancestor.
- `ea`, `eb`, or `ec` edits a text in place with the editor's Vim keys, the regions following, until `C-c C-c` or `C-S-c`.
- `wa`, `wb`, `wc` (or `C-s` while editing) write a text back to its file.
- `D` shows the differences as a unified diff, `i` says where the session stands, and `q` quits, asking first about a text not written.

**Markdown preview.** `F7` (or `S-F6`, `F6`, or `Markdown: Preview Beside`) shows the Markdown file in front rendered in the pane beside it, as a web page in the theme's colors. The document is written out as HTML, laid out by the Blitz engine, and drawn on the GPU.

- Headings, emphasis, links, lists, tasks with boxes, quotes, tables, and images beside the document render as a README does. Nothing is fetched from the network, so a remote image or badge is left out.
- Code blocks are colored by their language with the editor's own highlighting.
- Math, inline between `$` and on its own line between `$$`, is typeset in the text's color.
- A fenced `mermaid` block is drawn as a diagram in the theme's colors (shown as code where it does not parse). The page runs no scripts.
- It follows the file's text as you type, unsaved, and keeps the block holding the cursor's line in view; `f` stops following the cursor, or follows it again. With no editor on the file, it rereads the file from disk whenever it changes.
- The wheel, `j`/`k`, `C-d`/`C-u`, `C-f`/`C-b`, `gg`, and `G` scroll it.
- Clicking a link opens a web address in the browser and a file beside the document in Winter; a `#heading` link scrolls to that heading.
- Dragging elsewhere selects text and copies it as the selection grows; `ESC` clears it.
- `RET` or `o` goes to the source line of the block at the top of the pane; a double-click goes to the line of the block under the pointer.
- `/` finds text in the source and scrolls to it (`n` and `N` step through the matches).

**Occurrences and the shape of the code.**

- The word under the cursor is outlined wherever else it shows, with a thin border around each occurrence over its own colors. `Editor: Toggle Occurrence Highlights` turns it off.
- `M-n` and `M-p` step between its occurrences.
- `C-;` changes them all at once: what you type goes into every occurrence together, drawn on a band. `Tab` and `S-Tab` step between them, and `Esc` or `RET` ends it. With a selection on one line, the selected text's occurrences are changed.
- `C-c C-a` and `C-c C-e` go to the start and the end of the function or module the cursor is in, as its grammar names it.
- `C-M-i` selects that function or module by lines; pressed again, it selects the one around it.
- `zj` and `zk` step between the blocks the indentation makes.

**Typing by context.**

- In Insert mode, `RET` carries a line comment (`//`, `///`, `#`, `--`) on to the next line, and a block comment's ` * `.
- In a Markdown list, `RET` continues under the item's text; on an empty item, it moves its bullet down a line.
- On a Markdown heading, `Tab` and `S-Tab` add and remove a `#`.
- On a bullet, they indent and outdent the item, the bullet turning `-`, `+`, `*` by depth.
- `S-DEL` just past a bullet blanks it out, so the text continues as a paragraph of the item above.

**Writing help.**

- `M-s i` inserts a math or Greek symbol picked from a list: the symbol itself, or its command in a LaTeX file.
- `M-s e`, `M-s ]`, and `M-s [` go to the first, next, and previous misspelled word in the whole project's prose: its Markdown, LaTeX, BibTeX, and plain text files, with open buffers read as they stand.
- `S-M-t` looks up the word under the cursor (or the selection) in your GoldenDict dictionary and shows its entry in a card. The dictionary is the largest `.dsl.dz` under `~/Setup/GoldenDict`, `~/Workspace/data/dictionaries`, or `~/.local/share/goldendict/content`. The first look-up reads the headwords on a separate thread; later look-ups are instant.
- In a `.bib` file, `C-S-M-s` sorts the entries by year, and `BibTeX: Sort Entries by …` sorts them by citation key or year, ascending or descending.

**Clipboard ring.** Winter keeps every copy made in it, newest first, plus whatever is on the clipboard when you open the ring. `Clipboard: Paste from Clipboard Ring…` lists them and pastes the one you choose wherever typing goes. In the editor, `M-y` does the same when the previous key was not a yank; after `C-y` it steps back through what was yanked, as before. `Clipboard: Clear Clipboard Ring` empties it.

**Small things.**

- `C-l` puts the cursor's line in the middle of the pane, then at its top, then at its bottom.
- The line flashes after a jump to a definition or an offset. `Editor: Flash Current Line` flashes it on demand.
- `M-g M-c` goes to a character offset from the file's start, and `M-g c` shows the cursor's offset.
- `C-M-'` opens the scratch file beside the settings, for notes not tied to a project.
- `C-S-M-o` in a Rust file runs `cargo fix` over the project in its Compile tab.

**File browser** (`C-S-f`, or `File: Browse Files`) walks the working directory in the command picker, not in a pane, to open a file quickly.

It opens where the terminal points, when that is an existing file or directory. It looks, in order, at the Normal-mode cursor, the mouse selection, the shell's cursor (on a path, or just past one you typed), and the rightmost path on the last line printed. A file opens the browser on its directory with that file selected; a directory opens the browser inside it. Paths resolve against the shell's working directory, and `~`, `..`, and `path:line:col` suffixes are understood. With nothing found, or over an editor, it opens on the working directory.

| Key | Action |
|---|---|
| Typing | Filter this directory's entries |
| `/` after a name | Go into that directory and keep typing: `src/`, `../`, `~/`, or a filter like `sr/` that selects `src/`. `/` with nothing typed goes to the root |
| `Up` `Down` or `C-p` `C-n` | Move through the list |
| `Enter` on a directory | Open it in Dir, beside the focused pane; on `./`, the directory being browsed; on `../`, go up and keep browsing |
| `Tab` / `Right` on a directory | Go into it; the browser stays up, rooted there |
| `Enter` on a file | Open it in the text editor, over the focused pane |
| `C-h`, or `Left` | Go up to the parent directory (`Backspace` only deletes typed characters) |
| `C-S-f` again, or `Esc` | Close it |

Directories come first, then files, with hidden entries after each group and `./` and `../` at the top. The path being browsed is shown above the list, and each row's icon shows whether it is a directory or what kind of file it is. It reads one directory at a time, so it opens instantly however large the tree is.

**Files as root, in containers, and over SSH.** The same browser and editor reach files you cannot access as yourself. A remote file is named by a URI, and the URI works everywhere a local path does: the browser, the text editor, the recent list, and a saved session.

| URI | Reaches |
|---|---|
| `sudo:///etc/hosts` | `/etc/hosts` as root, through `sudo` |
| `docker://web/etc/nginx.conf` | Inside the running container `web` |
| `docker://www-data@web/var/www` | The same, as user `www-data` |
| `ssh://me@example.com:2222/srv` | On an SSH host, as `[user@]host[:port]` |
| `ssh://[::1]/etc` | An IPv6 host, in brackets |
| `docker://` or `ssh://` | The containers, or the `~/.ssh/config` hosts |

`sudo:/etc/hosts`, `admin:///etc/hosts`, and `sftp://` are also accepted, as the same places. A space or other character a URI cannot hold is written as a `%` escape (`%20`); non-ASCII letters stay as they are. The host list follows the SSH config's `Include`s.

Three commands open the browser on them: `File: Browse Files as Root` (the working directory as root), `File: Browse Docker Containers`, and `File: Browse SSH Hosts`. Choosing a container or a host goes into its root. From there the browser works as it does locally:

- An absolute path typed with `/` stays on the target, so `/var/log/` goes to that directory in the container.
- `../` at a target's root goes back to the list of targets.
- A URI typed in any browser goes where it says once you type its last `/`: `ssh://myhost/`, `docker://web/`, or `docker:///` for the list of containers.

The text editor shows where a buffer lives in the status bar, as `[sudo]`, `[docker://web]`, or `[ssh://me@host:2222]`, and reads and saves it the same way it was reached.

- A save copies the new contents over the file instead of replacing it, so the file keeps its owner, mode, and any link to it. A transfer cut short leaves the file as it was.
- A local file only root can read opens as root without asking.
- Saving a file you can read but not write asks whether to save it as root.

Where `sudo` wants a password, Winter asks for it in a dialog that shows a dot for each character typed. The password goes to `sudo` once and is not kept. `sudo`'s own timestamp spares the next read or save the question for as long as it would in a shell, and with `NOPASSWD` nothing is asked. Until a root buffer has been read, it stays empty and every key tries to read it again, so nothing can be saved over the file.

SSH runs without prompting (`BatchMode`), so a host needs key authentication (an agent is fine) and an accepted host key. Each command has a deadline, so a host that stops answering is reported instead of waited on.

Docker and SSH work from Linux, macOS, and Windows alike. The scripts run in the container or on the host, which needs `sh` and `cat` (the listing uses `stat` where available, else `ls`). On Linux and macOS, one SSH connection per host is shared for ten minutes; Windows' OpenSSH cannot share one, so each step there connects anew. Root (`sudo:`) needs `sudo`, so it works on Linux, macOS, and the BSDs; on Windows, Winter says it is unavailable.

A restored session with remote buffers reads each one when it is first shown, not at startup. Listing and reading block while you wait, so a slow host is felt as a pause. A distroless image, with no `sh`, cannot be browsed. Dir, Grep, Git, `gx`, `$EDITOR` (`C-o`), and the PDF viewer stay on the local disk.
**PDF** opens wherever a file does, on any path ending in `.pdf`: `Enter` in Dir, Git, or Grep, the file browser, or a `gx` reference. It has no chord of its own and opens as a tab beside the editor you reached it from, so closing it puts the listing back.

- Winter draws the document itself, with a built-in Rust PDF renderer and the fonts a PDF may name without carrying, so it needs nothing installed and no network access.
- Pages are drawn on their own thread, only those in view plus one either side, so a long document opens at once and scrolling never waits. A page not drawn yet shows as a blank sheet.
- The same thread reads every page's text when idle, so searches and motions seldom wait.
- The picker, menus, key hints, and notifications sit over a document as over anything else.
- The viewer reads only the file it was opened on. An encrypted PDF opens if it needs no password.

| Key | Action |
|---|---|
| `{count}` before a motion | Repeat it that many times: `3j`, `12k` |
| `j` `k` or `Down` `Up` | Scroll down, up |
| `h` `l` or `Left` `Right` | Scroll left, right, for a page zoomed past the pane |
| `C-e` / `C-y` | A line down, up, without moving what is on screen otherwise |
| `C-d` / `C-u` | Half a screen down, up |
| `C-f` / `C-b`, `Space` / `b`, `PageDown` / `PageUp` | A screen down, up |
| `gg` / `G`, or `Home` / `End` | First page, last page |
| `{count}gg` / `{count}G` | Go to that page |
| `]` / `[` | Next page, previous page |
| `zt` / `zz` / `zb` | Put this page at the top, middle, bottom of the pane |
| `C-g` | Show the file, the page of how many, and how far through |
| `g C-g` | Count the words and characters selected, or with nothing selected, the whole document's words |
| `+` / `-` | Zoom in, out |
| `=` | Fit the page to the pane's width again |
| `P` / `T` | Fit the whole page in the pane; fit the text's width, margins off the sides |
| `C` | Crop every page to its text box (measured over the first ten pages), or show them whole again |
| `E` | Center the pages across the pane |
| `V` | Show page thumbnails down the pane's left, the current one framed; click one to go there |
| `:` | Ask which page to go to |
| `O` | Open or close the outline |
| `I` | Invert the pages' colors (light text on dark), or put them back |
| `f` | Label the links in view; type a label to follow its link (a URL opens in the browser) |
| `F` | Label the links in view; type a label to preview where its link leads, under it, then `Enter` to follow |
| A click on a link | Follow it |
| Resting the pointer on a link | After a moment, show where it leads under it (a URL shows in the status bar); moving off closes it |
| `M-S-o` | Outline every link in view (a frame for an internal link, an underline for a URL), or only the one under the pointer again |
| `C-o` / `C-i`, `M-<left>` / `M-<right>`, `M-S-b` / `M-S-f`, the mouse's back / forward buttons | Back to where the last jump left (a link, an outline entry, `gg`, `G`, `Home`, `End`, `:`), forward again; `C-` plus a mouse side button shows the pane's previous / next tab instead |
| `Enter` / `K` with a cursor | Follow the link under the cursor, show where it leads |
| `S`, or a double-click | Go back to the LaTeX source of the cursor's spot (SyncTeX), or of the line being read with no cursor |
| `s` | Label the start of every line in view; type a label to go to that line's LaTeX source |
| `M-S-s` | Tint each sentence its own color for proofreading, or remove the tints |
| `M-S-i` | Underline misspelled words, or stop |
| `C-M-n` / `C-M-p` | Next, previous misspelled word, wrapping past the ends: marked, with suggested spellings |
| `zg` | Accept the word last stepped to as correct, from now on and in every editor |
| `i` | Put a cursor in the text, for the motions below |
| `/{text}` / `?{text}` | Search down, up the document; the cursor lands on the match |
| `n` / `N` | Repeat the search, same direction or opposite |
| `Esc` | Abandon a half-typed command |

A rebuilt PDF reloads on its own. The viewer checks the file twice a second and reloads once a change has stayed the same between two checks, so it never reads a half-written build. The old document stays on screen while the new one loads, then the new one shows at the same spot of the same page, or at the SyncTeX mark being shown. Old pages stay up until redrawn, so nothing goes blank. The cursor goes away (its line may have moved), but the last search stays for `n`. If the file will not open, the last good document stays, Winter retries a few times, then reports the failure.

Links are the document's own: a `\ref`, a `\cite`, a contents line, or a URL, as hyperref writes them. A preview is cut from the target's page at the current zoom, a paragraph or two from the spot, framed under the link (over it near the pane's bottom). A publisher's copy often keeps a `\cite`'s link but drops its destination table. Such a link is read for its own number (the `12` of `[12]`) and sent to the reference-list line that starts with `[12]`. The list is found as the longest run counting up from `[1]` at line starts, searched back from the last page. A link whose number has no such entry leads nowhere and is left out.

SyncTeX works both ways for a document built with `-synctex=1` (latexmk and most editor builds do this), using the `synctex` program that TeX Live and MiKTeX ship.

- Backward, from the PDF: `S`, a double-click, or the menu's SyncTeX Backward (offered when the PDF has its `.synctex.gz`) asks `synctex edit` which line of which file set the spot. Winter then narrows that line down using the text around the spot (SyncTeX alone often points at the paragraph's first line), and searches the files an `\input` or `\include` nearby brings in when the text is not where SyncTeX said. The file opens in the text editor at that line.
- Forward, from a `.tex` file: `F6` (the picker's "LaTeX: Show in PDF") finds the document's root: the file a `% !TEX root = …` comment names, or the one declaring `\documentclass` in its directory or up to three above. If one directory has several, it picks the one whose last build read the file (by its `.fls`), else the one with the newest PDF.
- The PDF is looked for where latexmk was told to write it (`$out_dir`, else `$aux_dir`, in the project's `.latexmkrc` or `latexmkrc`), else beside the root.
- It shows in the tab already showing it, else in a new tab of this pane. `S-F6` or `F7` ("LaTeX: Show in PDF in the Other Pane") opens it in the other pane.
- The box the line was set in scrolls to the reading line and stays marked until the next key or click.
- If SyncTeX does not find the line, or the PDF has no SyncTeX data, the PDF still opens and says so.
- From a `.bib` file, the line searched is the first `\cite` of the entry under the cursor in the document's `.tex` files. Outside any entry, the PDF just opens.

A toolbar takes the pane's top row, drawn in the window's font like the status bar, and the pages scroll under it.

- Left: the section being read, from the document's outline (bookmarks). Click it to drop the outline down under the bar as a card over the pages, each section's page at its right. It is folded to the top level except the path to the current section. Move with `j` and `k`, unfold with `l`, fold with `h` (or `Tab`, or a click on a row's chevron), and jump with `Enter` or a click, which closes it. `Esc`, `q`, `O`, or a click on the pages closes it without moving. A document with no sections says so there instead.
- Right: the zoom, `−` and `+` either side of the percentage. Click the percentage to enter a number, or `w`, `h`, or `p` to fit the page's width, its height, or the whole page.
- Then `‹` and `›` either side of the page number; click the number to choose a page.
- Then `◐`, which inverts the pages' colors. The pages are redrawn, so those in view change as soon as they are drawn.

The motions are vim's and compose the same way: a count typed before a motion multiplies it and is used up by it, `g` and `z` wait for the key that completes them, and the pane's key-hint card shows what finishes a pending one. The header shows a count as you type it, so an accidental one is visible before it moves anything. `gg` and `G` read a count as a position, not a repeat: `5G` is page five, not five pages on.

Some motions differ because a document has no cursor by default: `h` and `l` scroll sideways, and `C-d` moves half the pane. `zt`, `zz`, and `zb` move the window, not the page being read, as in vim.

`i` puts a cursor in the document's text, and the keys become text motions. `Esc` takes it back out one step at a time: a half-typed command first, then the selection, then the cursor itself.

The mouse selects text too:

- A drag selects what it passes over and copies it to the clipboard as it goes; the status bar shows how many characters it holds.
- A click puts no cursor in the text. Only `i` does, starting where a drag left off; once the cursor is in, a click moves it.
- With the cursor in, the mouse selection is the same one `v` and `V` make, so `o` swaps its ends and motions extend it from where the mouse left it; `y` copies it again after that.
- Every copy from a PDF is announced by its length, not echoed.

| Key | Action |
|---|---|
| `h` `j` `k` `l` or the arrows | Move the cursor left, down, up, right |
| `w` / `b` / `e` | Forward a word, back a word, to a word's end |
| `0` / `^` / `$` | Start of the line, its first non-blank, its end |
| `{` / `}` | Back, forward a paragraph |
| `(` / `)` | Back, forward a sentence on this line |
| `f{char}` / `F{char}` | To the next, previous occurrence of a character on this line |
| `t{char}` / `T{char}` | To just before, just after it |
| `;` / `,` | Repeat the last `f`/`F`/`t`/`T`, same direction or opposite |
| `gg` / `G` | The first, last character of the document, across pages |
| `{count}gg` / `{count}G` | The start of that page |
| `H` / `M` / `L` | The line nearest the top, middle, bottom of the pane, without scrolling |
| `zt` / `zz` / `zb` | Scroll so the cursor's line lands at the top, middle, bottom |
| `v` / `V` | Select by character, by line; the same key again drops the selection |
| `o` | Put the cursor on the other end of the selection |
| `y` (with a selection) | Copy it |
| `y{motion}` | Copy exactly what the motion spans: `yw`, `ye`, `y$`, `y}`, `yfx` |
| `yy` / `Y` | Copy whole lines from the cursor's |
| `/{text}` / `?{text}`, `n` / `N` | Search, and walk the matches |
| `y/{text}` | Copy from the cursor up to the search match |
| `{count}` before any of them | Repeat it that many times |

Motions are inclusive or exclusive as in vim. For example, `yw` and `ye` over the same word land in the same place, but `ye` takes the last letter and `yw` stops before the next word starts. The window keys still work with a cursor in the text, so `Space`, `C-d`, `]` and the rest still page through the document.

A PDF has no text structure, so Winter models it:

- The page is cut into blocks along empty bands that run right across it.
- A vertical band is a two-column gutter, so motions read down one column, then the other. A page number under the gutter does not close it, and a review copy's margin line numbers join the column beside them.
- A horizontal band separates a banner or footnote from the body, so a title reads before the columns under it.
- Within a block, glyph runs are grouped into lines by how close their baselines are, a gap wide enough for a space becomes one, and a paragraph break is a line gap noticeably wider than that page's usual.
- Where a layout defeats the model, the motions follow the model, not the page.

Search works the same in either mode: `/` and `?` ask for text in the find box at the pane's top-right, and `n` and `N` walk the matches, wrapping around the document once. A lower-case pattern matches either case; a capital makes it exact, as in the text editor. A search with no cursor in the text puts one on the match, with the whole match underlined. It searches as you type, each letter from where the prompt opened, and the status bar counts matches in the whole document (`[2/18]`). `Esc` at the prompt restores the cursor and the last search.

**Image** opens wherever a file does, on a PNG, JPEG, GIF, WebP, BMP, ICO, or SVG file, and draws it fitted to the pane, never past its own size. Winter draws it on the GPU, so the picker, menus, key hints, and notifications sit over it.

- `+` and `-` zoom about the middle of the pane, `=` fits the image again, and `1` shows it at its own size.
- `h`/`j`/`k`/`l`, `SPC`, `b`, `C-d`, `C-u`, and the wheel scroll an image zoomed past the pane, stopping where its edge meets the pane's.
- The status bar shows the image's size and scale (`640×480  fit 42%`); `C-g` says the same.
- A GIF shows its first frame.
- An SVG is drawn fitted up to the pane and redrawn at each zoom, so it stays sharp. Open With opens its markup in the text editor.

**Hex** shows a file's bytes, sixteen to a row, as an offset, the bytes, and their characters. It opens on a file the editor refuses as binary, and from Open With on any file. `h` and `l` step a byte, the Vim keys move by rows, and `g o` or `M-g M-c` goes to an offset (decimal or `0x` hex). `C-x C-q` turns editing on: a hex digit writes the half of the byte under the cursor, `u` undoes the last byte written, and `C-s` or `w` writes the file.

**Open With** (`C-M-S-e`, `Open: This File With…`) opens the focused tab's file in another editor that can show it, as a tab beside it: for example, an SVG in the image viewer opened as markup, or one being edited drawn as an image. The list holds only editors that can show that file; if none can, it says so. If a tab of the chosen editor in the same pane already has the file, it goes to that tab, and a buried buffer holding the file comes back. The viewers read only the local disk, so a remote file is offered to the text editor only.

**Keys** (`Help: Keyboard Shortcuts`) lists every command and every editor's own keys, sorted by name, with Vim's motions for reference. Each entry shows its chords, whether each binding is built in or yours, and the command's id. It reads the keymap in force, so it always matches what the keys do.

- It opens in Normal mode over the list: `/`, `n`, and `N` search it, and `Enter` runs the command under the cursor.
- `i` is the only key into Insert mode, in the filter line at the top. Typing narrows the list to commands whose name, id, or chord match. `C-u` clears it. `M-k` records keys instead, so pressing `M-g` lists every command bound to a sequence starting with it.
- In Insert mode, `Enter` runs the command the cursor is on, and `Esc` returns to Normal mode, keeping the filter; nothing else leaves Insert mode.
- Either way, the Keys view hands the pane back before running a command, so a command that opens its own editor has somewhere to open.
- `c` assigns new keys to the command: press them (a sequence like `M-g o` works too), then `Enter` to save, `BS` to remove the last key, or `Esc` to cancel. The line warns when another command already uses them.
- The binding is saved to the `commands` block of `keybindings.kdl`, replacing what that file bound the command to; every other line of the file stays as you wrote it.
- `x` removes your binding, so the built-in one is back in force.
- A chord bound to two commands in one block is drawn in the warning color, since only one of them runs.
- Chords the window intercepts before any tab (such as the palette's) never reach the capture line, so you cannot assign them here.

**Describe key** (`Help: Describe Key`, `F1 k`) works the other way round, like Emacs's `C-h k`: run it, press a key, and a notice says what that key does where you pressed it, without doing it.

- `C-,` says `C-, runs Settings`.
- In an editor, one of its own keys is named with what it does there (`RET` in the Keys view says `runs Keys: Run the Command`).
- In a terminal, a plain letter says it is sent to the program, in the terminal's current mode.
- A key that starts a sequence (`M-g`) or a Vim prefix (`g` in Normal mode) waits for the key that ends it, so `M-g M-h` says `runs Move Tab to Left Pane`.
- It looks the key up through the same bindings in the same order as the keyboard, so it always matches what the key does.
- Its key is `F1 k`, beside `F1 h` for the keys of the focused tab.
## Workspaces

A workspace is a project you can switch the window to and back from. When you return, you find it as you left it: its panes, its tabs, and each terminal's directory. Winter keeps one file per workspace in `~/.local/state/winter-term/workspaces/` (`%LOCALAPPDATA%\winter-term\workspaces` on Windows) and never writes into the project's folder. Every command is in the picker:

- `Workspace: Mark Folder as Workspace` makes the focused tab's project a workspace, offering the folder's name for it. The window is in it from then on. If the project is already in a workspace, it says which.
- `Workspace: Switch` lists the workspaces, with the window's own marked `(current)`; `Enter` goes to one.
- `Workspace: Open Recent` lists the other workspaces, the one the window was in most lately first, however it got there (this picker, `Workspace: Switch`, a shell's `winter workspace switch`, a restored session); `Enter` goes to one. It has no default key; bind `workspace_recent` in `keybindings.kdl`.
- `Workspace: Add Folder` adds the focused tab's project to the window's workspace as another folder.
- `Workspace: Rename` renames the window's workspace.
- `Workspace: Remove` lists the workspaces so you can forget one. Its folders are left as they are, and its tabs close if it was open.

Switching puts the current workspace aside instead of closing it:

- Its tabs keep running out of sight, so a build or an agent in one of its terminals carries on, and switching back finds everything as it was.
- The workspace you switch to comes back as it was put aside; else as last saved; else, if never opened, as one terminal at its first folder.
- A layout in no workspace cannot be kept, so leaving it closes it. Winter asks first, as quitting does, if that would end a program running in a terminal or lose unsaved edits.
- A shell that exits in a workspace put aside closes its tab there. If nothing is left, that workspace opens as last saved next time.

Quitting saves every workspace open in the window, and asks first about programs running in any of them. A reload keeps the programs of the workspace in front running; those of workspaces put aside end, and Winter asks first. A restart that restores the session returns to the workspace the window was in. A window opened in a given directory (a file manager's "Open Terminal Here", or a new window) starts fresh there, in no workspace.

The same commands work from a shell as `winter workspace`. They act on the window the shell runs in (Winter's panes name it in `WINTER_SOCKET`; elsewhere it is the first window launched) and print what the window says:

| Command | What it does |
|---|---|
| `winter workspace add [dir] [--name N]` | Make the project of `dir` (default: current directory) a workspace, named `N` or its folder's name |
| `winter workspace add-folder [dir]` | Add the project of `dir` to the window's workspace |
| `winter workspace switch <name>` | Switch the window to a workspace |
| `winter workspace rename <name>` | Rename the window's workspace |
| `winter workspace remove <name>` | Forget a workspace |
| `winter workspace list` | List workspaces and their folders, the window's marked `*` |

A workspace has its own settings, layered over `settings.kdl`. In a window with a workspace, the Settings page's title shows two scopes, `User` and `Workspace: <name>`; `TAB` switches between them.

- An edit in the Workspace scope applies while that workspace is the window's, and is kept in its file, never in `settings.kdl`. Rows it sets are marked with a dot; `S-Del` on one drops it and restores the user's value.
- An edit in the User scope goes to `settings.kdl`. Where the workspace sets the same row, the workspace value stays in force.
- Switching workspaces replaces the last one's settings with the next one's.

## Extensions

An extension adds commands, status bar segments, completion back-ends, pickers, and its own tabs to Winter. An extension's picker is one of Winter's own: it matches, previews, and takes the picker keys as every other does, and its rows may offer several actions in the action menu (`F4`). Most come as a single WebAssembly file. Its commands appear in the picker under its name and can be bound to keys like any other. If an extension fails to load, a notice says why when the window opens.

Each extension runs in its own sandbox. It can read and write only its own directory, `~/.local/state/winter-term/extension-data/<id>/`, and whatever you grant it. An extension says what it asks for, but asking grants nothing: you approve it when you install it, and Winter runs only the very file you approved.

To install one, run `winter extension install <file.wasm>`, or **Extensions: Install From File…** in the Command Picker (`M-x`). Winter reads what the file asks to reach without running any of it, shows you, and installs it only once you approve: it copies the file alone to `~/.config/winter-term/extensions/<id>/<id>.wasm` (`%APPDATA%\winter-term\extensions` on Windows) and keeps your approval, with the file's hash, in `~/.local/state/winter-term/extension-approvals.kdl`. Restart Winter for it to run. A file replaced or rebuilt since you approved it, or one copied into `extensions` by hand, is not run, and the notice names the command that reviews it. Installing a newer build that asks for nothing new keeps your grants and asks nothing; one that asks for more asks about the new part alone.

| Command | What it does |
|---|---|
| `winter extension install <file.wasm>` | Show what it asks to reach, and install it once you approve |
| `winter extension list` | List installed extensions, what each may reach, and whether it runs |
| `winter extension permissions <id>` | Show what one may reach: what it asked, what you added, what you refused |
| `winter extension grant <id> read\|write\|run <what>` | Let it read a directory (`read`), read and write one (`write`), or have Winter run a program as its server (`run`) |
| `winter extension revoke <id> read\|write\|run <what>` | Take a grant back |
| `winter extension remove <id>` | Uninstall it and forget its approval |

**Extensions: Manage Extensions** opens the Extensions page: each extension installed as a file, whether it runs, and what it may reach, a row per grant, marked as asked, added by you, or asked and refused. `d` takes a grant back, `RET` on a refused one grants it, the rows under each extension grant it a directory, asked for as a path, and `D` removes the extension. Its first row installs one from its component. Every change takes effect when Winter next starts. A directory is written from `/` or `~`; a program by its file name alone. An extension never starts a program itself: `run` lets it have Winter run one as its server (Copilot's `run "copilot-language-server"`).

Winter also loads the extensions it ships, from the `extensions` directory installed with it (`/usr/lib/winter/extensions` on Linux, beside `winter.exe` on Windows, in the app bundle's resources on macOS). They are part of Winter, so they reach what they ask without an approval. One you install with the same id, and approve, takes a shipped one's place: that is how you run a newer Copilot before Winter ships it.

An extension's settings go in a block of `settings.kdl` named for its id (`copilot { enable #false }`) and show on the settings page under its name. A block no installed extension claims is reported and left alone, so removing an extension never refuses your settings file. A command's id is exactly what the extension registers and its package lists, with no prefix added, so it need not start with the extension's id.

An extension has no network access and at most 256 MiB of memory. Winter stops it if a call runs longer than a second while you type, if slow background work runs longer than a minute, or if it crashes. That extension then stops, a notice says so, and Winter carries on. While an extension's slow work runs, its commands report it is busy and its tabs show what they last drew.

To write one, start from `crates/ext/winter-simple-ext`, a working template. The same Rust crate can be compiled into Winter or built as a `.wasm` file (`cargo rustc --release --target wasm32-wasip2 --crate-type cdylib --features wasm`). Its `package.kdl`, at the crate's root, says what it is, what it asks to reach, and what its commands are called; `winter_ext_guest::export!` embeds it in the `.wasm` file as it is built, so the file is all there is to install:

```kdl
package id="<id>" name="<Name>" version="0.1.0" {
    description "What it does, shown when it is installed"

    // What it asks to reach; the user approves it on install
    permissions {
        read "~/Notes" "~/Documents"
        write "~/Notes/Data"
    }

    // What its commands are called in the Command Picker
    commands {
        "<id>.word_count" title="Count Words in File"
        "<id>.flag_todos" title="Flag TODOs as Problems"
    }
}
```

Keys for its commands are the user's to choose, in `keybindings.kdl`, as for every other command. The [architecture notes](architecture.md#extensions) describe how both kinds run.

## Configuration

Config lives in `~/.config/winter-term/` (`%APPDATA%\winter-term` on Windows), in two files:

- `settings.kdl` for appearance and behaviour
- `keybindings.kdl` for the `window` and `editing` chords, each editor's own block (`git`), and every editor's own keys by id (`commands "dir" { "toggle_archive" "M-z" }`)

Both use [KDL](https://kdl.dev/). Winter installs a reference copy of each. The shipped copies (`crates/app/winter-term/builtin/settings.kdl` and `crates/base/winter-model/builtin/keybindings.kdl`) are the real defaults, read before yours: anything you leave out keeps its default, and anything you set replaces it.

Changes apply on save, without a restart. `winter --reload` also asks a running instance to reload.

### Updating Winter without losing your session

After installing a new binary, run `winter --reload` (or the `reload` command in the picker) in a running window. On Unix (tested on Linux) the window replaces itself with the new binary in place:

- Every program running in a terminal pane (Claude Code, an editor, a build, a remote shell) keeps running with its process id and its pty.
- The layout and focus come back.
- Each pane's screen and scrollback are rebuilt from a copy taken at the moment of the reload.
- Each program then gets a resize, so full-screen programs repaint themselves.

What does not come back: editor tabs other than Dir, Git, Grep, and the text editor; block history and folds; and anything a program wrote in the instant between the copy and the replacement. If the new binary cannot start, the window stays as it is and says so.

The running window must already have this feature. A window started from an older build reloads by starting fresh shells, so restart it once. On Windows, reload keeps the older behavior, since its pseudo-terminals cannot be handed to another process image.

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
| `gutter-side` | `"left"` (default) or `"right"`: which edge holds an editor's gutter, the columns of line numbers, git states, and marks (an editor with nothing to show beside its rows has none), and of every terminal its column of command statuses while `terminal-gutter` shows it; a gutter's icons take one column each |
| `terminal-gutter` | Keep a column beside every terminal's rows for each command's exit status: exited zero, failed, running (default off). Only a shell with Winter's integration reports them. `Terminal: Toggle Gutter` turns it on and off |
| `window-controls-side` | Which side the window buttons sit on |
| `pane-border-width` | Divider thickness between panes |
| `popup-input-height` | Height of the input field in every popup (every picker, the find box, a question), in font rows: `1.5` by default, `1` to `4` |
| `dim-inactive` | Dim panes that are not focused |
| `url-underline` | Underline detected URLs and OSC 8 links |
| `palette-match-underline` | Underline matched characters in the picker |
| `paste-on-right-click` | Right click pastes instead of opening a menu |
| `restore-session` | Reopen the previous layout on launch (off by default), with its editor tabs (Dir, Git, Grep, text, PDF, image, hex, launcher) where they were looking |
| `scroll-margin` | Screen rows an editor keeps above and below the cursor when scrolling, like Vim's `scrolloff` (5 by default, 0 for none, held under half the pane) |
| `scrollback-lines` | Scrollback ceiling |
| `shell` | Shell to spawn, instead of the system default |
| `shell-linux` / `shell-macos` / `shell-windows` | Per-platform override of `shell` |
| `window-title-template` | Window title: `"{{ name }}"` by default (`main.rs`), without the app's name; also `{{ location }}`, `{{ path }}`, `{{ title }}`, `{{ app_name }}`, `{{ pane_title }}`, and `{{ cwd }}`; an empty part drops out with its separator |
| `rainbow-parens` | Color brackets by nesting depth |
| `editor-indent-guides` | Draw a faint line at each level of a line's indentation in the text editor (default on); a blank line inside a block takes the block's guides |
| `sentence-highlight` | Alternating bands over sentences, as a reading aid |
| `wrap-indent` | Indent the continuation of a soft-wrapped line under its start. Applied to the scrollback when a width change settles; the visible screen always wraps at the margin, where programs expect it |
| `wrap-words` | Break soft wraps at word boundaries instead of mid-word. Applied to the scrollback when a width change settles, like `wrap-indent` |
| `prompt-edit-bindings` | `"emacs"` (default) or `"none"`, see above |
| `cursor { … }` | `blink`, `hide-in-inactive`, plus the shape per mode: `insert`, `normal`, `visual` |
| `status-bar { … }` | `show` (default on); `show-mode` (default on); `segments`: the right side's segments in order, from `antigravity`, `claude`, `zai`, `tool` (the focused editor's), default all in that order, and a bare `segments` shows none; `priority`: the order segments hold on when the bar is short of room, first kept longest, unlisted ones dropped first, default none (drops from the left) |
| `clipboard-read` | Let programs read the clipboard through OSC 52 |

You can also edit every setting above on the Settings page (`C-,`), which writes the same file. It opens as a tab, so it can sit beside a terminal while you try a change. `j` and `k` move between settings, `h` and `l` (or `SPC`) change one, and `i` edits a text setting, in Insert until `Esc`. Every change applies at once. The exceptions are a colour table and the keybindings, which need the file or their own page.

Themes are separate KDL files under `themes/`. Each has an optional `base "dark"|"light"` plus a `colors` block layered over it. A `colors` block written directly in `settings.kdl` also works, as does a `keybindings` block (the format of the legacy single-file `winter.kdl`).

## Shell integration

Blocks, exit codes, folding, and the working directory Winter shows all come from OSC 133 and OSC 7 marks your shell must emit. Add one line to your rc file:

```bash
# bash
source /usr/share/winter-term/shell-integration/winter.bash

# zsh
source /usr/share/winter-term/shell-integration/winter.zsh

# fish
source /usr/share/winter-term/shell-integration/winter.fish
```

Without it, Winter still works, but the whole session is one rolling block: no per-command boundaries, no exit-code tags, no folding. The marks also let Winter know:

- When a command owns the pane. A REPL or an agent CLI running on the primary screen gets `Esc` from Normal mode, and this stops as soon as a fresh prompt appears.
- Where the editable command line begins. A Vim operator aimed at the shell's own `❯ ` decoration is refused ("that is the shell's own prompt, not the command line") instead of turning into readline chords that edit the wrong characters.

## Rich blocks

Programs can emit typed, MIME-tagged content that Winter draws inline: images, SVG, charts, Markdown, HTML, tables, and JSON. Every block carries a `text/plain` fallback, so the same program stays readable under `tmux`, `ssh`, or in CI.

Winter draws every block itself, on the GPU, from the richest representation in it that it can draw:

| Representation | Drawn as |
|---|---|
| `image/png`, `image/jpeg`, `image/gif`, `image/webp` | The image, scaled down to the band its rows reserve |
| `image/svg+xml` | The drawing, redrawn at its shown size so it stays sharp |
| `application/vnd.vega-lite+json` | A chart: one `bar`, `line`, `area`, or `point` mark over inline `data.values`, placed by `x` and `y`, split into stacked or separate series by `color`, with axes, a legend, and a title |
| `text/html` | Rich text: headings, emphasis, lists, quotes, links, code, and tables in aligned columns; scripts and styles are dropped, and nothing runs |
| `text/markdown` | Rich text, the same way |
| `text/csv`, `application/json` | A table, and pretty-printed JSON, in the terminal's font |
| `text/plain` | The text |

A chart Winter cannot draw (layers, facets, transforms, data fetched from a URL, or a full Vega spec) falls back to the block's next representation, and finally to its text. Nothing in a block is ever run or fetched, so a block from a downloaded file, a pipe, or a host behind `ssh` can only show its content. A block's requested trust tier is accepted and has no effect.

The protocol is documented in [`terminal-block-protocol-spec.md`](terminal-block-protocol-spec.md), and client libraries live under `clients/`.

## Multiplexer

`winter mux` manages headless PTY sessions that outlive the window:

```bash
winter mux serve            # run the session server
winter mux list             # list sessions
winter mux new <name>       # create a session
winter mux attach <name>    # attach to one
winter mux attach --host <host> <name>   # attach to one on a remote server, over ssh
winter mux kill <name>      # terminate one
winter mux proxy            # what a remote attach runs on the server
```

A session name may contain letters, digits, `-`, `.`, and `_`, and must not start with `-`. Sessions are reachable over a Unix socket locally, or over an SSH tunnel to a remote server. PTY size follows the smallest attached client, so a larger pane letterboxes instead of showing output wrapped for the wrong width.
