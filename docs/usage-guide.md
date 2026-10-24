# Winter usage guide

How to drive Winter day to day: the modes, the full keymap, and the settings that change them.

For what Winter is and how to install it, see the [README](../README.md). For how it is built, see [`architecture.md`](architecture.md).

## Modes

A window is split into panes, each a strip of tabs, and every tab is a terminal or an editor. A terminal tab is always in exactly one of three modes, and each keeps its own; an editor tab is in none, since it owns the keys it binds. The status bar is the window's, not the terminal's: while an editor has the keyboard it shows the editor's own mode where it has one (the text editor's Insert or Normal, a PDF's caret or selection, a listing whose names are being edited) and `P` where it has none, what the editor has to report, and at its right edge where the cursor is, such as `12:5  340 lines` in the text editor or `page 3 / 18` in a PDF. The status bar is shown by default (`status-bar { show #false }` hides it and frees its row), and the mode indicator is enabled by default (`status-bar { show-mode #false }` hides it).

The status bar's left edge says where the focused terminal's commands run: a desktop glyph for this machine, the remote glyph and the host's name while `ssh` (or a remote mux session) is in the foreground, and the Docker whale and the container's name while `docker exec`, `docker compose exec`, or `podman exec` is. Its right edge ends in a bell.

Before the right edge, the status bar shows the AI quotas there are to show, each under its logo as its short window and its weekly window, used and resetting at (`42% 14:05 | 10% Mon 09:00`, in local time): Antigravity's, Claude Code's, and Z.ai's. Each is asked for the way its own editors ask, so nothing needs setting up beyond them. Claude Code's comes from Anthropic's usage endpoint with the token Claude Code signed in with (the macOS keychain, else `.credentials.json` in `~/.claude` or `$CLAUDE_CONFIG_DIR`). Antigravity's comes from the language server a running Antigravity editor starts, else from the statusline file the `agy` CLI writes while it runs. Z.ai's needs an API key: `Quota: Set Z.ai API Key` in the command palette asks for it without showing it and keeps it in the system's secret store (the Secret Service on Linux, the Keychain on macOS, the Credential Manager on Windows), never in the config; answering blank forgets it. Claude Code's and Z.ai's are asked every five minutes, Antigravity's every minute, and `Quota: Refresh` asks them all at once. A provider that cannot be asked (not signed in, not running, no key) is left off, and one whose asking fails on the way keeps what it showed last. The requests go through `curl`, with the tokens on its standard input rather than its command line, and nothing is asked while the status bar is hidden.

What the app and its editors report goes by how much it matters. A confirmation (`pushed`, `wrote 120 lines`) is said in the status bar after its mode for three seconds. A warning or an error appears as a card in the window's top-right corner, newest on top and at most three at once: an icon and colored edge for its kind (yellow warning, red error), the text wrapped to a few lines, and the editor that said it. A warning stays six seconds, an error eight; a card the pointer rests on stays until the pointer leaves, its `×` dismisses it, and a click anywhere else on it opens the history. `notifications { toasts #false }` says warnings and errors in the status bar too, and while the bar is hidden every notice, a confirmation included, is a card, having nowhere else to go. The same thing said again in a row counts on one card, `(3x)`, rather than stacking copies. Every notice is kept for the session: an error or a warning not yet looked at dots the bell, counts on it, and colors it. Clicking the bell, `C-M-m`, or `Notifications: Show History` in the palette opens them in a Notifications tab beside the focused one (or goes to the one the pane has), newest first, each led by an icon for its kind (red error, yellow warning) with its source and text at the left and its age at the right edge, counts them as read, and takes the cards down. The tab stays current as new notices come in; `y` or `Enter` copies the message under the cursor, `/` finds one, `C` clears the history, and `q` closes the tab. `Notifications: Clear` forgets them all from anywhere.

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
| `PageUp` `PageDown` | Page up, page down |
| `C-u` `C-d` | Half page up, half page down |
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

The marks named with punctuation are kept for you rather than set by hand, the way Vim keeps its own. Marks and the jumplist are per pane. Registers are shared across every pane, so a yank in one pastes in another.

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
| `F8` / `S-<f8>` | Open the next, the previous file reference the last command printed |

