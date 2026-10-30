# Winter usage guide

How to drive Winter, a terminal-first IDE, day to day: the modes, the full keymap, and the settings that change them.

For what Winter is and how to install it, see the [README](../README.md). For how it is built, see [`architecture.md`](architecture.md).

## Modes

A window is split into panes, each a strip of tabs, and every tab is a terminal or an editor. A terminal tab is always in exactly one of three modes, and each keeps its own; an editor tab is in none, since it owns the keys it binds. The status bar is the window's, not the terminal's: while an editor has the keyboard it shows the editor's own mode where it has one (the text editor's Insert or Normal, a PDF's caret or selection, a listing whose names are being edited) and `P` where it has none, what the editor has to report, and at its right edge where the cursor is, such as `12:5  340 lines` in the text editor or `page 3 / 18` in a PDF. The status bar is shown by default (`status-bar { show #false }` hides it and frees its row), and the mode indicator is enabled by default (`status-bar { show-mode #false }` hides it).

The status bar's left edge says where the focused terminal's commands run: a desktop glyph for this machine, the remote glyph and the host's name while `ssh` (or a remote mux session) is in the foreground, and the Docker whale and the container's name while `docker exec`, `docker compose exec`, or `podman exec` is. Its right edge ends in a bell.

Before the right edge, the status bar shows the AI quotas there are to show, each under its logo as its short window and its weekly window, used and resetting at (`42% 14:05 | 10% Mon 09:00`, in local time): Antigravity's, Claude Code's, and Z.ai's. Each is asked for the way its own editors ask, so nothing needs setting up beyond them. Claude Code's comes from Anthropic's usage endpoint with the token Claude Code signed in with (the macOS keychain, else `.credentials.json` in `~/.claude` or `$CLAUDE_CONFIG_DIR`). Antigravity's comes from the language server a running Antigravity editor starts, else from the statusline file the `agy` CLI writes while it runs. Z.ai's needs an API key: `Quota: Set Z.ai API Key` in the command picker asks for it without showing it and keeps it in the system's secret store (the Secret Service on Linux, the Keychain on macOS, the Credential Manager on Windows), never in the config; answering blank forgets it. Claude Code's and Z.ai's are asked every five minutes, Antigravity's every minute, and `Quota: Refresh` asks them all at once. A provider that cannot be asked (not signed in, not running, no key) is left off, and one whose asking fails on the way keeps what it showed last. The requests go through `curl`, with the tokens on its standard input rather than its command line, and nothing is asked while the status bar is hidden.

What the app and its editors report goes by how much it matters. A confirmation (`pushed`, `wrote 120 lines`) is said in the status bar after its mode for three seconds. A warning or an error appears as a card in the window's top-right corner, newest on top and at most three at once: an icon and colored edge for its kind (yellow warning, red error), the text wrapped to a few lines, and the editor that said it. A warning stays six seconds, an error eight; a card the pointer rests on stays until the pointer leaves, its `×` dismisses it, and a click anywhere else on it opens the history. `notifications { toasts #false }` says warnings and errors in the status bar too, and while the bar is hidden every notice, a confirmation included, is a card, having nowhere else to go. The same thing said again in a row counts on one card, `(3x)`, rather than stacking copies. Every notice is kept for the session: an error or a warning not yet looked at dots the bell, counts on it, and colors it. Clicking the bell, `C-M-m`, or `Notifications: Show History` in the picker opens them in a Notifications tab beside the focused one (or goes to the one the pane has), counts them as read, and takes the cards down. Each notice is a card, newest first, grouped under Today, Yesterday, and Earlier: its icon (red error, yellow warning), who said it, how many times it came (`×3`), and how long ago on the top line, what was said under it, and a dot on those that had not been looked at. A message of more than a line is folded to its first; `Enter`, or a second click on the card, shows the whole of it, and again folds it. The toolbar filters the cards to the errors, the warnings, or the rest, each with how many there are, `Tab` and `S-Tab` stepping through them; `Clear all` at its right forgets them, as `C` does. The tab stays current as new notices come in; `y` copies the message under the cursor, and `/` finds one. `Notifications: Clear` forgets them all from anywhere.

| Mode | Who owns the keyboard | Enter it with |
|---|---|---|
| **Insert** | The PTY. This is an ordinary terminal. | `i`, `a`, or `o` from Normal |
| **Normal** | Winter. Motions, operators, layout commands. | `Esc` or `M-ESC` from Insert |
| **Visual** | Normal, but motions extend a selection. | `v` `V` `C-v` from Normal |

Insert is the default, so Winter behaves like a normal terminal until you ask it not to.

**Getting into Normal mode.** Press `Esc`, or `M-ESC` anywhere. The first `Esc` is Winter's, even with a program like Claude Code running; press it again to send it to the program (see below). Programs that run their own modes on `Esc` keep it: in vim, Neovim, Helix, Kakoune, Snow, and Emacs, or while a shell tab completion is open, `Esc` goes straight to the program and only `M-ESC` enters Normal mode.

**Getting out.** `M-ESC`, `i`, `a`, or `o` return to Insert. `Esc` in Normal stays in Normal, and it goes to Winter's own features first: a half-typed command or count, a search, a quick-select, a selection. Only when Winter has nothing of its own to put away does it reach the program underneath, so with a program running (or a tab completion pending) an `Esc` is sent to it. To send `Esc` to Claude Code, for example, press `Esc` twice.

Normal mode works against any shell, over `ssh`, and inside a Python REPL, because Winter owns the keyboard while it is active and nothing has to be installed on the far end.

## Keybinding reference

`normal`, `insert`, and `visual` use the built-in Vim-style bindings below and are not configurable yet. The `window` and `editing` chords further down are, in `keybindings.kdl`.

### Motions

Available in both Normal and Visual mode. Most take a count prefix, so `5j` moves down five lines and `3w` advances three words.

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

When the target character occurs more than once, Winter shows a labelled overlay so you can pick a landing spot with one keystroke instead of repeating `;`.

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

Text objects follow `i` (inner) or `a` (around) after an operator, so `diw` deletes a word and `ci"` changes the text inside quotes.