`F8` reads the newest command output that holds a `path:line` or `path:line:col` reference to a file that exists (a compiler's errors, a test runner's failures, a linter's findings), resolved against the directory the command ran in, and opens each in turn in the text editor at its line, the way Emacs' `next-error` walks a compilation. It keeps reading the same terminal while the text editor has the focus, and starts over when newer output arrives.

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
| `M-S--` / `M-S-\` | Split horizontally, vertically |
| `M-S-=` | Zoom the focused pane (toggle) |
| `C-S-q` | Close the focused pane |
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
| `C-1` .. `C-9` | Go to tab N |
| `C-S-c` / `C-S-v` | Copy selection, paste |
| `C-,` | Open settings |
| `C-=` / `C--` / `C-0` | Font bigger, smaller, reset |
| `C-S-d` | Open Dir as a tab in the focused pane (toggle) |
| `C-S-g` | Open Git as a tab beside the work (toggle) |
| `C-S-u` | Put back the last thing closed: a tab, or a whole pane |
| `C-S-e` | Pick from every editor tab: the open ones, then the closed ones |
| `C-M-S-e` | Open the focused tab's file in another editor that can show it |
| `M-x` | Command palette |
| `C-S-r` | History palette |
| `C-S-z` | Pane switcher (then press the digit shown on a pane) |
| `C-M-z` | Find a file in the project and open it (`C-z` still goes to the shell) |
| `M-S-z` | Find a file under the focused tab's working directory and open it |
| `C-M-g` | Search the project as you type, and open a line found |
| `C-S-<up>/Down` | Previous, next prompt block |
| `C-DEL` | Delete the word before the cursor |

A single-chord binding whose action is not one of the built-in window actions is looked up against the command palette instead, so `"M+q" "mux_new_session"` works.

## Editors

An editor is a tab Winter paints itself: Dir, Git, Grep, the text editor, a PDF, an image, Processes, System, and Keys. Each opens as a tab of its own in a pane's strip, a peer of the terminal tabs beside it, and the terminal it was opened from keeps running one tab away. Close an editor's tab with `C-S-q`. Pressing an editor's own chord while it is in front does nothing; from elsewhere in the pane it goes to the editor's tab. The window chords all still work from an editor: split, zoom, close, and `M-h/j/k/l` to move focus.

`C-M-z` (`File: Find in Project`) lists every file in the focused tab's project to fuzzy-find one by its path: the git repository the tab is looking into, with what git ignores left out, else the directory itself as `rg --files` lists it. `Enter` opens the file in the editor that shows it, and `C-M-z` again puts the list away. The window holds the chord, so it never reaches the shell, where it would arrive as `ESC C-z` and suspend the job the way `C-z` does; `C-z` itself still goes to the shell.

`M-S-z` (`File: Find File`) is the same picker kept to the focused tab's working directory: the files under the directory the terminal is in, or the one an editor is looking at, rather than the whole repository around it. `M-S-z` again puts it away.

`C-M-g` (`Search: Project as You Type`) searches the same project for what you type: once the typing pauses on two characters or more, ripgrep runs and the lines it finds stream into the list, each beside its file and line, with the title counting how many are shown of how many found. The pattern is searched for as it is, except that a `*` stands for anything; case is ignored unless the pattern has a capital in it. A ` @ ` after the pattern starts a file filter: `foo @ src/*.rs` keeps the lines of the files matching the glob, and `foo @ recent` those whose path holds `recent`, and changing only the filter narrows what was found without searching again. `Enter` opens the file on the line, `M-p` and `M-n` bring back earlier queries, and `Esc` or `C-M-g` again stops the search and puts it away.

`C-S-z` (`Go To Recent: Tabs, Files, Directories...`) opens one list of everything to go back to: the tabs open in every pane, then the files opened lately, then the directories, newest first within each, each with its icon. `Enter` goes to a tab, opens a file in the editor that shows it, or lists a directory in Dir. A file is remembered when an editor, the file browser, a click, or `$EDITOR` opens it, and a directory when Dir lists it or a shell has been in it; the files and directories carry over between sessions, and one that is gone is left out. The pane switcher that used to be on `C-S-z` is still `Go to Pane` in the command palette.

`C-S-u` (`Reopen Closed Tab`) puts back the last thing that was closed. For an editor, that is the editor itself coming back and not a fresh one of its kind: the text editor's buffers and the edits never written to them, where a listing's cursor was and what it had folded, the editor a document was turned to. It re-reads what it was showing on the way back.

For a pane, it is the pane: the split comes back where it was, with a shell in the directory the old one was in and every editor it was holding back in its strip. The shell itself cannot come back, since closing a pane ends the process; everything around it can. Pressing the key again reaches whatever was closed before that, up to the last eight.

`C-S-e` (`Go To or Reopen Tab...`) shows every editor tab as a list to pick from, in the fuzzy panel every other list in Winter is picked from: the ones open somewhere first, labelled with the tab they are in, then what was closed recently, newest first, each saying how long ago it was closed. Every row names the kind of editor, what it is showing, a `[+]` where it is holding edits that are on no disk, and the directory it is looking at, so typing `main` finds the text editor that was on `main.rs` and `win` finds the listing of a tree with that in its path. `Enter` on an open editor goes to it rather than opening a second one; `Enter` on a closed one puts it back and leaves the rest closed. `Esc` changes nothing.

`F1 h` (`Help: Keys of This Tab`) lists the keys of the tab in front, each beside what it does, in the same fuzzy panel: an editor's own keys for the state it is in (the Git status or the commit it is reading, a PDF with or without a cursor, Dir while renaming), then the motions every editor shares; over a terminal, every window command bound to a key. Typing filters on what the keys do, and `Esc` puts it away. Editors no longer spell their keys out in their header row: this list is where they are, and it is read from the same table each editor looks its keys up in, so it cannot name a key the editor does not answer.

Closing a pane or a tab with an editor holding unwritten edits asks first, naming what would go with it. Answering `y` closes it and keeps the editor anyway, so the edits are still a `C-S-u` away; anything else leaves everything as it was. An editor holding unsaved edits is never the entry the stash drops when it fills up, and the first close of a session says in the status bar which key brings it back.

Every editor asks the same way, in the middle of the window. A question with a known set of answers opens a picker: what the list is of on top, a filter line under it, and the matches below, narrowing as you type. `Up`/`Down` or `C-p`/`C-n` move through them, `Enter` takes the highlighted one, and `Esc` answers nothing. A question with no list to offer, or one whose answer is a name that does not exist yet, opens the same panel without the list: the question, the line it is answered on, and how to answer it. A question that cannot be undone is answered by a single key rather than a line, and says so instead of drawing a caret. Either way the question is where the eye already is, and the status bar is left to what it was saying.

Every editor answers to `M-S-,` and `M-S-.` for the top and the bottom of what it is showing, which are emacs' `M-<` and `M->` under the fingers, so an editor is read with either set of habits: the Vim `gg` and `G` reach the same two rows. The other Vim motions (`H` `M` `L`, `C-d` `C-u` `C-f` `C-b`) work in every editor too, and `C-r` re-reads or reruns whatever an editor is showing. The Git view still binds some of these keys to its own commands, after magit; it is the one exception.

Right-clicking inside an editor opens that editor's own menu, rather than the terminal's: over a listing there is nothing to paste into, and the entries that would mean something act on rows the terminal knows nothing about. The click moves the cursor to the row it landed on before the menu opens, so what is chosen acts on what was clicked. What the menu offers follows that row, because a commit cannot be staged and a section heading has no path to copy. Each entry is a key the editor already binds, or a few typed in order, shown by name with those keys at its right, so the menu is a way of finding a command and learning its keys rather than a second set of them. Entries come in groups set off by a line: reading or opening what was clicked, changing it, then copying what names it.

Every editor has one. Dir, Git, Grep, Processes, and System offer what the row under the pointer can have done to it. The text editor follows its mode: over a selection, Copy and Cut (through the clipboard register, so they reach other windows) and commenting; in Normal mode, Paste, Undo and Redo, commenting, Find, Save, and `$EDITOR`; in Insert mode only what types nothing, and a commit message or a rebase plan adds what finishes it. A right-click over a selection keeps it, so Copy has something to copy. The PDF viewer offers Copy over a selection (its right-click moves nothing), then Find, Go to Page, and the outline, the zoom, and inverting the colors; the image viewer offers its zoom and what the image is; Keys runs or copies the command under the pointer, finds one, or closes.

Every editor searches the same way, and so does a terminal in Normal mode. `/` (or `?`, back up the text) opens a small find box at the top-right corner of the pane being searched, where the text is typed with the same keys every field takes, while the pane stays lit so its matches show as you type; `Enter` keeps the search and `Esc` drops it. The status bar shows how far the search has got: the query and which match the cursor is on out of how many. `/` asks for text and moves the cursor to the next row holding it, `n` and `N` step through the rest of the matches and wrap at the ends, and a search that finds nothing says so instead of sitting still. Matching is literal, and a search typed in lower case matches either case while one typed with a capital in it matches exactly, the rule the text editor and the PDF viewer search by: a directory listing matches on the entry's name, every other editor on the whole row as it is painted, so a chord is as findable as the command beside it. Matches are lit the way a terminal's are, every one on screen in one color and the one the cursor is on in another, from the first letter typed, and they stay lit after `Enter` until `Esc` puts them away; `n` and `N` light them again. A PDF is the exception, since its pages are pictures rather than rows: it lights only the match the cursor lands on, though it counts them all. The status bar shows the query and, where the editor counts them, which match the cursor is on out of how many. A search moves the cursor and nothing else: marks, folds, and everything staged stay as they were. Grep is the one exception to the first part, since its rows already are a search: `/` there asks for a new one.

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

A directory's size is not something the filesystem knows, so `M-S-s` walks each one in the background, a directory at a time, showing `...` until a total arrives. Turning it off stops whatever is still walking. Totals are kept while you stay in the same directory and dropped when the root moves.

An operation acts on the marked entries, or on the entry under the cursor when nothing is marked, never both: the status bar shows how many are marked and what the last operation reported. A name typed into a prompt is a name, so `../elsewhere` is refused rather than reaching outside the listing, and nothing overwrites an existing entry. Copy and move take a new name for one target and a destination directory for several. When another pane on screen is showing a directory (a listing, or an editor rooted somewhere), it is offered first: the prompt for one target starts out holding its path, and the list for several puts it at the top. A whole path typed out in full is taken as a directory to copy or move into, under the entries' own names.

`x` sends what it removes to the desktop trash (the freedesktop.org one, in `~/.local/share/Trash`), with a record of where each entry came from, so a file manager can restore it; `X` deletes for good. Where there is no trash to send to, `x` says so rather than deleting.

Every row has a gutter, a narrow column at the left edge (or the right, with `gutter-side "right"`), holding icons: a check on an entry marked for an operation and, in a git checkout, each changed entry's state, the set magic-vscode draws: modified, added, deleted, renamed, untracked, in conflict, or ignored. A directory shows the most pressing change inside it, and what lies in an untracked or ignored directory shows that directory's state. Clean entries leave the gutter blank. The marks are read in the background whenever the listing is, so they follow edits and stages made anywhere. Moving the root clears the marks, since a mark held over would count toward an operation in a listing it was never part of.

`C-x C-q` turns every listed name into editable text, in the manner of Emacs' `wdired`, to rename many entries at once. The names take the text editor's keys: Vim in Normal and Visual mode, the Emacs chords while typing. Each name is a line that stands for its entry, so nothing adds or removes one: `dd` empties a name rather than pulling the ones below it up, and `o`, `J`, and a line-joining `C-k` do nothing. `ZZ`, `C-s`, or `Enter` while typing applies the differences as renames, refusing the whole batch if any name is not a name or two land on the same one, before anything moves. A batch of more than one rename is listed for a yes first. Swapping two names, or a longer cycle, goes through: entries waiting for a name another still holds are parked under a temporary one until it is free; `q` with nothing part-typed, `ZQ`, or a second `C-x C-q` throws them away. The same Emacs chords edit the answer in any prompt an editor asks, such as a search or a new name, and `Up` and `Down` (or `C-p` and `C-n`) bring back what that same question was answered with before, newest first, the way a shell's history does.

Directories sort before files whatever the key, and moving the root, toggling a view option, or folding keeps the cursor on the entry it was already on. A block cursor sits on the active row's first non-blank column and follows it as you move, so an editor shows where a selection would start before you ask for one. `v` starts selecting from there, for copying out of a listing, and `V` does the same by whole lines: `hjkl` and the arrows move and extend, `w` and `b` step words, `0`, `$`, `g` and `G` jump, `V` and `v` switch between whole lines and characters, a second press of the same one drops the selection, `y` or `Enter` copies and leaves, and `Esc` cancels. While selecting it answers every key ahead of the editor, so the motions mean what they do in Vim rather than what the listing binds them to; chorded keys still reach the window, so splitting, zooming, and moving focus work from inside it. Dragging with the mouse selects the same rows.

Each entry carries an icon for what it is: bundled artwork keyed by extension, exact filename, or directory name, which needs nothing of the terminal font. The popups that list files, such as the file browser, the recent directories, and the text editor's open buffers, draw the same icons.

**Git** (`C-S-g`, or `Git: Status`) shows the working tree of the repository the pane's directory sits in, as foldable sections: the changes, then the stashes, then what the upstream has that you do not, then the recent commits. A file changed both in the index and in the working tree appears in both, which is what lets one half be staged without the other. Every command runs from the repository root, off the event-loop thread, and the view re-reads the tree after anything that changed it; what a command reports, a failure included, shows in the status bar rather than being swallowed.

| Key | Action |
|---|---|
| `j` `k` or `Down` `Up` | Move down, up |
| `M-n` / `M-p` | Next, previous entry, skipping blank lines |
| `g` | Jump: `t` untracked, `u` unstaged, `s` staged, `r` recent, `g` top, `j`/`k` next, previous entry |
| `/` | Search the view, or the log or listing filling it |
| `n` / `N` | Next, previous match |
| `Tab` / `S-TAB` | Fold the section, fold everything |
| `Enter` | Open the file under the cursor in the text editor; from a hunk, at the line it changes |
| `C-o` | Open it in `$EDITOR` instead, in a new tab |
| `Enter` on a commit | Read the commit: message, then the files it changed, each opening to its patch |
| `Enter` in that patch | Open the file the row belongs to, at the line its hunk changes |
| `Enter` on a stash | Read the stash as a diff |
| `.` | File: what can be done with the file under the cursor |
| `Tab` on a file | Show its diff, hunk by hunk |
| `s` / `S` | Stage the target, stage everything |
| `u` / `U` | Unstage the target, unstage everything |
| `x` | Discard the target, after confirming |
| `a` / `-` | Apply, reverse the hunk under the cursor |
| `y` | Copy the hash or path under the cursor |
| `P` | Push menu, as magit's: `p` to the push remote and `u` to the upstream (each set first where it is not), `e` elsewhere, `o` another branch or commit, `T` a tag, `t` every tag; `-` then `f` `F` `i` `h` `d` toggles force-with-lease, force, force-if-includes, no hooks, dry run, and `RET` goes back |
| `F` / `f` | Pull with rebase, fetch all |
| `C-S-p` | Quick push: the current branch to the remote it tracks (else `origin`), setting it to track, with no questions |
| `C-S-M-f` | Quick pull with rebase: from the branch the current one tracks, else the same name on `origin`, with no questions |
| `G` | Re-read the working tree |
| `+` | Show more of the recent commits, 50 at a time, while the list runs on |
| `C-S-c` | Commit what is staged, writing the message in the commit editor |
| `C-S-s` | Stage all: untracked files when the cursor is in that section, tracked changes otherwise |
| `C-S-a` | Stage every untracked file, wherever the cursor is |
| `!` | Run any git command |
| `M-b` | Blame the file under the cursor |
| `M-y` | Show every ref |
| `M-g r` | Open the remote in a browser (rebindable in the `git` block of `keybindings.kdl`) |
| `C-g` | Stop the git command running (a push, a fetch, a hook), with everything it started; git commands have no time limit. Works in any editor tab with work running |
| `C-SPC` | Set the mark on a diff line, or clear it |
| `M-s` / `M-u` | Stage, unstage only some lines: from the mark to the cursor, or the line under the cursor |

The view is read by color as much as by name, the way magic-vscode's own git view is: a section heading wears the hue of what sits under it (cyan staged, yellow unstaged, green untracked, red contested), every other heading and the header block's own labels are blue, a file says what changed to it in that change's own hue, a branch is magenta and one on a remote green, a tag cyan, and anything the upstream has not seen yet — the commits ahead of it and the count saying how many — is red.

When git is part-way through something — a merge, a rebase, a cherry-pick, a revert, a bisect — a `State:` line in the header says so, what it is working on, how far along it is where git counts the steps, and the keys that finish it. Nothing in a porcelain status reports any of that, so without the line the only sign of an open rebase is a section of conflicts and no reason for them. It is read from the files git writes it in, under the repository's own directory.

Closing a tab, a pane, the other tabs or panes, or the whole window asks first when a terminal has a program running in it (an agent such as Claude Code, an editor, a build; a shell sitting at its prompt does not count), and when an editor tab holds edits that are on no disk. Answer `y` to close; any other key keeps things as they are. Naming the running program uses the process list, so on platforms without `/proc` the question says "a program" instead, and a program that does not own the screen may not be noticed there.

With more than one pane, the status view and the commit editor open in another pane rather than over the one you are working in: the one beside it in the same row (to its right first, else its left) if there is one, otherwise the next in order, and asking for the status view again goes to the one already open there. With a single pane, one is split off to its right for them.

The status view watches the repository through the platform's file notifications and reads git's status again three seconds after the changes go quiet, redrawing only when the status differs: an edit, a stage from another terminal, a commit, a fetch. Build output and dependency directories at the top of the tree (`target`, `node_modules`, `dist`, `build`, and the like) are not watched, and inside `.git` only the index, `HEAD`, the references and the merge state count. Where the platform cannot be watched, it reads on a three second timer instead. A log, a diff, or any other output view is left alone while you read it, and an open file diff is re-read on each pass since the status cannot say that a file changed again.

A triangle opens every row a fold key acts on, pointing down while what hangs off it shows and right while it is hidden: section headings, file rows, and a commit's hunk headers all carry one. A hunk in the status view does not, since `Tab` there folds the file it belongs to rather than the hunk itself. Opening a row brings what it opened into view: a block that would run past the pane's bottom pulls its own row to the top, where the most of it fits, while one that already shows in full leaves the editor where it was.

On a section heading, `s`, `u`, and `x` act on every file in that section. On a hunk, or on any line inside it, they act on that hunk alone: the patch is built from the hunk and fed to `git apply` on standard input, so the rest of the file is untouched. `M-s` and `M-u` go finer, to single lines: the changes left out are written so the patch still applies (an addition not chosen is dropped, a removal not chosen stays as context), and the header's counts are worked out again.

On a file in conflict, `.` then `o` or `t` settles it by taking one side whole, ours or theirs, and stages the result, which is what marks the conflict resolved. Discarding an untracked file deletes it, since `git restore` cannot reach a path that is not in the index.

A command that takes a name asks for it as a list rather than a blank line: checking out, merging or rebasing onto a branch offers every branch, deleting one offers the local branches, deleting a tag offers the tags, and removing or pruning a remote offers the remotes. Where there is nothing to list, the question falls back to being typed.

Every key that waits for a second one names what the second key may be, on the same card the terminal's own key hints use: centred over the window, above whatever is under it, so it cannot be pushed off the bottom by a long listing. It shows at once rather than after a pause, since the menus are how the editors are learned. Any key the menu does not offer closes it without doing anything, which is what makes a mistyped second key harmless.

These keys open a menu, and the next key picks from it:

| Key | Menu |
|---|---|
| `b` | Branch: checkout, create, delete |
| `c` | Commit: in an editor, amend, amend keeping the message, reword the last message (`c w`, leaving anything staged out of it) |
| `d` | Diff: working tree, staged, against a revision |
| `i` | Ignore: this path, this extension |
| `m` | Merge: merge, continue, abort |
| `r` | Rebase: interactive from the commit under the cursor (that commit and every one after it; off a commit, onto a branch picked from a list), onto upstream, continue, skip, abort |
| `t` | Tag: create, delete, list |
| `z` | Stash: push, pop, apply, drop, list |
| `A` | Cherry-pick: pick, continue, abort |
| `B` | Bisect: start, good, bad, reset |
| `M` | Remote: list, add, remove, prune |
| `O` | Reset: mixed, soft, hard |
| `V` | Revert: revert, continue, abort |
| `Z` | Worktree: list, add, remove |
| `C-l` | Log: this branch, all refs, this file |

Anything that wants a terminal of its own goes to a new tab: commit and amend in `$EDITOR`, and interactive rebase with its todo list. `o` resets to the commit under the cursor, or asks which one when the cursor is elsewhere.

A log, a listing, a blame, or a diff fills the view; `j`/`k` move through it, `/` searches it, `+` asks for more of a log, `y` copies the first field of a line, which is the commit hash in a log or a blame, and `q` goes back to the status. In a blame, where every row is a line of one file, `Enter` opens that file at the row under the cursor and `C-o` opens it in `$EDITOR` there.

A commit read with `Enter` opens shut, the way the status view does: the summary, then a `Changes` heading counting the files it touched, then one band per file, each counting the hunks it holds, each saying in words what the commit did to it (`modified`, `new file`, `deleted`, `renamed`) after its fold triangle and before its icon and path, with the patch waiting behind them. It folds the same way too. `Tab` on the heading takes the whole file list away and brings it back, `Tab` on a file band opens it to its hunk headers and shuts it again, `Tab` on a hunk opens that hunk's body, and `S-TAB` acts on the whole commit at once, opening every file and hunk of it, or shutting all of them when none is folded.

**Grep** (`C-S-s`, or `Grep: Search Files`) finds the lines under the pane's working directory that hold some text, grouped under one heading per file.

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

Where [ripgrep](https://github.com/BurntSushi/ripgrep) is installed the search runs through it, which reads the tree's `.gitignore` files and is faster on a large tree; otherwise Winter walks the tree itself. Either way the rules are the same: literal text in any case, hidden files read, and version-control and build directories skipped.

`C-x C-q` turns the matched lines into editable text, as Emacs' `wgrep` does, with the text editor's keys over them. `ZZ`, `C-s`, or `Enter` while typing writes every changed line back to its file, each file read and written once through the text editor's own save, so line endings and permissions survive. A line whose file no longer says what the search found there (changed since, or cut short in the list) is left alone and counted in the report, rather than written over. `ZQ`, or `q` with nothing part-typed, throws the edits away.

The walk runs off the event loop, so the pane stays live while a large tree is read, and `Esc` abandons it. `/` here asks for a new search rather than searching the rows, since the rows already are one, and `n` and `N` step through what it found. Matching is literal text, whatever the case.

The walk skips what would swamp the results rather than reading everything: `.git`, `.hg`, `.svn`, `node_modules`, and `target`, files over a megabyte, anything that does not read as text, and symlinks, which are never followed. It stops at 500 matches and says so in the header, because a query loose enough to pass that is one to narrow.

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

**System** (`System: Monitor` in the command palette; it has no default chord) shows the machine as a whole, refreshed every two seconds: host, operating system, uptime and load average; processor use as a total and a grid of cores with their clocks; memory and swap; each NVIDIA GPU's video memory, load and temperature; and the local disks with a bar for each. Bars turn yellow at 75 percent and red at 90. Linux reads `/proc` and `df`, macOS and the BSDs read `sysctl`, `vm_stat`, `ps` and `df`, and Windows asks PowerShell. macOS has no per-core counter without a system library, so it shows the total only, worked out from what the processes are using. The GPU section needs `nvidia-smi` on the path and is left out otherwise.

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
| `]b` / `[b` | The next open buffer, the one before, in the order they were opened |
| `C-M-[` / `C-M-]` | Back through the buffers you last worked on, and forward again |
| `gb` | List the open buffers, most recently used first, and go to one |
| `C-/` | Comment the line, or the selected lines, out; or back in when they all are |
| `M-/` (typing) | Complete the word from the words around it, then the other buffers; again for the next |

One text editor holds as many files as you open into it. Opening a file while the text editor is up (from the file browser, or `gx` on a path) adds it as another buffer rather than covering the text editor with a second one, and opening a file that is already open goes to it. `]b` and `[b` step through them in the order they were opened, and the status bar says which of how many you are on (`2 of 3`). `C-M-[` and `C-M-]` walk them by how recently you worked on them instead, which is usually what you want when two files are in play: one press of `C-M-[` goes back to the file you just left, and pressing it again goes further back rather than bouncing between the two. Walking over a file does not make it the recent one, so the order stays put until you stop and work in it. `gb` lists them in that same order, the file in front at the top and the one before it a row below, and it works in any of the text editor's modes. Closing the text editor's tab closes every buffer in it, and asks first about any with unsaved edits, so nothing is dropped quietly.


### Commit messages

In a Winter pane, `git commit` edits its message in the built-in text editor instead of `$EDITOR`: Winter sets `GIT_EDITOR` to `winter --edit-wait`, which opens `COMMIT_EDITMSG` in a new text editor tab, in Insert mode, and holds git until that tab closes. The staged diff is a second tab in the same pane, one tab away, and goes when the text editor does. `C-c C-c` (or `ZZ`) saves and commits; `C-c C-k` (or `ZQ`, or discarding when asked) empties the message so git stops with "Aborting commit due to empty commit message". The header lists the two chords. `C-M-p` swaps the message above git's `#` comment lines for an earlier one from the repository's history (the last ten distinct messages), going further back each press, and `C-M-n` comes forward again to what you had typed. With no running window to ask, `winter --edit-wait` runs `$VISUAL` / `$EDITOR` instead, so git is never left without an editor.

An interactive rebase's plan (`git-rebase-todo`) opens the same way, as text the usual keys edit, with keys for the plan: in Normal mode `p`, `r`, `e`, `s`, `f`, or `d` alone (or after `C-c`, in any mode) sets the line's action to pick, reword, edit, squash, fixup, or drop and steps on to the next line, standing in for the Vim keys of those letters in this file only, `C-c j` and `C-c k` move the line down and up, `C-c C-c` (or `ZZ`) finishes, and `C-c C-k` empties the plan so git aborts the rebase. A plan git would refuse, one whose first commit squashes or fixes up into nothing, is not finished but said what is wrong with.

What the buffers share is what Vim shares: the registers, the last search, the last char search, and the last change `.` repeats. What belongs to each file stays with it: its marks, its undo history, where the cursor and the window are in it, and what it looked like on disk.

A count typed before a command applies to it, as in Vim: `3x`, `2dd`, `5j`, `2d3w`. Insert mode is an Emacs-style field. Moving: `C-a`, `C-e`, `C-b`, `C-f`, `C-n`, `C-p`, `M-b` and `M-f` by word, `M-m` to the first non-blank, `C-v` and `M-v` by page, `M-<` and `M->` to the start and end, and `C-l` to centre the cursor. Deleting: `C-d`, `C-h`, `M-d` and `M-DEL` (or `C-DEL`) by word, `C-k` to the end of the line (at the end, the line break), `C-u` to the start, and `C-w` back a word. What the kills take goes on a kill ring, as in Emacs: kills made one straight after another join into one entry, so `C-k C-k` takes a whole line and `C-y` brings it back whole, and `M-y` straight after a `C-y` swaps what it put in for the kill before. Deletes and yanks made in Normal mode go on the ring as well, so `C-y` reaches them too. Editing: `C-t` and `M-t` transpose characters and words, `M-u` and `M-c` upper-case and capitalise the word ahead, and `C-o` opens a line and stays put. A word here is a run of letters and digits. The `M-` chords the window owns (`M-h` `j` `k` `l`, the digits, `M-x`) are left to it, so there is no `M-l` for lower-casing.

Every input field takes the same keys: a dialog's answer (an editor's question, the sudo password, the Z.ai key, a theme name), the palette's query, a tab's new name, a settings text row, and the find box a `/` search is typed in. A field holds the keyboard while it is up, so it adds the keys a file leaves to the window: `C-g` cancels as `Esc` does, `C-/` and `C-_` undo (a word at a time while typing), `C-\` redoes, `M-l` lower-cases the word ahead, and `M-\` deletes the spaces around the caret. `C-S-v` pastes into whichever field is up. The palette keeps its own keys for its list: `C-n` and `C-p` move the selection, `M-p` and `M-n` recall earlier queries, and in the file browser `Left` and `Right` go up and into directories, so `C-b` and `C-f` move the caret there.

Selecting with `v` or `V` puts the operators on what is selected rather than on a motion: `d`, `c`, `y`, `>`, `<`, `J`, `~`, `r`, and `p` all act on the selection and end it, `i` and `a` grow it to a text object, and `o` moves to its other end so it can be grown from either side. `Esc` drops it.

The mouse moves the cursor too: a click puts it on the character under the pointer, dragging selects from where the button went down, and the wheel scrolls, carrying the cursor with it so the two never part company.

Registers work as Vim's do. `"a` through `"z` name one, and `"A` through `"Z` append to it. With no register named, a yank also goes to `"0`, a delete of whole lines or across lines to `"1` (the older ones shifting up to `"9`), and a smaller delete to `"-`. `"_` is the black hole, which takes nothing, not even into the unnamed register, so `"_dd` deletes a line without losing what `p` would put back. `"+` and `"*` are the system clipboard, so `"+yy` copies a line out of Winter and `"+p` puts back whatever was copied anywhere else.

Searching is literal text rather than a pattern, and a search typed in lower case matches either case, while one typed with a capital in it matches exactly. `*` and `#` look for the word under the cursor without typing it out.

It colors what it is reading with the TextMate grammars VS Code and Sublime Text read with, over 200 of them: every common programming language, LaTeX and BibTeX, Markdown (code in a fenced block in its own language), diffs, commit messages and rebase plans, and the configuration formats (JSON, YAML, TOML, KDL, INI, `.env`, Nix, Terraform, Protocol Buffers, Dockerfile, Makefile, CMake, and the git dotfiles). The grammar is chosen by the file's name (`Makefile`, `.bashrc`, `COMMIT_EDITMSG`), else its extension, else its first line, so a script with a shebang and no extension is colored too. The dark preset colors each kind of thing as magic-vscode's One Color theme does: functions, types, macros, attributes, constants and members each in a hue of their own, and in LaTeX each section level, environment names, math, labels, citations, and escapes. The light preset keeps the same hues, each darkened until it reads on the light background. Any of them can be recolored by its TextMate scope in the `colors` block (`syntax { entity.name.function "#bf73df" }`; `comment`, `string`, `keyword`, `number`, and `type` still work); `Text Editor: Show Scopes at Cursor` in the palette says which scopes the character under the cursor is in and which of them color it, and a name in the block that no scope goes by is reported when the settings are read rather than coloring nothing. A theme that sets no syntax colors colors comments, strings, numbers, keywords, and types from its own palette. A commit message's conventional type (`feat:`, `fix(ui)!:`) is colored as a keyword, and its summary is marked past 72 characters. A file in a language it does not know is painted as the plain text it may well be rather than guessed at, a long file is colored from the top down over a few frames rather than holding the window while it is read, and a line over 20,000 characters (a minified script, a data dump) is left plain, as VS Code leaves it. It colors what the characters themselves say and never decides that something is wrong with the code.

If the file changes on disk while it is open with nothing unsaved in it, it is read again the next time the text editor comes back to the front, so what is on screen is what is on disk. With unsaved edits it leaves your work alone and asks before overwriting at save time.

Past that it edits text and nothing more: completion is only the words already in the open files, and there is no language server. That is what `C-o` is for, and it saves before handing over, so `$EDITOR` opens what you were looking at rather than what was last written. A file it cannot open as text says so and stays closed: anything holding a NUL byte, anything that is not UTF-8, and anything over four megabytes, all of which it would otherwise write back as something the original was not. A UTF-8 byte order mark at the head of a file is kept aside rather than shown, and written back on save.

Saving writes through a temporary file beside the original and renames it over the top, so an interrupted write leaves the previous contents whole rather than a truncated file. Undoing back to what was last written leaves nothing to save, so closing does not ask about edits that are no longer there. A file's permission bits, its line endings, and whether it ended with a newline all survive the trip, so saving one edited line does not show up as a whole-file diff. If something else has written to the file since it was opened, saving asks before overwriting rather than throwing that away.

**File browser** (`C-S-f`, or `File: Browse Files`) walks the working directory in the command palette rather than in a pane, for the times you know roughly where a file is and want it open rather than listed.

It opens where the terminal points, when it points at a file or directory that exists: the Normal-mode cursor, else the mouse selection, else the shell's cursor (on a path, or just past one you typed), else the rightmost path on the last line printed. A file opens the browser on its directory with that file selected; a directory opens the browser inside it. Paths resolve against the shell's working directory, and `~`, `..`, and `path:line:col` suffixes are understood. With nothing found, or over an editor, it opens on the working directory as before.

| Key | Action |
|---|---|
| Typing | Filter this directory's entries |
| `/` after a name | Go into that directory and keep typing from there: `src/`, `../`, `~/`, or a filter like `sr/` that selects `src/`. `/` with nothing typed goes to the root |
| `Up` `Down` or `C-p` `C-n` | Move through what is showing |
| `Enter` / `Tab` / `Right` on a directory | Go into it: the browser stays up, rooted there |
| `Enter` / `Tab` on a file | Open it in the text editor, over the focused pane |
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

**PDF** opens wherever a file does, on any path ending in `.pdf`: `Enter` in Dir, Git, or Grep, the file browser, or a `gx` reference. Like the text editor it has no chord of its own and opens as a tab beside the editor it was reached from, so closing it puts the listing back. Winter draws the document itself, with a PDF renderer written in Rust and built into the binary along with the fonts a PDF may name without carrying, so a document opens with nothing installed and no network access. Its pages are drawn on a thread of their own, only those in view and one either side, so a long document opens at once and scrolling never waits: a page not drawn yet shows as a blank sheet until it is. The same thread reads every page's text while it has nothing to draw, so a search or a motion seldom waits on that either. Because the pages are drawn like everything else in the window, the palette, menus, key hints, and notifications sit over a document as they do over anything else. The viewer reads nothing but the file it was opened on, and an encrypted PDF opens if it needs no password.

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
| `+` / `-` | Zoom in, out |
| `=` | Fit the page to the pane's width again |
| `:` | Ask which page to go to |
| `O` | Open or close the outline |
| `I` | Invert the pages' colors, light text on dark, or put them back |
| `i` | Put a cursor in the text, for the motions below |
| `/{text}` / `?{text}` | Search down the document, back up it; the cursor lands on what is found |
| `n` / `N` | The same search again, the same way or the other |
| `Esc` | Abandon a half-typed command |

A toolbar takes the pane's top row, and the pages scroll under it. On the left it names the section being read, taken from the document's own outline (its bookmarks), and a click there opens the outline under the bar: folded to the top level but for the way down to that section, moved through with `j` and `k`, unfolded with `l` and folded with `h` (or `Tab`, or a click on a row's `▸`), and jumped to with `Enter` or a click, which closes it again; `Esc`, `q`, `O`, or a click on the pages closes it without moving. A document that lists no sections says so there instead. On the right are the zoom, `−` and `+` either side of the percentage, where a click asks for one: a number, or `w`, `h`, or `p` to fit the page's width, its height, or the whole page; then `‹` and `›` either side of the page number, where a click asks which page to go to; then `◐`, which inverts the pages' colors. Inverting draws the pages again, so the ones in view change as soon as they are drawn.

The motions are vim's, and they compose the way vim's do: a count is typed before the motion it multiplies and is spent by it, `g` and `z` wait for the key that completes them, and the pane's own key-hint card says what finishes one while it is pending. The header shows a count as it is typed, so one entered by accident is visible before it moves anything. The two places a count is read as a position rather than a repeat are `gg` and `G`, exactly as in vim: `5G` is page five, not five pages on.

Where the motions differ from vim's, it is because a document is not a buffer. There is no cursor to move, so `h` and `l` scroll sideways instead of stepping characters, and `C-d` moves half the pane rather than half a file's worth of lines. `zt`, `zz`, and `zb` move the window and not the page being read, which is what they do in vim too.

`i` puts a cursor in the document's own text, and from there the keys are vim's text motions rather than the window's. `Esc` takes it back out, one step at a time: a half-typed command first, then the selection, then the cursor itself.

The mouse selects text too. A click puts the cursor in the text where it lands, and a drag from there selects what it passes over and copies it to the clipboard as it goes, the status bar saying how many characters it holds. The selection is the same one `v` and `V` make, so `o` swaps its ends and the motions go on extending it from where the mouse left it; `y` copies it again once they have. Every copy from a PDF is announced by its length rather than echoed, since it may run to pages.

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

**Image** opens wherever a file does, on a PNG, JPEG, GIF, WebP, BMP, or ICO file, and draws it fitted to the pane, never past its own size. Winter draws it itself, on the GPU like an image printed in a terminal, so the palette, menus, key hints, and notifications all sit over it as they do over anything else. `+` and `-` zoom about the middle of the pane, `=` fits the image again, `1` shows it at its own size, and `h`/`j`/`k`/`l`, `SPC`, `b`, `C-d`, `C-u`, and the wheel scroll round one zoomed past the pane, stopping where its edge meets the pane's. The status bar shows the image's size and its scale (`640×480  fit 42%`), and `C-g` says the same. A GIF shows its first frame. An SVG is markup as well as a picture, so it opens in the text editor, and is drawn by opening it with the viewer: fitted up to the pane, and drawn again at each zoom, so it stays sharp.

**Open With** (`C-M-S-e`, `Open: This File With...`) opens the file the focused tab shows in another editor that can show it, as a tab beside it: an SVG being edited, drawn by the image viewer, or a drawn one opened back up as its markup. The list holds only the editors that can show that file; one shown by no other says so instead. Where a tab of the chosen editor in the same pane already has the file open, it goes to that tab instead of opening a second one, and a text editor holding the file in another buffer brings that buffer forward. The viewers read the local disk alone, so a remote file is offered to the text editor only.

**Keys** (`Keys: Show Every Command`) lists every command and the chord bound to it, read from the keymap in force, so it cannot disagree with what the keys actually do. `/`, `n`, and `N` search it, over both columns, so a chord you pressed by accident is as easy to look up as a command you are hunting for. `Enter` runs the command under the cursor: the editor hands the pane back first, so a command that opens an editor of its own has somewhere to open.

## Configuration

Config lives in `~/.config/winter-term/` (`%APPDATA%\winter-term` on Windows), split in two:

- `settings.kdl` for appearance and behaviour
- `keybindings.kdl` for the `window` and `editing` chords, and each editor's own block (`git`)

Both are written in [KDL](https://kdl.dev/). Winter installs a reference copy of each, and the shipped copies in `crates/winter-term/samples/` are the real defaults rather than examples: anything you do not mention keeps its default, anything you do mention replaces it.

Changes apply on save, without a restart. `winter --reload` asks a running instance to reload as well.

### Updating Winter without losing your session

After installing a new binary, run `winter --reload` (or the `reload` command in the palette) in a running window. On Unix (tested on Linux) the window replaces itself with the new binary in place: every program running in a terminal pane (Claude Code, an editor, a build, a remote shell) keeps running with its process id and its pty, the layout and focus come back, and each pane's screen and scrollback are rebuilt from a copy taken at the moment of the reload. Each program is then asked to redraw by a resize, so full-screen programs repaint themselves. What does not come back: editor tabs other than Dir, Git, Grep, and the text editor, block history and folds, and the text of anything a program wrote in the instant between the copy and the replacement. If the new binary cannot be started, the window stays as it is and says so.

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
| `dim-inactive` | Dim panes that are not focused |
| `url-underline` | Underline detected URLs and OSC 8 links |
| `palette-match-underline` | Underline matched characters in the palette |
| `paste-on-right-click` | Right click pastes instead of opening a menu |
| `restore-session` | Reopen the previous layout on launch, with its Dir, Git, Grep, and text editor tabs where they were looking |
| `scrollback-lines` | Scrollback ceiling |
| `shell` | Shell to spawn, instead of the system default |
| `shell-linux` / `shell-macos` / `shell-windows` | Per-platform override of `shell` |
| `window-title-template` | Window title, e.g. `"{{ app_name }} - {{ pane_title }}"` |
| `rainbow-parens` | Color brackets by nesting depth |
| `sentence-highlight` | Alternating bands over sentences, as a reading aid |
| `wrap-indent` | Hang the continuation of a soft-wrapped line under its start. Applied to the scrollback when a width change settles; the visible screen always wraps at the margin, where programs expect it |
| `wrap-words` | Break soft wraps at word boundaries instead of mid-word. Applied to the scrollback when a width change settles, like `wrap-indent` |
| `prompt-edit-bindings` | `"emacs"` (default) or `"none"`, see above |
| `icons` | Editor entry icons: `"svg"` (default), `"font"`, or `"none"` |
| `cursor { ... }` | `blink`, `hide-in-inactive`, plus the shape per mode: `insert`, `normal`, `visual` |
| `status-bar { ... }` | `show` (default on), `show-mode` (default on), `segments` (the right side's segments in order, from `antigravity`, `claude`, `zai`, `editor`; default all, in that order; a bare `segments` shows none), `priority` (the segments in the order they hold on when the bar is short of room, the first kept longest; one left out goes before any listed; default none, which drops from the left), and the per-mode icons |
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
winter mux kill <name>      # terminate one
```

A session name may hold letters, digits, `-`, `.`, and `_`, and must not start with `-`. Sessions are reachable over a Unix socket locally, or over an SSH tunnel to a remote server. PTY geometry is arbitrated at the smallest attached client, so a larger pane letterboxes rather than rendering a stream wrapped for the wrong width.