`d` and `c` reach only the line the shell is editing (see [prompt line editing](#prompt-line-editing)); `y` reads the whole buffer, so `yiw`, `yap` and `yy` copy out of the scrollback without going through Visual first. What a yank took stays lit for a moment, so you can see what landed in the register. `ap` takes the blank lines after the paragraph as well, which over command output is one block of it plus the gap to the next. A sentence is read within its own row, and only a stop followed by a blank or the row's end ends one, so a version number or a file name stays in one piece.

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
| `/` `?` | Search forward, search backward, typed in a find box at the pane's top-right |
| `n` `N` | Next match, previous match |
| `*` `#` | Search forward, backward for the word under the cursor |
| `Esc` | Put the search away, keeping the cursor on the match it found |

`n` and `N` still work after the find box is dismissed: the pattern and its direction survive, the way vim keeps them across `:nohlsearch`.

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

The marks named with punctuation are kept for you rather than set by hand, the way Vim keeps its own. Marks and the jumplist are per tab. Registers are shared across every tab, so a yank in one pastes in another.

### `g` commands

| Keys | Action |
|---|---|
| `gt` `gT` | Next tab, previous tab |
| `g<` `g>` | Move the current tab left, right |
| `gx` | Open what is under the cursor: a URL goes to the browser, a directory to Dir, a path to the text editor over this pane, at the line a `path:line` reference names. `M-.` does the same from any mode (see the file browser below for where it looks without a cursor) |
| `gy` | Yank the block under the cursor |
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
| `gy` | Yank the block under the cursor |
| `q` | Quick-select: label every block, press a label to act on it |
| `F8` / `S-<f8>` | Open the next, the previous file reference the last command or build printed |
| `C-S-b` | Choose one of the project's builds and run it in its Compile tab |
| `M-S-b` | Run the newest build again |
| `F6` | From the `.tex` or `.bib` file being edited, show where the cursor's line was set in its PDF (SyncTeX), in this pane |
| `S-F6`, `F7` | The same, the PDF in the other pane |

`F8` reads the newest command output that holds a `path:line` or `path:line:col` reference to a file that exists (a compiler's errors, a test runner's failures, a linter's findings), resolved against the directory the command ran in, and opens each in turn in the text editor at its line, the way Emacs' `next-error` walks a compilation. It keeps reading the same terminal while the text editor has the focus, and starts over when newer output arrives. A build run in a [Compile](#editors) tab takes over: from then on `F8` walks the places that build pointed at, read in each tool's own format, until `F8` is pressed in a terminal, which walks that terminal's output again.

### Prompt line editing

Normal-mode operators aimed at the line the shell is currently editing are translated into the readline keystrokes that produce the same result, so `dw` on the prompt really does delete a word in your shell.

| Keys | Action |
|---|---|
| `C-/` | Undo your prompt edits |
| `C-\` | Redo them |

This assumes your shell's default emacs-mode bindings. **If your shell is in vi mode** (`bindkey -v`, `set editing-mode vi`) set `prompt-edit-bindings "none"`, so Winter declines these operators and leaves the line to the shell, which gives you vim editing there anyway. Everything else, all navigation over the screen and scrollback, works the same either way.

### Window, pane, and tab chords

These work in any mode and are configurable in `keybindings.kdl`, which writes chords as Emacs does: `C-` is Ctrl, `M-` is Meta/Alt, `S-` is Shift, `s-` is Super (the logo key), always in that order (`C-M-S-s-n`), with `RET`, `TAB`, `SPC`, `ESC`, `DEL` and `<up>`-style names for the other keys. A letter is always lowercase, so Shift-s is `S-s`.

A binding may also be a sequence of up to four chords separated by spaces, such as `M-g M-h`. Sequences work the same in terminal and editor tabs, and several can share a first chord: after it, a hint lists what can follow. `ESC` drops a sequence, and a key that continues nothing drops it with a notice (`M-g x is undefined`). An editor's own block (`git { "M-g r" "open_remote" }`) binds commands that apply only while that editor has focus, ahead of the window's. Keys lists those commands too. When `keybindings.kdl` has a mistake (a chord that does not parse, a command or editor block that does not exist, or two bindings that overlap so only one can run), the status bar says so when the file is loaded.

| Chord | Action |
|---|---|
| `M-S--` / `M-S-\` | Split horizontally, vertically: the new pane opens a launcher (`RET` or `t` a terminal, `f` a file, `d` a directory, `g` git) in the directory split from; the commands `Split Horizontal: Terminal` and `Split Vertical: Terminal` split straight to a shell |
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
| `C-1` .. `C-9` | Go to tab N |
| `C-S-c` / `C-S-v` | Copy selection, paste |
| `C-,` | Open settings |
| `C-=` / `C--` / `C-0` | Font bigger, smaller, reset |
| `C-S-d` | Open Dir as a tab in the focused pane (toggle) |
| `C-S-g` | Open Git as a tab beside the work (toggle) |
| `C-S-k` | Kill the focused tab's buffer (closing a tab only buries it) |
| `C-x C-b` | Pick from every buffer: the terminals, then the buried buffers, then the shown ones |
| `C-M-S-e` | Open the focused tab's file in another editor that can show it |
| `M-x` | Command picker |
| `C-S-r` | History picker |
| `C-S-j` | The dock along the bottom of the window: open it with a shell, put it away when it has the keyboard, else focus it |
| `C-S-z` | Go back to a recent tab, buried buffer, file, or directory |
| `C-M-z` | Find a file in the project and open it (`C-z` still goes to the shell) |
| `M-S-z` | Find a file under the focused tab's working directory and open it |
| `C-M-g` | Search the project as you type, and open a line found |
| `C-S-<up>/Down` | Previous, next prompt block |
| `C-DEL` | Delete the word before the cursor |

A single-chord binding whose action is not one of the built-in window actions is looked up against the command picker instead, so `"M+q" "mux_new_session"` works.

## Editors

An editor is a tab Winter paints itself: Dir, Git, Grep, the text editor, a PDF, an image, Processes, System, and Keys. Each opens as a tab of its own in a pane's strip, a peer of the terminal tabs beside it, and the terminal it was opened from keeps running one tab away. Close an editor's tab with `C-S-q`. Pressing an editor's own chord while it is in front does nothing; from elsewhere in the pane it goes to the editor's tab. The window chords all still work from an editor: split, zoom, close, and `M-h/j/k/l` to move focus.

`C-M-z` (`File: Find in Project`) lists every file in the focused tab's project to fuzzy-find one by its path. A project is the nearest folder at or above the tab's directory holding a `.winter` folder or a git repository, else the directory itself; every project-wide command (this list, project search, Grep, Todo, and the build) works from the same one. Its files are the ones git lists, with what git ignores left out, else what `rg --files` lists. `Enter` opens the file in the editor that shows it, and `C-M-z` again puts the list away. The window holds the chord, so it never reaches the shell, where it would arrive as `ESC C-z` and suspend the job the way `C-z` does; `C-z` itself still goes to the shell.

`M-S-z` (`File: Find File`) is the same picker kept to the focused tab's working directory: the files under the directory the terminal is in, or the one an editor is looking at, rather than the whole repository around it. `M-S-z` again puts it away.

`C-M-g` (`Search: Project as You Type`) searches the same project for what you type: once the typing pauses on two characters or more, ripgrep runs and the lines it finds stream into the list, each beside its file and line, with the title counting how many are shown of how many found. The pattern is searched for as it is, except that a `*` stands for anything; case is ignored unless the pattern has a capital in it. A ` @ ` after the pattern starts a file filter: `foo @ src/*.rs` keeps the lines of the files matching the glob, and `foo @ recent` those whose path holds `recent`, and changing only the filter narrows what was found without searching again. `Enter` opens the file on the line, `M-p` and `M-n` bring back earlier queries, and `Esc` or `C-M-g` again stops the search and puts it away.

`C-S-z` (`Go To Recent: Tabs, Files, Directories...`) opens one list of everything to go back to: the tabs open in every pane, then the buried buffers, then the files opened lately, then the directories, newest first within each, each with its icon. `Enter` goes to a tab, brings a buried buffer back, opens a file in the editor that shows it, or lists a directory in Dir. A file a buried buffer holds is listed once, as that buffer. A file is remembered when an editor, the file browser, a click, or `$EDITOR` opens it, and a directory when Dir lists it or a shell has been in it; the files and directories carry over between sessions, and one that is gone is left out. The pane switcher that used to be on `C-S-z` is still `Go to Pane` in the command picker.

`C-S-j` (`View: Toggle the Dock (Bottom Pane)`) works as VS Code's panel does: the first press opens a pane across the whole bottom of the window, a third of its height, with a shell started where the focused tab was working. It is a pane like any other, so it takes tabs, splits, and editors, and it keeps its height when the panes above it are split or balanced. Pressed while the dock has the keyboard, it puts the dock away and gives the keyboard back to the pane that had it; its tabs keep running unseen. Pressed anywhere else, it brings the dock back, or focuses it if it is showing, and going to a tab that lives in a hidden dock brings it back too. Closing the dock's last tab removes it.

An editor's contents are a buffer, which the window keeps apart from the tab showing it. Closing an editor's tab, on its own or with its pane, buries the buffer rather than dropping it: a build goes on running, a listing keeps its place, a file keeps its edits and its undo, and a language server keeps the file open. Nothing is lost, so nothing is asked. `C-S-k` (`Kill Buffer`) is what lets a buffer go: it asks first when the buffer holds edits on no disk or a build or search is still running, then closes its tab. Quitting or reloading asks about edits in buried buffers as well as shown ones. A tab a program is waiting on, such as a `git commit` message, is killed when it closes rather than buried, so the program is not left waiting.

A terminal's tab is its shell, so it is never buried: closing it ends the shell, asking first when a program is running in it. A shell meant to outlive its tab runs in a mux session.

`C-x C-b` (`Switch to Buffer...`) shows every buffer as a list to pick from, in the fuzzy panel every other list in Winter is picked from: the terminal tabs in pane order, then the buried buffers, the most recently buried first, then the buffers a pane shows. Every row names the kind of editor, what it is showing, a `[+]` where it is holding edits that are on no disk, and the directory it is looking at, so typing `main` finds the text editor that was on `main.rs`. `Enter` goes to a tab a pane already shows, or brings a buried buffer back into the focused pane, as it was. `Esc` changes nothing.

`F1 h` (`Help: Keys of This Tab`) lists the keys of the tab in front, each beside what it does, in the same fuzzy panel: an editor's own keys for the state it is in (the Git status or the commit it is reading, a PDF with or without a cursor, Dir while renaming), then the motions every editor shares; over a terminal, every window command bound to a key. Typing filters on what the keys do, and `Esc` puts it away. Editors no longer spell their keys out in their header row: this list is where they are, and it is read from the same table each editor looks its keys up in, so it cannot name a key the editor does not answer.

Every editor asks the same way, in the middle of the window. A question with a known set of answers opens a picker: what the list is of on top, a filter line under it, and the matches below, narrowing as you type. `Up`/`Down` or `C-p`/`C-n` move through them, `Enter` takes the highlighted one, and `Esc` answers nothing. A question with no list to offer, or one whose answer is a name that does not exist yet, opens the same panel without the list: the question, the line it is answered on, and how to answer it. A question that cannot be undone is answered by a single key rather than a line, and says so instead of drawing a caret. Either way the question is where the eye already is, and the status bar is left to what it was saying.

Every editor answers to `M-S-,` and `M-S-.` for the top and the bottom of what it is showing, which are emacs' `M-<` and `M->` under the fingers, so an editor is read with either set of habits: the Vim `gg` and `G` reach the same two rows. The other Vim motions (`H` `M` `L`, `C-d` `C-u` `C-f` `C-b`) work in every editor too, and `C-r` re-reads or reruns whatever an editor is showing. The Git view still binds some of these keys to its own commands, after magit; it is the one exception.

Every editor whose cursor is a whole row (Git, Dir, Grep, Processes, System, Notifications) also has a column cursor along that row, kept by the window for all of them, so the Vim motions read the same there as in a terminal's Normal mode: `h` `l`, `0` `^` `$`, `w` `b` `e` and `W` `B` `E` (running onto the next or the previous row at either end), and `f` `F` `t` `T` with `;` `,`. The editor's rows stay its own: `j` `k`, `gg` `G`, the paging chords, and `H` `M` `L` move its cursor through what it lists, and `Home` and `End` reach its first and last rows. A click puts it on the cell clicked, as well as taking the editor's cursor to the row. `v` and `V` select from where the column cursor is. A key an editor binds for itself comes first: Dir's `h` and `l` still close and open a directory, Processes' fold its tree, and Git's `f`, `F`, and `t` still fetch, pull, and tag. Git's own commands that were on Vim's motion letters moved onto modifiers: re-reading to `C-r`, the branch menu to `C-M-b`, and the bisect, remote, and revert menus to `M-S-b`, `M-S-m`, and `M-S-v`.

Opening a file never opens it twice in one pane. `Enter` on a Git hunk, a Grep match, a Dir entry, a file reference, or a pick from the recent list, project files, or Open With first looks through the tabs of the pane it is in, the most recently shown first, for one that already has the file open: a text editor holding it in any of its buffers, or a viewer showing it. That tab is shown, the buffer brought forward, at the line asked for (a PDF at the page), and only a pane with no such tab gets a new one. A file is one buffer however many tabs show it, as in Emacs: a text editor tab in another pane opening a file already open elsewhere shows the same buffer rather than reading the file again. The text, its undo history, and whether it is saved are the buffer's, so an edit in one tab is in the other at once (its cursor kept on the line it was on), `u` in either undoes the last change made in both, and a save in one is a save in all; each tab keeps its own cursor, scroll, and selection. A language server hears of the file once, and that it is closed only when the last tab showing it closes. A buffer is a file's text; a terminal's screen and scrollback are that terminal's own.

Right-clicking inside an editor opens that editor's own menu, rather than the terminal's: over a listing there is nothing to paste into, and the entries that would mean something act on rows the terminal knows nothing about. The click moves the cursor to the row it landed on before the menu opens, so what is chosen acts on what was clicked. What the menu offers follows that row, because a commit cannot be staged and a section heading has no path to copy. Each entry is a key the editor already binds, or a few typed in order, shown by name with those keys at its right, so the menu is a way of finding a command and learning its keys rather than a second set of them. Entries come in groups set off by a line: reading or opening what was clicked, changing it, then copying what names it.

Every editor has one. Dir, Git, Grep, Processes, and System offer what the row under the pointer can have done to it. The text editor follows its mode: over a selection, Copy and Cut (through the clipboard register, so they reach other windows) and commenting; in Normal mode, Paste, Undo and Redo, commenting, Find, Save, and `$EDITOR`; in Insert mode only what types nothing, and a commit message or a rebase plan adds what finishes it. A right-click over a selection keeps it, so Copy has something to copy. The PDF viewer offers Copy over a selection (its right-click moves nothing), then Find, Go to Page, and the outline, the zoom, and inverting the colors; the image viewer offers its zoom and what the image is; Keys runs or copies the command under the pointer, finds one, or closes.

Every editor searches the same way, and so does a terminal in Normal mode. `/` (or `?`, back up the text) opens a small find box at the top-right corner of the pane being searched, where the text is typed with the same keys every field takes, while the pane stays lit so its matches show as you type; `Enter` keeps the search and `Esc` drops it. The status bar shows how far the search has got: the query and which match the cursor is on out of how many. `/` asks for text and moves the cursor to the next row holding it, `n` and `N` step through the rest of the matches and wrap at the ends, and a search that finds nothing says so instead of sitting still. Matching is literal, and a search typed in lower case matches either case while one typed with a capital in it matches exactly, the rule the text editor and the PDF viewer search by: a directory listing matches on the entry's name, every other editor on the whole row as it is painted, so a chord is as findable as the command beside it. A search is of the focused tab alone, never the whole window: other panes in view light nothing. Matches are lit the way a terminal's are, every one on screen in one color and the one the cursor is on in another, from the first letter typed, and they stay lit after `Enter` until `Esc` puts them away; `n` and `N` light them again. A PDF is the exception, since its pages are pictures rather than rows: it lights only the match the cursor lands on, though it counts them all. The status bar shows the query and, where the editor counts them, which match the cursor is on out of how many. A search moves the cursor and nothing else: marks, folds, and everything staged stay as they were. Grep is the one exception to the first part, since its rows already are a search: `/` there asks for a new one.

**Dir** (`C-S-d`, or `Dir: Open Working Directory`) lists a directory, rooted at the working directory of the pane it opened from.

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

A directory's size is not something the filesystem knows, so `M-S-s` walks each one in the background, a directory at a time, showing `...` until a total arrives. Turning it off stops whatever is still walking. Totals are kept while you stay in the same directory and dropped when the root moves.

An operation acts on the marked entries, or on the entry under the cursor when nothing is marked, never both: the status bar shows how many are marked and what the last operation reported. A name typed into a prompt is a name, so `../elsewhere` is refused rather than reaching outside the listing, and nothing overwrites an existing entry. Copy and move take a new name for one target and a destination directory for several. When another pane on screen is showing a directory (a listing, or an editor rooted somewhere), it is offered first: the prompt for one target starts out holding its path, and the list for several puts it at the top. A whole path typed out in full is taken as a directory to copy or move into, under the entries' own names.

`x` sends what it removes to the desktop trash (the freedesktop.org one, in `~/.local/share/Trash`), with a record of where each entry came from, so a file manager can restore it; `X` deletes for good. Where there is no trash to send to, `x` says so rather than deleting.

Every row has a gutter, a narrow column at the left edge (or the right, with `gutter-side "right"`), holding icons: a check on an entry marked for an operation and, in a git checkout, each changed entry's state, the set magic-vscode draws: modified, added, deleted, renamed, untracked, in conflict, or ignored. A directory shows the most pressing change inside it, and what lies in an untracked or ignored directory shows that directory's state. Clean entries leave the gutter blank. The marks are read in the background whenever the listing is, so they follow edits and stages made anywhere. Moving the root clears the marks, since a mark held over would count toward an operation in a listing it was never part of.

`C-x C-q` turns every listed name into editable text, in the manner of Emacs' `wdired`, to rename many entries at once. The names take the text editor's keys: Vim in Normal and Visual mode, the Emacs chords while typing. Each name is a line that stands for its entry, so nothing adds or removes one: `dd` empties a name rather than pulling the ones below it up, and `o`, `J`, and a line-joining `C-k` do nothing. `ZZ`, `C-s`, `C-S-c`, or `Enter` while typing applies the differences as renames, refusing the whole batch if any name is not a name or two land on the same one, before anything moves. A batch of more than one rename is listed for a yes first. Swapping two names, or a longer cycle, goes through: entries waiting for a name another still holds are parked under a temporary one until it is free; `C-S-k`, `q` with nothing part-typed, `ZQ`, or a second `C-x C-q` throws them away. The same Emacs chords edit the answer in any prompt an editor asks, such as a search or a new name, and `Up` and `Down` (or `C-p` and `C-n`) bring back what that same question was answered with before, newest first, the way a shell's history does.

Directories sort before files whatever the key, and moving the root, toggling a view option, or folding keeps the cursor on the entry it was already on. A block cursor sits on the active row's first non-blank column and follows it as you move, so an editor shows where a selection would start before you ask for one. `v` starts selecting from there, for copying out of a listing, and `V` does the same by whole lines: `hjkl` and the arrows move and extend, `w` and `b` step words, `0`, `$`, `g` and `G` jump, `V` and `v` switch between whole lines and characters, a second press of the same one drops the selection, `y` or `Enter` copies and leaves, and `Esc` cancels. While selecting it answers every key ahead of the editor, so the motions mean what they do in Vim rather than what the listing binds them to; chorded keys still reach the window, so splitting, zooming, and moving focus work from inside it. Dragging with the mouse selects the same rows.

Each entry carries an icon for what it is: bundled artwork keyed by extension, exact filename, or directory name, which needs nothing of the terminal font. The popups that list files, such as the file browser, the recent directories, and the text editor's open buffers, draw the same icons.

**Archives and two panes in Dir.** `Z` extracts an archive where it stands (`.tar.gz`, `.tar.xz`, `.zip`, `.7z`, `.rar`, `.gz`, ...), or compresses a file (`gzip`, keeping it) or a directory (`.tar.gz`) beside itself; `c` puts the marked entries into one archive whose name says its kind. `Dir: Open Two-Pane File Manager` opens two listings side by side: each copies and moves to the other's directory by default, and `=` with nothing marked compares the two directories.

**Git** (`C-S-g`, or `Git: Status`) shows the working tree of the repository the pane's directory sits in, as foldable sections: the changes, then the stashes, then what the upstream has that you do not, then the recent commits. A file changed both in the index and in the working tree appears in both, which is what lets one half be staged without the other. Every command runs from the repository root, off the event-loop thread, and the view re-reads the tree after anything that changed it; what a command reports, a failure included, shows in the status bar rather than being swallowed.

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
| `P` or `p` | Push menu, as magit's: `p` to the push remote and `u` to the upstream (each set first where it is not), `e` elsewhere, `o` another branch or commit, `T` a tag, `t` every tag; `-` then `f` `F` `i` `h` `d` toggles force-with-lease, force, force-if-includes, no hooks, dry run, and `RET` goes back |
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

The view is read by color as much as by name, the way magic-vscode's own git view is: a section heading wears the hue of what sits under it (cyan staged, yellow unstaged, green untracked, red contested), every other heading and the header block's own labels are blue, a file says what changed to it in that change's own hue, a branch is magenta and one on a remote green, a tag cyan, and anything the upstream has not seen yet — the commits ahead of it and the count saying how many — is red.

When git is part-way through something (a merge, a rebase, a cherry-pick, a revert, a bisect), a `State:` line in the header says so, what it is working on, how far along it is where git counts the steps, and the keys that finish it. Nothing in a porcelain status reports any of that, so without the line the only sign of an open rebase is a section of conflicts and no reason for them. A rebase, and a cherry-pick or a revert of several commits, also get a section of their own at the top of the view listing the commits they work through: the ones done, the one stopped on, and the ones still to come. Each row is its commit, so `Enter` reads it, `y` copies its hash, and the other keys that act on a commit act on it. It is all read from the files git writes it in, under the repository's own directory.

Closing a tab, a pane, the other tabs or panes, or the whole window asks first when a terminal has a program running in it (an agent such as Claude Code, an editor, a build; a shell sitting at its prompt does not count), and when an editor tab holds edits that are on no disk. Answer `y` to close; any other key keeps things as they are. Naming the running program uses the process list, so on platforms without `/proc` the question says "a program" instead, and a program that does not own the screen may not be noticed there.

With more than one pane, the status view and the commit editor open in another pane rather than over the one you are working in: the one beside it in the same row (to its right first, else its left) if there is one, otherwise the next in order, and asking for the status view again goes to the one already open there. With a single pane, one is split off to its right for them.

The status view watches the repository through the platform's file notifications and reads git's status again three seconds after the changes go quiet, redrawing only when the status differs: an edit, a stage from another terminal, a commit, a fetch. Build output and dependency directories at the top of the tree (`target`, `node_modules`, `dist`, `build`, and the like) are not watched, and inside `.git` only the index, `HEAD`, the references and the merge state count. Where the platform cannot be watched, it reads on a three second timer instead. A log, a diff, or any other output view is left alone while you read it, and an open file diff is re-read on each pass since the status cannot say that a file changed again.

A triangle opens every row a fold key acts on, pointing down while what hangs off it shows and right while it is hidden: section headings, file rows, and a commit's hunk headers all carry one. A hunk in the status view does not, since `Tab` there folds the file it belongs to rather than the hunk itself. Opening a row brings what it opened into view: a block that would run past the pane's bottom pulls its own row to the top, where the most of it fits, while one that already shows in full leaves the editor where it was.

On a section heading, `s`, `u`, and `x` act on every file in that section. On a hunk, or on any line inside it, they act on that hunk alone: the patch is built from the hunk and fed to `git apply` on standard input, so the rest of the file is untouched. A `v` or `V` selection goes finer, to single lines: `s`, `u`, or `x` over it stages, unstages, or discards just the lines it covers, the changes left out written so the patch still applies (an addition not chosen is dropped, a removal not chosen stays as context) and the header's counts worked out again. Discarding asks first. A selection reaching past one hunk acts on every file it covers instead, and either way the selection ends.

On a file in conflict, `.` then `o` or `t` settles it by taking one side whole, ours or theirs, and stages the result, which is what marks the conflict resolved. Discarding an untracked file deletes it, since `git restore` cannot reach a path that is not in the index.

A command that takes a name asks for it as a list rather than a blank line: checking out, merging or rebasing onto a branch offers every branch, deleting one offers the local branches, deleting a tag offers the tags, and removing or pruning a remote offers the remotes. Where there is nothing to list, the question falls back to being typed.

Every key that waits for a second one names what the second key may be, on the same card the terminal's own key hints use: centred over the window, above whatever is under it, so it cannot be pushed off the bottom by a long listing. It shows at once rather than after a pause, since the menus are how the editors are learned. Any key the menu does not offer closes it without doing anything, which is what makes a mistyped second key harmless.

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

Anything that wants a terminal of its own goes to a new tab: commit and amend in `$EDITOR`, and interactive rebase with its todo list. `o` resets to the commit under the cursor, or asks which one when the cursor is elsewhere.

A commit, a stash, a log, a listing, a blame, or a diff opens in a Git tab of its own beside the status, which stays as it is; asking for one already open goes to its tab. `j`/`k` move through it, `/` searches it, `+` asks for more of a log, `Enter` on a log's commit opens that commit in a tab of its own, `y` copies the first field of a line, which is the commit hash in a log or a blame, and `q` closes the tab, back to the one it was opened from. In a blame, where every row is a line of one file, `Enter` opens that file at the row under the cursor and `C-o` opens it in `$EDITOR` there.

A commit read with `Enter` opens shut, the way the status view does: the summary, then a `Changes` heading counting the files it touched, then one band per file, each counting the hunks it holds, each saying in words what the commit did to it (`modified`, `new file`, `deleted`, `renamed`) after its fold triangle and before its icon and path, with the patch waiting behind them. It folds the same way too. `Tab` on the heading takes the whole file list away and brings it back, `Tab` on a file band opens it to its hunk headers and shuts it again, `Tab` on a hunk opens that hunk's body, and `S-TAB` acts on the whole commit at once, opening every file and hunk of it, or shutting all of them, every hunk too, when none is folded.

In Git, `=` on a file opens it in the Diff editor, the last commit's copy against the working tree's, which can be edited there and written back.

**Compile** (`C-S-b`, or `Compile: Run a Task...`) runs a build in a tab of its own and makes every place its output points at a link, the way Emacs' compilation mode reads a build. `C-S-b` runs a project's only task at once, its Compile tab left in the focused pane's strip behind the work and how it ended said in the status bar; with several, it lists the project's tasks first and opens nothing until you choose one; the task run last leads the list, marked, and the last entry asks for a command line instead (`Compile: Run a Command...` goes straight there). The build runs out of the way: by default its tab is shown in the dock panel along the bottom of the window (configurable via `compile { dock #false }` to split beside), and the focus stays where it was. Every file being edited in the text editor is saved before a build starts. Each task has a tab of its own, so a test run never replaces a build still going, and running a task again reuses its tab. `M-S-b` (`Compile: Run the Last Build Again`) reruns the newest build from any tab.

The output streams in while the build runs, in the colors the tools give it, with the progress lines they redraw in place left as one line each; the cursor rides the end of it until you move it, and the tab spins on the strip while the build runs. When a build ends, Winter says so wherever you are, a failure as a toast with its error and warning counts, and by default a failed build opens its first error in the file you are in. The build runs under a hidden terminal in the project's root (the nearest directory with a `.winter` folder or a `.git`), handed to `sh -c` (`cmd /C` on Windows), with pagers turned off.

| Key | Action |
|---|---|
| `Enter`, or a double-click | Open the place on this line beside the tab, at its line and column |
| `C-o` | Open it in `$EDITOR` instead |
| `y` | Copy the place's `path:line` |
| `M-n` / `M-p` | Next, previous error or warning |
| `e` | Show only the errors and warnings (each with its message line), or everything again |
| `C-r` | Run the build again |
| `c` | Run another command in this tab, starting from this one |
| `i` | Type a line to the running build, for a tool that asks |
| `C-g` | Stop the build, and everything it started |
| `C-c C-v` | Open the PDF a LaTeX build made |
| `/`, `n` / `N` | Search the output |

The tasks offered are the document a `.tex` file belongs to when one is focused, then the ones `.winter/tasks.kdl` in the project's root lists, then what its files suggest (`cargo build`, `cargo test`, and `cargo clippy` beside a `Cargo.toml`, `make -k` beside a makefile, `go build` and `go test` beside a `go.mod`, the `build`, `test`, and `lint` scripts a `package.json` defines, `latexmk` beside a `latexmkrc`). Winter remembers the command each project was last built with. A task list names one shell command per task, run in the root or in the directory `cwd` names; `default=#true` marks the task a project builds with until another is chosen, `on-save=#true` one run whenever a file of the project is saved in Winter, and `env` children set variables for it:

```kdl
task "build" "cargo build --workspace" default=#true
task "check" "cargo check" on-save=#true
task "docs" "make html" cwd="docs" {
    env SPHINXOPTS="-W"
}
```

A place is read in the format each tool prints: GNU's `path:line:col: error: message` (gcc, clang, go, pytest, most linters, and TeX's `-file-line-error`), rustc's `-->` under its `error:` or `warning:` line, MSVC's `path(line,col): error`, and Python's `File "path", line N`, red for an error, yellow for a warning, and blue for a note. The place `F8` or `Enter` opened last is marked in the output. A path is read relative to the directory `make` says it entered, else where the build ran, else a directory up to four above that, which is where cargo prints a workspace member's paths from.

LaTeX builds with `LaTeX: Compile Document`, or the first task `C-S-b` offers from a `.tex` file: `latexmk -pdf -interaction=nonstopmode -file-line-error -synctex=1` over the document's root, found the way `F6` finds it, with TeX told not to fold its lines. Once the build is done, the log TeX wrote is read for what it did not print as a place: the warnings of LaTeX, its packages, and its classes, and the boxes it could not set, each placed in the file TeX was reading, listed after the build's own output under a count. The PDF tab showing the document reads the new one by itself, and `C-c C-v` opens it where none does. The `compile` block of `settings.kdl` sets the LaTeX command (`latex-command`), whether saving a `.tex` file in Winter rebuilds its document (`latex-on-save`, off by default), and whether a failed build opens its first error (`jump-to-error`, on by default).

**Grep** (`C-S-s`, or `Grep: Search Files`) finds the lines in the focused tab's project that hold some text, grouped under one heading per file.

| Key | Action |
|---|---|
| `/` | Search for text, starting from what you searched for last |
| `j` `k` or `Down` `Up` | Move down, up; the vim motions (`gg`, `G`, `C-d`, `C-u`) work too |
| `n` / `N` | Next, previous match, skipping the file names between them |
| `Home` / `End` | First row, last row |
| `M-n` / `M-p` | Next, previous file |
| `Enter` | Open the file in the text editor, at the line that matched |
| `C-o` | Open it in `$EDITOR` instead, at the same line |
| `y` | Copy the match's `path:line` |
| `C-r` | Run the same search again |
| `Esc` | Stop a running search |
| `C-x C-q` | Edit the matched lines in place, and write them back to their files |
| `M-c` / `M-r` / `M-w` | Toggle match case, regular expression, whole word, and search again |
| `M-f` | Only the files a glob matches (`*.rs`); empty for every file |
| `M-t` | Only the files of the type under the cursor, or every file again |
| `M-h` | An earlier search, newest first |

Where [ripgrep](https://github.com/BurntSushi/ripgrep) is installed the search runs through it, which reads the tree's `.gitignore` files and is faster on a large tree; otherwise Winter walks the tree itself. Either way the rules are the same: literal text in any case, hidden files read, and version-control and build directories skipped.

`C-x C-q` turns the matched lines into editable text, as Emacs' `wgrep` does, with the text editor's keys over them. `ZZ`, `C-s`, `C-S-c`, or `Enter` while typing writes every changed line back to its file, each file read and written once through the text editor's own save, so line endings and permissions survive. A line whose file no longer says what the search found there (changed since, or cut short in the list) is left alone and counted in the report, rather than written over. `ZQ`, `C-S-k`, or `q` with nothing part-typed, throws the edits away. As in every edit mode, `C-S-c` finishes and `C-S-k` throws away.

The walk runs off the event loop, so the pane stays live while a large tree is read, and `Esc` abandons it. `/` here asks for a new search rather than searching the rows, since the rows already are one, and `n` and `N` step through what it found. Matching is literal text, whatever the case.

The walk skips what would swamp the results rather than reading everything: `.git`, `.hg`, `.svn`, `node_modules`, and `target`, files over a megabyte, anything that does not read as text, and symlinks, which are never followed. It stops at 500 matches and says so in the header, because a query loose enough to pass that is one to narrow.

**Todo** (`Todo: List Keywords in Project` in the command picker; it has no default chord) is a Grep over the focused tab's project for the comment keywords `todo-keywords` names in `settings.kdl` (`TODO`, `FIXME`, `XXX`, `HACK`, and `BUG` by default), run as soon as it opens. A keyword is found as a whole word in its own case, so `TODO` finds `// TODO:` and `TODO(me)` but never `todos` or `TODOS`. Its keys are Grep's: `Enter` opens the line, `C-r` searches again, `C-x C-q` edits the lines in place, and `/` asks for other keywords, separated by spaces, for this list alone.

**Processes** (`C-S-,`, or `Processes: Monitor`) lists every process on the machine, refreshed every two seconds. CPU use is a share of one core, so a busy multi-threaded process passes 100. The columns are PID, NAME, CPU%, MEM%, RSS, VRAM, STATE, USER, and the command line. The tree lives in the NAME column, as an indent and a fold marker before the name, so the command stays whole. The list is the tree of who started whom by default, sorted by CPU within each level; `=` flattens it into one sorted list. Linux reads `/proc`, macOS and the BSDs read `ps`, and Windows asks PowerShell, so the working-directory column is missing on Windows and the state is always `run` there. A `VRAM` column of per-process video memory appears when `nvidia-smi` is on the path and answers, on any platform; there is no vendor-neutral way to read it, so AMD and Intel GPUs show no column.

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
| `f` | Filter by name, command, pid, or user; a match keeps the processes it descends from |
| `Esc` | Clear the filter |
| `x` / `X` | Terminate, kill (asks first) |
| `t` / `T` | Terminate, kill the process and everything under it (asks first) |
| `z` / `Z` | Suspend, resume (not on Windows) |
| `R` | Renice: a nice value from -20 to 19, or a priority class on Windows |
| `r` or `C-r` / `p` | Refresh now, pause or resume the refreshing |
| `y` / `Y` | Copy the pid, the command line |

A strip under the list describes the process under the cursor: its pid, parent, owner, state, CPU, memory and video memory shares, executable path, working directory, and the whole command line. It is left out of a pane shorter than twelve rows so the list keeps the room.

Signals go through the system's own `kill`, `renice`, and `taskkill`, so Winter needs no special privilege and is refused exactly where those are: another user's process needs `sudo` from a terminal. On Windows a terminate only asks a process's windows to close, which a console or background process declines; `X` forces it.

The toggles read the query the way VS Code's search toggles do: by default it is text found anywhere in any case; `M-c` makes its case count, `M-r` reads it as a regular expression (Rust's syntax, which ripgrep shares), and `M-w` wants it as a whole word. The header says how the search is reading when that is not the default, and a regular expression that does not parse is said in the status bar rather than finding nothing. The Todo list keeps reading its keywords as whole words in their own case, so the toggles do nothing there.

**Docker** (`Docker: Containers, Images, Volumes, Networks` in the command picker; it has no default chord) lists what the docker daemon knows, one kind at a time, refreshed every three seconds: the containers (running or not, with their image, state, status, ports, and compose project), the images, the volumes, and the networks. `Tab` and `S-TAB` step between them. Everything goes through the `docker` command, so the view needs no more right to the daemon than the shell it was started from, and says what docker said when it cannot reach it.

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
| `e` / `a` / `L` | A shell in the container, attach to it, follow its logs, each in a new terminal tab |
| `o` / `t` / `S` | Read its last 500 log lines, its processes, its CPU and memory, in the view |
| `P` / `H` | Pull the image again, read its layers |

What is only read (an inspect, a page of logs, a container's processes) fills the view in place of the list, with search and the motions over it; `r` reads it again and `q` goes back. A shell opens bash where the image has it and `sh` otherwise.

**System** (`System: Monitor` in the command picker; it has no default chord) shows the machine as a whole, refreshed every two seconds: host, operating system, uptime and load average; processor use as a total and a grid of cores with their clocks; memory and swap; each NVIDIA GPU's video memory, load and temperature; and the local disks with a bar for each. Bars turn yellow at 75 percent and red at 90. Linux reads `/proc` and `df`, macOS and the BSDs read `sysctl`, `vm_stat`, `ps` and `df`, and Windows asks PowerShell. macOS has no per-core counter without a system library, so it shows the total only, worked out from what the processes are using. The GPU section needs `nvidia-smi` on the path and is left out otherwise.

| Key | Action |
|---|---|
| `j` `k` or `Down` `Up` | Scroll down, up; the vim motions (`gg`, `G`, `C-d`, `C-u`, `C-f`, `C-b`) work too |
| `C-e` / `C-y` | Scroll one line down, up |
| `n` / `N` | Jump to the next, previous section |
| `r` or `C-r` / `p` | Refresh now, pause or resume the refreshing |

**Text Editor** opens the file an editor was pointing at, as editable text. It is not reached by a chord of its own: `Enter` on a file in Dir, Git, or Grep opens it, at the line that editor knew about. It opens as a tab right after the editor it was reached from, so closing it puts the listing back where you left it.

| Key | Action |
|---|---|
| `h` `j` `k` `l` or the arrows | Move by character and line |
| `w` `W` / `b` `B` / `e` `E` | Word starts and ends, crossing lines |
| `ge` / `gE` | Back to the end of the word before |
| `0` / `^` / `$` / `g_` | Column one, first non-blank, end of line, last non-blank |
| `{` / `}` | The paragraph before, after: the blank line past the text |
| `%` | The bracket matching the first one at or right of the cursor |
| `f` `F` `t` `T` / `;` `,` | To a character on this line; the same again, or back |
| `gg` / `G` / `20G` | First line, last line, line twenty |
| `C-d` / `C-u` / `C-f` / `C-b` | Half a screen, a screen, either way |
| `H` / `M` / `L` | Top, middle, bottom of what is on screen |
| `zz` / `zt` / `zb` | This line to the middle, the top, the bottom of the pane |
| `m{a-z}` / `'{a-z}` / `` `{a-z} `` | Mark this place; back to it, by line or exactly |
| `''` | Back to where the last jump started |
| `/` `?` / `n` `N` / `*` `#` | Search on, back; the next match, the one before; the word here |
| `i` `a` / `I` `A` | Type here, after here; at the first non-blank, at the end |
| `o` / `O` | Open a line below, above, keeping the indent |
| `v` / `V` | Select from here, by character or by whole lines |
| `R` | Type over what is there rather than pushing it along |
| `x` `X` / `r` / `~` | Delete a character here, before here; replace one; flip its case |
| `s` `S` / `C` `D` | Change a character, a line; change, delete to the line's end |
| `d` `c` `y` + motion | Delete, change, yank over it; doubled, over whole lines |
| `d` `c` `y` + `i`/`a` + object | The same over a word, paragraph, sentence, quoted or bracketed run |
| `>>` / `<<` / `>` `<` + motion | Indent, outdent whole lines |
| `p` / `P` | Put back what was taken, after the cursor or before it |
| `"{a-z}` / `"+` | Use that register for the next yank, delete, or put |
| `J` | Join the next line onto this one |
| `C-a` / `C-x` | Add one to the number at the cursor, take one off |
| `.` | Do the last command that changed the text again |
| `u` / `C-r` | Undo, redo |
| `C-s` | Write the file |
| `Z Z` / `Z Q` | Write and close; close, discarding edits |
| `C-o` | Hand the file to `$EDITOR` in a new tab, at the cursor's line |
| `]b` / `[b` | The next tab of the pane, the one before |
| `C-M-[` / `C-M-]` | Back through the tabs you last worked in, and forward again |
| `gb` | The buffer list (`C-x C-b`) |
| `C-/` | Comment the line, or the selected lines, out; or back in when they all are |
| `M-/` (typing) | Complete the word from the words around it, then the other buffers; again for the next |

A text editor holds one file, its buffer. Opening another file, from the file browser, `gx` on a path, or a jump to a definition, a reference, or a problem in another file, shows that file's buffer: the pane's tab on it, a buried one brought back, or a new tab. Opening a file that is already open goes to it. `]b` and `[b` step through the pane's tabs, `C-M-[` and `C-M-]` walk them by how recently you worked in them, and `gb` opens the buffer list. The jump history is the window's, so going back after a jump into another file returns to the file and the place it left from. A rename or a fix a language server makes across files edits each file in its own buffer, bringing a file not open in as a buried buffer, unsaved, for `C-x C-b` to find. Closing the tab buries the buffer; `C-S-k` kills it, asking first about unsaved edits.


### Commit messages

In a Winter pane, `git commit` edits its message in the built-in text editor instead of `$EDITOR`: Winter sets `GIT_EDITOR` to `winter --edit-wait`, which opens `COMMIT_EDITMSG` in a new text editor tab, in Insert mode, and holds git until that tab closes. The staged diff is a second tab in the same pane, one tab away, and goes when the text editor does. `C-c C-c` or `C-S-c` (or `ZZ`) saves and commits; `C-c C-k` or `C-S-k` (or `ZQ`, or discarding when asked) empties the message so git stops with "Aborting commit due to empty commit message". In front, these win over the window's own `C-S-c` (copy) and `C-S-k` (close the buffer). The header lists the two chords. `C-M-p` swaps the message above git's `#` comment lines for an earlier one from the repository's history (the last ten distinct messages), going further back each press, and `C-M-n` comes forward again to what you had typed. With no running window to ask, `winter --edit-wait` runs `$VISUAL` / `$EDITOR` instead, so git is never left without an editor.

An interactive rebase's plan (`git-rebase-todo`) opens the same way, as text the usual keys edit, with keys for the plan: in Normal mode `p`, `r`, `e`, `s`, `f`, or `d` alone (or after `C-c`, in any mode) sets the line's action to pick, reword, edit, squash, fixup, or drop and steps on to the next line, standing in for the Vim keys of those letters in this file only, `C-c j` and `C-c k` move the line down and up, `C-c C-c` or `C-S-c` (or `ZZ`) finishes, and `C-c C-k` or `C-S-k` empties the plan so git aborts the rebase. A plan git would refuse, one whose first commit squashes or fixes up into nothing, is not finished but said what is wrong with.

What every file shares, in every tab of the window: the clipboard, the named registers, the kill ring, the last search, and the last change `.` repeats; the last char search stays with the editor. What belongs to each file stays with it: its marks, its undo history, where the cursor and the window are in it, and what it looked like on disk.

A count typed before a command applies to it, as in Vim: `3x`, `2dd`, `5j`, `2d3w`. Insert mode is an Emacs-style field. Moving: `C-a`, `C-e`, `C-b`, `C-f`, `C-n`, `C-p`, `M-b` and `M-f` by word, `M-m` to the first non-blank, `C-v` and `M-v` by page, `M-<` and `M->` to the start and end, and `C-l` to centre the cursor. Deleting: `C-d`, `C-h`, `M-d` and `M-DEL` (or `C-DEL`) by word, `C-k` to the end of the line (at the end, the line break), `C-u` to the start, and `C-w` back a word. What the kills take goes on a kill ring, as in Emacs: kills made one straight after another join into one entry, so `C-k C-k` takes a whole line and `C-y` brings it back whole, and `M-y` straight after a `C-y` swaps what it put in for the kill before. Deletes and yanks made in Normal mode go on the ring as well, so `C-y` reaches them too. Editing: `C-t` and `M-t` transpose characters and words, `M-u` and `M-c` upper-case and capitalise the word ahead, and `C-o` opens a line and stays put. A word here is a run of letters and digits. The `M-` chords the window owns (`M-h` `j` `k` `l`, the digits, `M-x`) are left to it, so there is no `M-l` for lower-casing.

Every input field takes the same keys: a dialog's answer (an editor's question, the sudo password, the Z.ai key, a theme name), the picker's query, a tab's new name, a settings text row, and the find box a `/` search is typed in. A field holds the keyboard while it is up, so it adds the keys a file leaves to the window: `C-g` cancels as `Esc` does, `C-/` and `C-_` undo (a word at a time while typing), `C-\` redoes, `M-l` lower-cases the word ahead, and `M-\` deletes the spaces around the caret. `C-S-v` pastes into whichever field is up. The picker keeps its own keys for its list: `C-n` and `C-p` move the selection, `M-p` and `M-n` recall earlier queries, and in the file browser `Left` and `Right` go up and into directories, so `C-b` and `C-f` move the caret there.

Selecting with `v` or `V` puts the operators on what is selected rather than on a motion: `d`, `c`, `y`, `>`, `<`, `J`, `~`, `r`, and `p` all act on the selection and end it, `i` and `a` grow it to a text object, and `o` moves to its other end so it can be grown from either side. `Esc` drops it.

The mouse moves the cursor too: a click puts it on the character under the pointer, dragging selects from where the button went down, and the wheel scrolls, carrying the cursor with it so the two never part company.

There is one clipboard, the system's. With no register named, `y`, `d`, `c`, and `x` put what they took on it, a yank saying so and a delete quietly, and `p` and `P` put back whatever is on it, copied in Winter or anywhere else; `"+` and `"*` name the same clipboard. Every copy is also kept in the clipboard ring, which a paste falls back on where the system has no clipboard to give. `"_` is the black hole, which takes nothing, so `"_dd` deletes a line without replacing what `p` would put back. `"a` through `"z` name a register of the window's, kept off the clipboard and shared with the terminals' Normal mode, so `"ayy` in a file and `"ap` at a prompt move a line between them; `"A` through `"Z` append to one.

Searching is literal text rather than a pattern, and a search typed in lower case matches either case, while one typed with a capital in it matches exactly. `*` and `#` look for the word under the cursor without typing it out.

It colors what it is reading with the TextMate grammars VS Code and Sublime Text read with, over 200 of them: every common programming language, LaTeX and BibTeX, Markdown (code in a fenced block in its own language), diffs, commit messages and rebase plans, and the configuration formats (JSON, YAML, TOML, KDL, INI, `.env`, Nix, Terraform, Protocol Buffers, Dockerfile, Makefile, CMake, and the git dotfiles). The grammar is chosen by the file's name (`Makefile`, `.bashrc`, `COMMIT_EDITMSG`), else its extension, else its first line, so a script with a shebang and no extension is colored too. The dark preset colors each kind of thing as magic-vscode's One Color theme does: functions, types, macros, attributes, constants and members each in a hue of their own, and in LaTeX each section level, environment names, math, labels, citations, and escapes. The light preset keeps the same hues, each darkened until it reads on the light background. Any of them can be recolored by its TextMate scope in the `colors` block (`syntax { entity.name.function "#bf73df" }`; `comment`, `string`, `keyword`, `number`, and `type` still work); `Developer: Inspect Tokens and Scopes` (`<f1> M-f`) says which scopes the character under the cursor is in and which of them color it, and a name in the block that no scope goes by is reported when the settings are read rather than coloring nothing. A theme that sets no syntax colors colors comments, strings, numbers, keywords, and types from its own palette. In Rust and Lean, whose grammars leave most names without a scope of their own, a name is colored by what stands around it, as VS Code's own Rust grammar and the language servers color them: in Rust a type, an enum's variant, a call, a path's module, a field, a constant, and a variable where it is bound; in Lean a namespace, a tactic, a type, a constructor, an operator, and a name where a binder binds it. A variable is colored where it is bound and left plain where it is used. A commit message's conventional type (`feat:`, `fix(ui)!:`) is colored as a keyword, its summary is marked past 72 characters, and git's `#` comment lines, in a commit message and in a rebase plan, are colored as comments throughout, the branch, changed files, and headings in them included. A file in a language it does not know is painted as the plain text it may well be rather than guessed at, a long file is colored from the top down over a few frames rather than holding the window while it is read, and a line over 20,000 characters (a minified script, a data dump) is left plain, as VS Code leaves it. It colors what the characters themselves say and never decides that something is wrong with the code.

If the file changes on disk while it is open with nothing unsaved in it, it is read again the next time the text editor comes back to the front, so what is on screen is what is on disk. With unsaved edits it leaves your work alone and asks before overwriting at save time.

**Merge conflicts.** A file holding the blocks a merge, a rebase, or a cherry-pick leaves (`<<<<<<<`, `=======`, `>>>>>>>`, and `|||||||` with `merge.conflictStyle diff3`) draws each marker on a band, keeps each side's own colors, and counts the conflicts in the status bar. `M-n` and `M-p` step between them, wrapping round the file; with the cursor in one, `C-c o` keeps ours (the checked-out side), `C-c t` theirs, `C-c b` both with ours first, `C-c r` both with theirs first, `C-c a` the ancestor's version, and `C-c d` neither. `C-c m` (or `Merge: Resolve Conflict...`, also on the right-click menu) offers every choice as a list, with keeping one side in every conflict of the file at once. Each is one change one `u` takes back, and the status bar says how many are left until it says to save and stage.

**Comparing.** `Diff: Compare Buffer with File on Disk` shows what the edits not yet saved would change, `Diff: Compare Buffer with Clipboard` what pasting the clipboard over the buffer would, `Diff: Compare Buffer with Another Open Buffer...` the difference between two open buffers, edits included, and `Diff: Compare Buffer with File...`, `...with Revision...` (`HEAD` asked first), and `...with Two Files...` the rest. Each opens in the Diff editor below, where the buffer's side is written back to its file.

**Writing Markdown and LaTeX.** A `.md` file and a `.tex` file have chords of their own, as Emacs' markdown-mode and AUCTeX have them; each is also in the picker as `Markdown: ...` or `LaTeX: ...`. A chord that wraps text wraps the word under the cursor, or a selection within one line, and takes the wrapping off again where it is already there.

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
| `C-c d` | Turn the display the cursor is in from `\[...\]` into an `equation`, or back |
| `C-c b` / `C-c i` | Toggle `\textbf`, `\textit` |
| `C-c C-f` then `b` `i` `e` `t` `f` `c` | Toggle `\textbf`, `\textit`, `\emph`, `\texttt`, `\textsf`, `\textsc` (with or without `C-`) |
| `C-c w` | Count the words with `texcount`: text, headings, captions, following `\input` and `\include` |

**Outline.** `C-S-o` (`Navigate: Outline of This File...`) lists the file's headings in Markdown, its sectioning commands in LaTeX, and in a program every line its grammar names a definition on (a function, a type, an `impl`, a class, a module, a TOML table), each indented by how deep it sits. It is gone through as the jump list is: the cursor follows the entry picked, starting on the one the cursor is under, and goes back if the list is put away. Like the coloring, it reads what the grammar says and never analyzes the code.

**Colors and rich text.** A color a file writes as `#rrggbb` or `#rrggbbaa` is drawn on that color, in black or white to stay readable, so a theme or a style sheet reads as what it looks like; `Editor: Toggle Color Swatches` turns it off. `Editor: Copy as Rich Text (Syntax Colors)` puts the selected lines, or the whole file, on the clipboard as HTML in the theme's colors, for pasting into a document or a slide, with the plain text beside it for what takes only that.

**Changes made elsewhere.** The editor looks at the file it is showing once a second. When another program has written to it (another editor, a formatter, a `git checkout`) and nothing is edited here, the file is read in again with the cursor where it was; with edits here not yet saved, they are kept and the status bar says once that the file changed, and saving asks before writing over it. A file on a remote host is read again on `C-r` instead, to spare it a round trip every second.

**Wrapped lines.** Long lines wrap at the pane's edge, at the last word that fits (`M-z` turns wrapping off and on for the tab). The rows a line continues on are indented as `editor-wrap-indent` says (also on the settings page as Wrapped line indent): `"semantic"`, the default, starts them under the line's text, past what opens it, a Markdown list item's bullet and task box, a heading's `#`s, a quote's `>`s, a LaTeX `\item`, or a comment's marker, and else under the line's indentation; `"same"` under its indentation alone; `"none"` at the gutter. An indent that would leave a row less than twelve columns of text is dropped. Over a wrapped line the motions go by the rows on screen, as Vim's `gj` and Emacs' `visual-line-mode` do: in Normal and Visual mode `j`, `k`, the arrows, `H`, `M`, `L`, `C-d`, `C-u`, `C-f`, and `C-b`, keeping the column a run of them started at, with `g0`, `g^`, `gm`, and `g$` on the screen row and `gj` and `gk` going by whole lines instead; while typing `C-n`, `C-p`, the arrows, `C-v`, and `M-v`, with `C-a`, `C-e`, `Home`, and `End` going to the screen row's ends. `0`, `^`, `$`, and an operator's motion (`dj`, `d$`) still take whole lines, as they do in Vim.

**Scroll margin.** The view keeps `scroll-margin` screen rows (5 by default; Scroll margin on the settings page) above and below the cursor as it follows it, in the text editor and in the Dir, Git, Grep, and other listings alike, as Vim's `scrolloff` does: a move into the margin scrolls the view a screen row instead of the cursor reaching the edge. `zt` and `zb` leave that many rows above or below the cursor's, and `H` and `L` stop that far in, except at the very top and end, where there is nothing more to scroll to. The margin is held under half the pane, and the wheel, which moves the view and leaves the cursor, is not held to it. Over wrapped lines it counts screen rows, as the scrolling does.

**Hidden markup markers.** As magic-vscode does, the text editor hides the markers of markup and draws what they mark: in Markdown the `**` of `**bold**` and the `_` of `_italic_` (the text drawn bold or italic, nested ones both), the `~~` of `~~struck~~` (the text struck through), `<u>`, `<ins>`, `<del>`, and `<s>` tags (underlined or struck), the backticks of `` `code` ``, the `$` of `$math$`, a heading's `#`s, and a link's `[` and `](url)`, leaving its text underlined; in LaTeX `\textbf{` and its `}`, and likewise `\textit`, `\emph`, `\textsl`, `\texttt`, `\textsf`, `\textsc`, `\underline` and `\uline` (underlined), `\sout` (struck), and `{\bf ..}`, `{\it ..}`, `{\em ..}`; Org's `*bold*`, `/italic/`, `_underlined_`, `+struck+`, `=verbatim=`, and `~code~`; reStructuredText's `**bold**`, `*italic*`, and ` ``code`` `; AsciiDoc's and Typst's `*bold*`, `_italic_`, and `` `code` ``; and the Markdown in a `///`, `//!`, or `/** .. */` doc comment of a language commented with `//`. Code blocks, math blocks, and LaTeX comments are left as written. The markers stay in the file and the cursor walks over them; they take no room on screen, so the caret, clicks, and wrapping go by what shows. `editor-markup-markers` (or Markup markers on the settings page) says when: `"always-hidden"` (the default), `"show-on-cursor"`, which shows a span's markers while the cursor or the selection touches it and a heading's on the cursor's line, or `"visible"`. `Editor: Toggle Hide Markup Markers` turns hiding off and back on, and `Editor: Toggle Always Hide Markup Markers` switches between always and where the cursor is, as magic-vscode's two toggles do.

**Spelling.** The prose of a file is checked against the dictionary ported from magic-vscode, American English with the words you have added: a LaTeX file's text but not its commands or math, a BibTeX file's titles and notes, a Markdown file's text but not its code, a plain text file whole, and in a program its comments alone. LaTeX, BibTeX, and Markdown are checked from the moment they open; `M-S-i` checks any other file, or stops checking one. A word spelled wrong is underlined as a language server's remark is, beneath any warning or error of the server's own, and checked again after every edit. In Normal mode `]s` and `[s` go to the next and previous one, round past the file's ends, saying how it might be spelled; `z=` offers the suggestions for the word under the cursor, the one chosen put in its place with its capital kept, as `M-\` (or `C-.`) does on a word spelled wrong, in any file and with or without a language server, its fixes listed ahead of any the server has; and `zg` takes the word as spelled right, in every editor and from then on, kept one a line in `spelling/words.txt` beside `settings.kdl`.

**Completion.** While typing, `C-SPC` opens a popup of what could go at the cursor, and it opens by itself once a word is three letters long or straight after a `\`, `/`, `{`, or `#` (`completion { auto #false }` keeps it to `C-SPC`, and `min-chars` sets the length). `C-n` and `C-p` (or `Down` and `Up`) move through it and `Tab` takes the entry; `RET` takes it too once the popup was asked for or moved through, and otherwise starts a new line as it always does; `Esc` closes it and leaves you typing. Each entry is marked with a letter for what it is and named with the back-end that offered it. The back-ends are asked in order, as Emacs' company asks them: the first with anything to offer decides where the completed text starts, and those after it answering at the same place are merged in under it. `words` offers what is in the open buffers, nearest the cursor first; `paths` the files and directories a path being typed goes on to; `snippets` each language's templates (`fn`, `test`, `fig`, `tab`, `code`), whose fields' defaults are put in with the cursor on the first, and your own, one file each in `snippets/<language>/` beside `settings.kdl`, named by the word that expands it; `keywords` the language's reserved words; `latex` commands after `\`, environments after `\begin{` (the one left open first after `\end{`), the project's `\label`s in a `\ref{`, and its `.bib` keys with their titles in a `\cite{`; and `markdown` the files a link's `](` can point at and the document's headings by their anchors after `#`. Which a language asks, and in what order, is `completion { backends { rust "snippets" "keywords" "words" } }`. `M-/` still completes in place from the open buffers, a press a word, as Emacs' dabbrev does.

**Language servers.** A language server reads the project and answers what the keys ask of it. None starts by itself: `LSP: Start the Language Server for This File` (picker) starts the one for the file in front, rooted at its project (the outermost `Cargo.toml` inside the repository for Rust, the nearest `go.mod`, `pyproject.toml`, `package.json`, and so on, else the repository, else the file's directory), and every open file of that project is handed to it as it changes. Winter knows rust-analyzer, pyright (or basedpyright, pylsp, ruff), gopls, clangd, typescript-language-server, texlab, marksman, bash-language-server, ocamllsp, taplo, yaml-language-server, vscode-json-language-server, lua-language-server, haskell-language-server, zls, and nil, and uses the first of a language's that is installed; `lsp { servers { python "pylsp" } }` names another, and `lsp { auto-start "rust" }` starts a language's server as soon as one of its files is open. `LSP: Stop`, `LSP: Restart`, and `LSP: Show Running Language Servers` do what they say, and what a server is busy with (indexing, say) shows in the status bar.

While a server serves the file, the keys follow magic-vscode's: `M-.` (or `F12`, or `gd` in Normal mode) goes to the definition, and `M-<left>` / `M-<right>` (or `S-M-b` / `S-M-f`) back and forward over the jumps; `C-c C-/` (or `gr`) lists the references; `C-c C-[`, `C-c C-]`, and `C-c C-\` go to the type's definition, the implementation, and the declaration; `M-,` or `M-0` (or `K`) opens a card saying what the name under the cursor is, and resting the pointer on a name does too; `C-S-SPC` shows the signature of the call being typed; `C-c C-r` (or `F2`) renames the name everywhere, opening the files it changes as unsaved buffers; `M-\` (or `C-.`) offers the fixes and refactorings there, after the spelling's own for a word spelled wrong, in one list; and `C-S-M-i` formats the file. The problems the server finds are drawn with a wavy line under them in the hue of how bad they are, the line number in the same hue, and the status bar counts the errors and warnings; `S-M-n` and `S-M-p` (or `]d` and `[d`) step through the file's, with a card saying what each is, while `M-e` goes to the project's first error, `M-g M-n` and `M-g M-p` step through the whole project's, and `C-M-p` lists them all. Completion asks the server too, first among the back-ends (`lsp`), as you type and after the characters it names (`.` and `::` in Rust); an entry that needs an import brings it in.

**GitHub Copilot.** While `copilot { enable }` is on (the default), Winter starts GitHub's `copilot-language-server` the first time a file is open in the editor, signed in as the Copilot sign-in kept on this machine; `Copilot: Sign In` signs in through GitHub's device flow (the code is put on the clipboard and the page opened in the browser), and `Copilot: Sign Out` and `Copilot: Show Status` do what they say. The server is the one `copilot { server-path }` names, else the first found on the `PATH`, in Winter's own directory, or in magic-vscode's; `Copilot: Install Language Server` fetches it from npm into Winter's directory. As you type, Copilot's suggestion shows dim at the cursor (ghost text), its lines past the first in a card under it, and the status bar says `Copilot`, with `2/3` where it offered several. `Tab` (or `C-RET`) takes all of it, `M-f`, `M-<right>`, or `C-,` its next word, `C-.` its next line, `M-n` / `M-p` the next or previous suggestion, and `C-g` hides it; typing what it shows keeps it showing, and `C-\` asks for one where none came. After a change Copilot may also expect a next edit elsewhere (`copilot { next-edit }`): it shows in a card, `Tab` makes it and `Esc` leaves it. The completion popup, when open, comes first for `Tab`.

For anything past that, `C-o` hands the file to `$EDITOR`: and it saves before handing over, so `$EDITOR` opens what you were looking at rather than what was last written. A file it cannot open as text says so and stays closed: anything holding a NUL byte, anything that is not UTF-8, and anything over four megabytes, all of which it would otherwise write back as something the original was not. A UTF-8 byte order mark at the head of a file is kept aside rather than shown, and written back on save.

Saving writes through a temporary file beside the original and renames it over the top, so an interrupted write leaves the previous contents whole rather than a truncated file. Undoing back to what was last written leaves nothing to save, so closing does not ask about edits that are no longer there. A file's permission bits, its line endings, and whether it ended with a newline all survive the trip, so saving one edited line does not show up as a whole-file diff. If something else has written to the file since it was opened, saving asks before overwriting rather than throwing that away.

**Diff** is Emacs' ediff: texts compared region by region in a tab of their own, two side by side, three against the middle one, or two merged into a third. It opens from the text editor's `Diff:` commands, from Dir's `=` on two or three marked files, from Git's `=` on a file (the last commit against the working tree), from `Merge: Merge Another File into This One...`, and from `Merge: Resolve Conflicts in a 3-Way Merge`, which sets a conflicted file's two sides (and the ancestor `diff3` keeps) against each other and writes the merge back over the file. Each region is tinted in its text's hue (A red, B green, the third blue), filler stands where a text has no line, and the current region is marked in the gutter with its changed words drawn louder. `n` or `SPC` and `p` or `DEL` step through the regions, `J` jumps to one by number, `j`, `k`, `C-d`, `C-u`, `v`, `V`, `gg`, and `G` scroll every text together, and `<` and `>` scroll sideways. In a comparison `a` copies the region from A to B and `b` from B to A (`ab`, `ac`, `ba`, `bc`, `ca`, `cb` with three texts), and `ra`, `rb`, `rc` put a region back as it was; in a merge `a` and `b` keep one side, `+` both between markers, `o` the ancestor's, `r` what the merge first held, and `$$` steps only through the conflicts left. `##` leaves whitespace out of the comparison and `#c` case, `!` works the regions out again, `@` turns the changed words off, `|` turns the columns into bands stacked one above another, `~` swaps A and B, and `/` shows a merge's ancestor too. `ea`, `eb`, or `ec` edits a text in place with the editor's Vim keys, the regions following, until `C-c C-c` or `C-S-c`; `wa`, `wb`, `wc` (or `C-s` while editing) write a text back to its file, `D` shows the differences as a unified diff, `i` says where the session stands, and `q` quits, asking first about a text not written.

**Markdown preview.** `F7` (or `S-F6`, `F6`, or `Markdown: Preview Beside`) shows the Markdown file in front rendered in the pane beside it, as a web page in the theme's colors: the document is written out as HTML, laid out by the Blitz engine, and drawn on the GPU. Headings, emphasis, links, lists, tasks with boxes, quotes, tables, and images beside the document read as a rendered README does (nothing is fetched from the network, so a remote image or badge is left out), and code blocks are colored by their language with the editor's own highlighting. Math, inline between `$` and on its own line between `$$`, is typeset in the text's color, and a fenced `mermaid` block is drawn as a diagram in the theme's colors (shown as code where it does not parse); the page runs no scripts. It follows the file's text as it is typed, unsaved, and keeps the block holding the cursor's line in view (`f` lets go of the cursor, or follows it again); with no editor on the file it reads the file from disk whenever it changes. The wheel, `j`/`k`, `C-d`/`C-u`, `C-f`/`C-b`, `gg`, and `G` scroll it. A click on a link opens a web address in the browser, a file beside the document in Winter, and a `#heading` link scrolls to that heading. A drag elsewhere selects text and copies it as the selection grows, as a terminal's selection does; `ESC` lets go of it. `RET` or `o` goes to the source line of the block at the top of the pane, a double-click to the line of the block under the pointer, and `/` finds text in the source and scrolls to it (`n` and `N` step through the matches).

**Occurrences and the shape of the code.** The word under the cursor is outlined wherever else it shows, a thin border around each occurrence over its own colors (`Editor: Toggle Occurrence Highlights` turns it off), and `M-n` and `M-p` step between its occurrences. `C-;` changes them all at once, as magic-vscode's occurrence mode and Emacs' iedit do: what is typed goes into every occurrence together, drawn on a band, `Tab` and `S-Tab` step between them, and `Esc` or `RET` ends it; with a selection on one line, its text's occurrences are changed. `C-c C-a` and `C-c C-e` go to the start and the end of the function or module the cursor is in, as its grammar names it, `C-M-i` selects it by lines (pressed again, the one around it), and `zj` and `zk` step between the blocks the indentation makes, as Vim steps between folds.

**Typing by context.** In Insert mode `RET` carries a line comment (`//`, `///`, `#`, `--`) on to the next line, and a block comment's ` * `; in a Markdown list it goes on under the item's text, and on an item with nothing in it moves its bullet down a line. On a Markdown heading `Tab` and `S-Tab` add and take away a `#`; on a bullet they indent and outdent the item, the bullet turning `-`, `+`, `*` by depth; and `S-DEL` just past a bullet blanks it out, so the text reads on as a paragraph of the item above.

**Writing help.** `M-s i` inserts a math or Greek symbol picked from a list: the symbol itself, or its command in a LaTeX file. `M-s e`, `M-s ]`, and `M-s [` go to the first, next, and previous word spelled wrong in the whole project's prose, its Markdown, LaTeX, BibTeX, and plain text files, open buffers read as they stand. `S-M-t` looks the word under the cursor (or the selection) up in your GoldenDict dictionary, the largest `.dsl.dz` under `~/Setup/GoldenDict`, `~/Workspace/data/dictionaries`, or `~/.local/share/goldendict/content`, and shows its entry in a card; the first look-up reads the dictionary's headwords on a thread of its own, after which each is at once. In a `.bib` file `C-S-M-s` sorts the entries by year, and `BibTeX: Sort Entries by ...` sorts them by citation key or year either way round.

**Clipboard ring.** Every copy made in Winter is kept, newest first, along with what is on the clipboard when the ring is opened: `Clipboard: Paste from Clipboard Ring...`, or `M-y` in the editor when the key before was not a yank (after `C-y` it steps back through what was yanked, as before), lists them and pastes the one chosen wherever typing goes. `Clipboard: Clear Clipboard Ring` empties it.

**Small things.** `C-l` puts the cursor's line in the middle of the pane, then at its top, then at its bottom, as Emacs' recenter does. The line flashes after a jump to a definition or an offset (`Editor: Flash Current Line` flashes it on asking). `M-g M-c` goes to a character offset from the file's start and `M-g c` says the one the cursor is at. `C-M-'` opens the scratch file beside the settings, for notes of no project, and `C-S-M-o` in a Rust file runs `cargo fix` over the project in its Compile tab.

**File browser** (`C-S-f`, or `File: Browse Files`) walks the working directory in the command picker rather than in a pane, for the times you know roughly where a file is and want it open rather than listed.

It opens where the terminal points, when it points at a file or directory that exists: the Normal-mode cursor, else the mouse selection, else the shell's cursor (on a path, or just past one you typed), else the rightmost path on the last line printed. A file opens the browser on its directory with that file selected; a directory opens the browser inside it. Paths resolve against the shell's working directory, and `~`, `..`, and `path:line:col` suffixes are understood. With nothing found, or over an editor, it opens on the working directory as before.

| Key | Action |
|---|---|
| Typing | Filter this directory's entries |
| `/` after a name | Go into that directory and keep typing from there: `src/`, `../`, `~/`, or a filter like `sr/` that selects `src/`. `/` with nothing typed goes to the root |
| `Up` `Down` or `C-p` `C-n` | Move through what is showing |
| `Enter` on a directory | Open it in Dir, beside the focused pane |
| `Tab` / `Right` on a directory | Go into it: the browser stays up, rooted there |
| `Enter` on a file | Open it in the text editor, over the focused pane |
| `C-h`, or `Left` | Back up to the parent directory (`Backspace` only deletes typed characters) |
| `C-S-f` again, or `Esc` | Put it away |

Directories come first, then files, with the hidden ones after each group and `../` at the top, so a listing opens on what is usually wanted and typing reaches the rest. The path being browsed is shown above the list, and each row's icon says whether it is a directory or what kind of file it is. It reads one directory at a time rather than walking the tree, so it opens instantly however large the tree below it is.

**Files as root, in containers, and over SSH.** The same browser and editor reach files you cannot touch as yourself. A remote file is named by a URI, and the URI goes everywhere a local path does: the browser, the text editor, the recent list, and a saved session.

| URI | Reaches |
|---|---|
| `sudo:///etc/hosts` | `/etc/hosts` as root, through `sudo` |
| `docker://web/etc/nginx.conf` | Inside the running container `web` |
| `docker://www-data@web/var/www` | The same, as user `www-data` |
| `ssh://me@example.com:2222/srv` | On an SSH host, as `[user@]host[:port]` |
| `ssh://[::1]/etc` | An IPv6 host, in brackets |
| `docker://` or `ssh://` | The containers, or the `~/.ssh/config` hosts |

`sudo:/etc/hosts`, `admin:///etc/hosts` (what GNOME Files and Dolphin call root, and what Winter wrote before), and `sftp://` are read as well, as the same places. A space or another character a URI cannot hold is written as a `%` escape (`%20`), while letters beyond ASCII stay as they are. The host list follows the SSH config's `Include`s.

Three commands open the browser on them: `File: Browse Files as Root` on the working directory as root, `File: Browse Docker Containers`, and `File: Browse SSH Hosts`. Choosing a container or a host goes into its root. From there the browser works as it does locally: an absolute path typed with `/` stays on the target, so `/var/log/` goes to that directory in the container, and `../` at a target's root goes back to the list of targets. A URI typed in any browser goes where it says once its last `/` is typed: `ssh://myhost/`, `docker://web/`, or `docker:///` for the list of containers.

The text editor shows where a buffer lives in the status bar, as `[sudo]`, `[docker://web]`, or `[ssh://me@host:2222]`, and reads and saves it the same way it was reached. A save copies the new contents over the file rather than replacing it, so the file keeps its owner, mode, and any link to it, and a transfer cut short leaves the file as it was. A local file only root can read opens as root without being asked for, and saving a file you can read but not write asks whether to save it as root.

Where `sudo` wants a password, Winter asks for it in a dialog that shows a dot for each character typed. The password goes to `sudo` once and is not kept: `sudo`'s own timestamp spares the next read or save the question for as long as it would in a shell, and with `NOPASSWD` nothing is asked. Until a buffer of root's has been read, it stands empty and every key reads it again, so nothing can be saved over the file. SSH runs without prompting (`BatchMode`), so a host needs key authentication (an agent is fine) and an accepted host key. Each command has a deadline, so a host that stops answering is reported rather than waited on.

Docker and SSH work from Linux, macOS, and Windows alike, since the scripts run on the far side, in the container or on the host, which needs `sh` and `cat` (the listing uses `stat` where there is one, and `ls` where not). On Linux and macOS one SSH connection per host is shared for ten minutes, so browsing costs a round trip rather than a handshake; Windows' OpenSSH cannot share one, so each step there connects anew. Root (`sudo:`) needs `sudo`, so it is for Linux, macOS, and the BSDs: Windows has no way to become administrator without a dialog of its own, and says so.

A session restored with remote buffers in it reads each one when it is first shown, not at startup. Listing and reading run while you wait, so a slow host is felt as a pause. A distroless image, with no `sh`, cannot be browsed. Dir, Grep, Git, `gx`, `$EDITOR` (`C-o`), and the PDF viewer stay on the local disk.

**PDF** opens wherever a file does, on any path ending in `.pdf`: `Enter` in Dir, Git, or Grep, the file browser, or a `gx` reference. Like the text editor it has no chord of its own and opens as a tab beside the editor it was reached from, so closing it puts the listing back. Winter draws the document itself, with a PDF renderer written in Rust and built into the binary along with the fonts a PDF may name without carrying, so a document opens with nothing installed and no network access. Its pages are drawn on a thread of their own, only those in view and one either side, so a long document opens at once and scrolling never waits: a page not drawn yet shows as a blank sheet until it is. The same thread reads every page's text while it has nothing to draw, so a search or a motion seldom waits on that either. Because the pages are drawn like everything else in the window, the picker, menus, key hints, and notifications sit over a document as they do over anything else. The viewer reads nothing but the file it was opened on, and an encrypted PDF opens if it needs no password.

| Key | Action |
|---|---|
| `{count}` before a motion | Repeat it that many times, as vim does: `3j`, `12k` |
| `j` `k` or `Down` `Up` | Scroll down, up |
| `h` `l` or `Left` `Right` | Scroll left, right, for a page zoomed past the pane |
| `C-e` / `C-y` | A line down, up, without moving what is on screen otherwise |
| `C-d` / `C-u` | Half a screen down, up |
| `C-f` / `C-b`, `Space` / `b`, `PageDown` / `PageUp` | A screen down, up |
| `gg` / `G`, or `Home` / `End` | First page, last page |
| `{count}gg` / `{count}G` | Go to that page |
| `]` / `[` | Next page, previous page |
| `zt` / `zz` / `zb` | Put this page at the top, middle, bottom of the pane |
| `C-g` | Say which file, which page of how many, and how far through |
| `g C-g` | Count the words and characters selected, or with nothing selected, the whole document's words |
| `+` / `-` | Zoom in, out |
| `=` | Fit the page to the pane's width again |
| `P` / `T` | Fit the whole page in the pane; fit the text's width, its margins off the sides |
| `C` | Crop every page to the box its text falls in (measured over the first ten pages), or show them whole again |
| `E` | Center the pages across the pane |
| `V` | Show the pages small down the pane's left, the one being read framed; a click on one goes there |
| `:` | Ask which page to go to |
| `O` | Open or close the outline |
| `I` | Invert the pages' colors, light text on dark, or put them back |
| `f` | Label the links in view; typing a label follows its link (a URL opens in the browser) |
| `F` | Label the links in view; typing a label shows where its link leads, under it, for `Enter` to follow |
| A click on a link | Follow it |
| Resting the pointer on a link | After a moment, show where it leads under it (a URL is shown in the status bar); moving off the link closes it |
| `M-S-o` | Outline every link in view (a frame around a link into the document, a line under a URL), or only the one under the pointer again |
| `C-o` / `C-i`, `M-<left>` / `M-<right>`, `M-S-b` / `M-S-f`, the mouse's back / forward buttons | Back to where the last jump left (a link, an outline entry, `gg`, `G`, `Home`, `End`, `:`), forward again; `C-` and a mouse side button shows the pane's previous / next tab instead |
| `Enter` / `K` with a cursor | Follow the link under the cursor, show where it leads |
| `S`, or a double-click | Go back to the LaTeX source of the cursor's spot (SyncTeX), or of the line being read with no cursor |
| `s` | Label the start of every line in view; typing a label goes back to that line's LaTeX source |
| `M-S-s` | Tint the sentences each their own color, for reading the prose over, or take the tints off |
| `M-S-i` | Underline the words spelled wrong, or stop |
| `C-M-n` / `C-M-p` | Next, previous word spelled wrong, round past the ends: marked, with how it might be spelled |
| `zg` | Take the word last stepped to as spelled right, from now on and in every editor |
| `i` | Put a cursor in the text, for the motions below |
| `/{text}` / `?{text}` | Search down the document, back up it; the cursor lands on what is found |
| `n` / `N` | The same search again, the same way or the other |
| `Esc` | Abandon a half-typed command |

A rebuilt PDF is read again on its own: the viewer looks at the file twice a second, and once a change has held still from one look to the next, so a build still writing it is not read half done, it reads the new document while the old one stays on screen, then shows it at the same spot of the same page, or at the SyncTeX mark being shown. The pages drawn from the old file stay up until they are drawn anew, so nothing goes blank; the cursor goes, its line perhaps moved, but the last search stays for `n`. A file that will not open keeps the last good document, is tried a few more times, then said so of.

Links are the document's own: a `\ref`, a `\cite`, a contents line, or a URL, as hyperref writes them. A preview is cut from the target's page as it is drawn at the current zoom, a paragraph or two from the spot, framed under the link (over it near the pane's bottom). A publisher's copy often keeps a `\cite`'s link but drops the table its destination was named in: such a link is read for its own number (the `12` of `[12]`) and sent to the line of the reference list that starts with `[12]`, the list found as the longest run counting up from `[1]` at line starts, read back from the last page. A link whose number has no such entry leads nowhere and is left out.

SyncTeX works both ways for a document built with `-synctex=1` (latexmk and most editors' builds do it), with the `synctex` program TeX Live and MiKTeX ship. Back from the PDF, `S`, a double-click, or the menu's SyncTeX Backward (offered when the PDF has its `.synctex.gz`) asks `synctex edit` which line of which file set the spot, then narrows that line down by the text around the spot, which SyncTeX alone often places at its paragraph's first line, and looks through the files an `\input` or `\include` near there brings in when the text is not where SyncTeX said. The file opens in the text editor at that line. Forward from a `.tex` file, `F6` (the picker's "LaTeX: Show in PDF"), as magic-vscode's, finds the document's root: the file a `% !TEX root = ...` comment names, or the one declaring `\documentclass` in its directory or up to three above (of several in one directory, the one whose last build read the file, by its `.fls`, else the one with the newest PDF). Its PDF is looked for where latexmk was told to write it (`$out_dir`, else `$aux_dir`, in the project's `.latexmkrc` or `latexmkrc`), else beside the root. It shows in the tab already showing it, else in a new tab of this pane; `S-F6` or `F7` ("LaTeX: Show in PDF in the Other Pane") opens it in the other pane. The box the line was set in is scrolled to the reading line, marked until the next key or click. A line SyncTeX does not find, or a PDF built without SyncTeX data, still opens the PDF, saying so. From a `.bib` file the line searched is the first `\cite` of the entry under the cursor in the document's `.tex` files, and outside any entry the PDF just opens.

A toolbar takes the pane's top row, drawn in the window's own font as the status bar is rather than in the pane's cells, and the pages scroll under it. On the left it names the section being read, taken from the document's own outline (its bookmarks), and a click there drops the outline down under the bar, a card over the pages with each section's page at its right: folded to the top level but for the way down to that section, moved through with `j` and `k`, unfolded with `l` and folded with `h` (or `Tab`, or a click on a row's chevron), and jumped to with `Enter` or a click, which closes it again; `Esc`, `q`, `O`, or a click on the pages closes it without moving. A document that lists no sections says so there instead. On the right are the zoom, `−` and `+` either side of the percentage, where a click asks for one: a number, or `w`, `h`, or `p` to fit the page's width, its height, or the whole page; then `‹` and `›` either side of the page number, where a click asks which page to go to; then `◐`, which inverts the pages' colors. Inverting draws the pages again, so the ones in view change as soon as they are drawn.

The motions are vim's, and they compose the way vim's do: a count is typed before the motion it multiplies and is spent by it, `g` and `z` wait for the key that completes them, and the pane's own key-hint card says what finishes one while it is pending. The header shows a count as it is typed, so one entered by accident is visible before it moves anything. The two places a count is read as a position rather than a repeat are `gg` and `G`, exactly as in vim: `5G` is page five, not five pages on.

Where the motions differ from vim's, it is because a document is not a buffer. There is no cursor to move, so `h` and `l` scroll sideways instead of stepping characters, and `C-d` moves half the pane rather than half a file's worth of lines. `zt`, `zz`, and `zb` move the window and not the page being read, which is what they do in vim too.

`i` puts a cursor in the document's own text, and from there the keys are vim's text motions rather than the window's. `Esc` takes it back out, one step at a time: a half-typed command first, then the selection, then the cursor itself.

The mouse selects text too. A drag selects what it passes over and copies it to the clipboard as it goes, the status bar saying how many characters it holds. A click puts no cursor in the text: the cursor comes only with `i`, which starts it where a drag left off, and once it is in the text a click moves it. With it in, the selection is the same one `v` and `V` make, so `o` swaps its ends and the motions go on extending it from where the mouse left it; `y` copies it again once they have. Every copy from a PDF is announced by its length rather than echoed, since it may run to pages.

| Key | Action |
|---|---|
| `h` `j` `k` `l` or the arrows | Move the cursor left, down, up, right |
| `w` / `b` / `e` | Forward a word, back a word, to a word's end |
| `0` / `^` / `$` | Start of the line, its first non-blank, its end |
| `{` / `}` | Back, forward a paragraph |
| `(` / `)` | Back, forward a sentence on this line |
| `f{char}` / `F{char}` | To the next, previous occurrence of a character on this line |
| `t{char}` / `T{char}` | To just before, just after it |
| `;` / `,` | The last `f`/`F`/`t`/`T` again, the same way or the other |
| `gg` / `G` | The first, last character of the document, across pages |
| `{count}gg` / `{count}G` | The start of that page |
| `H` / `M` / `L` | The line nearest the top, middle, bottom of the pane, without scrolling |
| `zt` / `zz` / `zb` | Scroll so the cursor's own line lands at the top, middle, bottom |
| `v` / `V` | Select by character, by line; the same key again drops the selection |
| `o` | Put the cursor on the other end of the selection |
| `y` (with a selection) | Copy it |
| `y{motion}` | Copy exactly what the motion spans: `yw`, `ye`, `y$`, `y}`, `yfx` |
| `yy` / `Y` | Copy whole lines from the cursor's own |
| `/{text}` / `?{text}`, `n` / `N` | Search, and walk the matches |
| `y/{text}` | Copy from the cursor up to what the search finds |
| `{count}` before any of them | Repeat it that many times |

The motions divide into inclusive and exclusive the way vim's do, which is the difference between `yw` and `ye` over the same word: both land in the same place, but `ye` takes the last letter and `yw` stops before the next word starts. The window keys keep working with a cursor in the text, so `Space`, `C-d`, `]` and the rest still page through the document the cursor is sitting in.

A PDF carries no structure to ask about any of this, so the text is modelled rather than read. The page is cut into blocks along the empty bands that run right across it: a band down the page is the gutter of a two-column paper, so the motions read down one column and then down the other (a page number set under the gutter does not close it, and a review copy's line numbers down the margins join the column beside them), and a band across it separates a banner or a footnote from the body, so a title reads before the columns under it rather than in the middle of them. Within a block, glyph runs are clustered into lines by how close their baselines are, a gap wide enough to have been a space becomes one, and a paragraph break is a line gap noticeably wider than that page's usual. Where a document's layout defeats the model, the motions follow the model rather than the page.

Search is Winter's one convention that holds whichever way a document is being read: `/` and `?` ask for text in the find box at the pane's top-right, and `n` and `N` walk the matches, wrapping around the document once. A pattern typed in lower case matches either case and a capital in one matches exactly, which is the rule the text editor searches its files by. A search made with no cursor in the text puts one on what it found, with the whole match drawn under it, since a page that moved with nothing marking what matched would be half an answer. It searches as you type, each letter from where the prompt opened, and the status bar counts the matches in the whole document (`[2/18]`); `Esc` at the prompt puts the cursor and the last search back as they were.

**Image** opens wherever a file does, on a PNG, JPEG, GIF, WebP, BMP, ICO, or SVG file, and draws it fitted to the pane, never past its own size. Winter draws it itself, on the GPU like an image printed in a terminal, so the picker, menus, key hints, and notifications all sit over it as they do over anything else. `+` and `-` zoom about the middle of the pane, `=` fits the image again, `1` shows it at its own size, and `h`/`j`/`k`/`l`, `SPC`, `b`, `C-d`, `C-u`, and the wheel scroll round one zoomed past the pane, stopping where its edge meets the pane's. The status bar shows the image's size and its scale (`640×480  fit 42%`), and `C-g` says the same. A GIF shows its first frame. An SVG is drawn fitted up to the pane, and drawn again at each zoom, so it stays sharp; it is markup as well as a picture, and Open With opens that in the text editor.

**Hex** shows a file's bytes, sixteen to a row, as an offset, the bytes, and the characters they read as. It opens on a file the editor refuses as binary, and from Open With on any file. `h` and `l` step a byte, the Vim keys move by rows, `g o` or `M-g M-c` goes to an offset (decimal or `0x` hex), and `C-x C-q` turns editing on: a hex digit writes the half of the byte under the cursor, `u` takes the last byte written back, and `C-s` or `w` writes the file.

**Open With** (`C-M-S-e`, `Open: This File With...`) opens the file the focused tab shows in another editor that can show it, as a tab beside it: an SVG drawn by the image viewer opened up as its markup, or one being edited drawn again. The list holds only the editors that can show that file; one shown by no other says so instead. Where a tab of the chosen editor in the same pane already has the file open, it goes to that tab instead of opening a second one, and a buried buffer holding the file comes back. The viewers read the local disk alone, so a remote file is offered to the text editor only.

**Keys** (`Keys View: Show Every Command`) lists every command and the chord bound to it, read from the keymap in force, so it cannot disagree with what the keys actually do. `/`, `n`, and `N` search it, over both columns, so a chord you pressed by accident is as easy to look up as a command you are hunting for. `Enter` runs the command under the cursor: the editor hands the pane back first, so a command that opens an editor of its own has somewhere to open.

## Workspaces

A workspace is a project the window can be switched to and back from, finding it as it was left: its panes, its tabs, and each terminal's directory. It is Winter's own record, kept in `~/.local/state/winter-term/workspaces/` (`%LOCALAPPDATA%\winter-term\workspaces` on Windows), one file each; nothing is ever written into the project's folder. Every command is in the picker:

- `Workspace: Mark Folder as Workspace` makes the focused tab's project a workspace, offering the folder's name for it, and the window is in it from then on. A project already in one says which.
- `Workspace: Switch` lists the workspaces, the window's own marked `(current)`, and `Enter` goes to one.
- `Workspace: Add Folder` adds the focused tab's project to the window's workspace, as another of its folders.
- `Workspace: Rename` renames the window's workspace.
- `Workspace: Remove` lists the workspaces to forget one; its folders are left as they are, and its tabs close if it was open.

A switch puts the workspace left aside rather than closing it: its tabs keep running out of sight, a build or an agent in one of its terminals carries on, and switching back finds everything as it was. The workspace switched to comes back the same way when it was put aside, else as it was last saved, else, never opened yet, as one terminal at its first folder. A layout in no workspace has nowhere to be kept, so leaving it closes it, asking first, as quitting does, when that would end a program running in a terminal or lose unsaved edits. A shell that exits in a workspace put aside closes its tab there; with nothing left, that workspace opens as last saved next time.

Quitting saves every workspace open in the window, and asks first about the programs running in any of them. A reload keeps the programs of the workspace in front running, as it always has; those of the workspaces put aside end with it, which is asked about first. A restart that restores the session comes back into the workspace the window was in. A window opened in a given directory (a file manager's "Open Terminal Here", or a new window) starts fresh there, in no workspace.

The same commands are there from a shell, as `winter workspace`, acting on the window the shell runs in (Winter's panes name it in `WINTER_SOCKET`; elsewhere it is the first window launched) and printing what the window says:

| Command | What it does |
|---|---|
| `winter workspace add [dir] [--name N]` | Make the project of `dir` (the current directory by default) a workspace, called `N` or by its folder's name |
| `winter workspace add-folder [dir]` | Add the project of `dir` to the window's workspace |
| `winter workspace switch <name>` | Switch the window to a workspace |
| `winter workspace rename <name>` | Rename the window's workspace |
| `winter workspace remove <name>` | Forget a workspace |
| `winter workspace list` | List the workspaces and their folders, the window's marked `*` |

A workspace has settings of its own over `settings.kdl`, as VS Code's workspace settings sit over the user's. In a window with a workspace the settings page shows two scopes in its header, `User` and `Workspace: <name>`, and `Tab` goes from one to the other. An edit in the Workspace scope is in force while that workspace is the window's and is kept in its file, never in `settings.kdl`; the rows it sets are marked with a dot, and `S-Del` on one drops it, putting the user's value back. An edit in the User scope goes to `settings.kdl` as before; where the workspace sets the same row, its own value stays in force. Switching workspaces puts the next one's settings in force in place of the last one's.

## Configuration

Config lives in `~/.config/winter-term/` (`%APPDATA%\winter-term` on Windows), split in two:

- `settings.kdl` for appearance and behaviour
- `keybindings.kdl` for the `window` and `editing` chords, and each editor's own block (`git`)

Both are written in [KDL](https://kdl.dev/). Winter installs a reference copy of each, and the shipped copies in `crates/winter-term/builtin/` are the real defaults rather than examples, read before yours: anything you do not mention keeps its default, anything you do mention replaces it.

Changes apply on save, without a restart. `winter --reload` asks a running instance to reload as well.

### Updating Winter without losing your session

After installing a new binary, run `winter --reload` (or the `reload` command in the picker) in a running window. On Unix (tested on Linux) the window replaces itself with the new binary in place: every program running in a terminal pane (Claude Code, an editor, a build, a remote shell) keeps running with its process id and its pty, the layout and focus come back, and each pane's screen and scrollback are rebuilt from a copy taken at the moment of the reload. Each program is then asked to redraw by a resize, so full-screen programs repaint themselves. What does not come back: editor tabs other than Dir, Git, Grep, and the text editor, block history and folds, and the text of anything a program wrote in the instant between the copy and the replacement. If the new binary cannot be started, the window stays as it is and says so.

The window you are running must already contain this feature: a window started from an older build still reloads by starting fresh shells, so restart it once. Windows keeps the older behavior, since its pseudo-terminals cannot be handed to another process image.

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
| `gutter-side` | `"left"` (default) or `"right"`: which edge of a listing the gutter of git states and marks stands at |
| `window-controls-side` | Which side the window buttons sit on |
| `pane-border-width` | Divider thickness between panes |
| `popup-input-height` | How tall the field typed into is in every popup (the picker and every picker, the find box, a question), in rows of the font: `1.5` by default, `1` to `4` |
| `dim-inactive` | Dim panes that are not focused |
| `url-underline` | Underline detected URLs and OSC 8 links |
| `palette-match-underline` | Underline matched characters in the picker |
| `paste-on-right-click` | Right click pastes instead of opening a menu |
| `restore-session` | Reopen the previous layout on launch (off by default), with its editor tabs (Dir, Git, Grep, text, PDF, image, hex, launcher) where they were looking |
| `scroll-margin` | Screen rows an editor keeps above and below the cursor as it scrolls, like Vim's `scrolloff` (5 by default, 0 for none, held under half the pane) |
| `scrollback-lines` | Scrollback ceiling |
| `shell` | Shell to spawn, instead of the system default |
| `shell-linux` / `shell-macos` / `shell-windows` | Per-platform override of `shell` |
| `window-title-template` | Window title: `"{{ name }} — Winter Term"` by default (`main.rs — Winter Term`); also `{{ location }}`, `{{ path }}`, `{{ title }}`, `{{ app_name }}`, `{{ pane_title }}`, and `{{ cwd }}`, a part left empty dropping out with its separator |
| `rainbow-parens` | Color brackets by nesting depth |
| `sentence-highlight` | Alternating bands over sentences, as a reading aid |
| `wrap-indent` | Hang the continuation of a soft-wrapped line under its start. Applied to the scrollback when a width change settles; the visible screen always wraps at the margin, where programs expect it |
| `wrap-words` | Break soft wraps at word boundaries instead of mid-word. Applied to the scrollback when a width change settles, like `wrap-indent` |
| `prompt-edit-bindings` | `"emacs"` (default) or `"none"`, see above |
| `cursor { ... }` | `blink`, `hide-in-inactive`, plus the shape per mode: `insert`, `normal`, `visual` |
| `status-bar { ... }` | `show` (default on), `show-mode` (default on), `segments` (the right side's segments in order, from `antigravity`, `claude`, `zai`, `tool` (the focused editor's); default all, in that order; a bare `segments` shows none), `priority` (the segments in the order they hold on when the bar is short of room, the first kept longest; one left out goes before any listed; default none, which drops from the left) |
| `clipboard-read` | Let programs read the clipboard through OSC 52 |

Every setting above is also editable in the settings page (`C-,`), which writes the same file: a colour table and the keybindings are the two that need the file or their own page instead of a row.

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

Without it Winter still works, but the whole session is one rolling block: no per-command boundaries, no exit-code tags, no folding. The marks also sharpen two behaviors elsewhere: with them, Winter knows exactly when a command owns the pane - a REPL or an agent CLI running on the primary screen is sent `Esc` from Normal mode, and a fresh prompt stops that the moment the shell does - and where the editable command line begins, so a Vim operator aimed at the shell's own `❯ ` decoration is refused ("that is the shell's own prompt, not the command line") instead of mistranslating into readline chords that edit the wrong characters.

## Rich blocks

Programs can emit typed, MIME-tagged content that Winter draws inline: images, SVG, charts, Markdown, HTML, tables, and JSON. Every block carries a `text/plain` fallback, so the same program stays readable under `tmux`, `ssh`, or in CI.

Winter draws every block itself, on the GPU, from the richest representation in it that it can draw:

| Representation | Drawn as |
|---|---|
| `image/png`, `image/jpeg`, `image/gif`, `image/webp` | The image, scaled down to the band its rows reserve |
| `image/svg+xml` | The drawing, drawn again at the size it is shown, so it stays sharp |
| `application/vnd.vega-lite+json` | A chart: one `bar`, `line`, `area`, or `point` mark over inline `data.values`, placed by `x` and `y`, split into stacked or separate series by `color`, with axes, a legend, and a title |
| `text/html` | Rich text: headings, emphasis, lists, quotes, links, code, and tables in aligned columns; scripts and styles are dropped, and nothing in it runs |
| `text/markdown` | Rich text, the same way |
| `text/csv`, `application/json` | A table, and pretty-printed JSON, in the terminal's font |
| `text/plain` | The text |

A chart Winter does not draw (layers, facets, transforms, data fetched from a URL, or a full Vega spec) falls back to the next representation the block carries, which in the end is its text. Nothing in a block is ever run or fetched, so a block arriving from a downloaded file, a pipe, or a host behind `ssh` can show nothing but its content: the trust tier a block asks for is accepted and has no effect.

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

A session name may hold letters, digits, `-`, `.`, and `_`, and must not start with `-`. Sessions are reachable over a Unix socket locally, or over an SSH tunnel to a remote server. PTY geometry is arbitrated at the smallest attached client, so a larger pane letterboxes rather than rendering a stream wrapped for the wrong width.
