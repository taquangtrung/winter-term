# Winter usage guide

How to drive Winter day to day: the modes, the full keymap, and the settings that change them.

For what Winter is and how to install it, see the [README](../README.md). For how it is built, see [`architecture.md`](architecture.md).

## Modes

A window is split into panes, each a strip of tabs, and every tab is a terminal or a page. A terminal tab is always in exactly one of four modes, and each keeps its own; a page tab is in none, since it owns the keys it binds. The status bar is shown by default (`status-bar { show #false }` hides it and frees its row), and the mode indicator can be enabled in it via `status-bar { show-mode #true }` (hidden by default).

| Mode | Who owns the keyboard | Enter it with |
|---|---|---|
| **Insert** | The PTY. This is an ordinary terminal. | `i`, `a`, or `o` from Normal |
| **Normal** | Winter. Motions, operators, layout commands. | `Alt-Esc` or `Ctrl-Shift-Space` from Insert |
| **Visual** | Normal, but motions extend a selection. | `v` `V` `Ctrl-v` from Normal |
| **Block-Focus** | A rich block's WebView. | `Enter` from Normal, on a block |

Insert is the default, so Winter behaves like a normal terminal until you ask it not to.

**Getting into Normal mode.** Press `Alt-Esc` (or `Ctrl-Shift-Space`). Bare `Esc` is always sent to the program running in the pane (vim, btop, Claude Code, or the shell) so it never interferes with full-screen editors or modal tools.

**Getting out.** `Alt-Esc`, `i`, `a`, or `o` return to Insert. `Esc` in Normal stays in Normal, and it goes to Winter's own features first: a half-typed command or count, a search, a quick-select, a selection. Only when Winter has nothing of its own to put away does it reach the program underneath, so with a program running (or a tab completion pending) an `Esc` is sent to it.

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
| `v` `V` `Ctrl-v` | Charwise, linewise, blockwise Visual |
| `Alt-i` | Select the paragraph under the cursor, linewise (vim's `vip`) |
| `o` | Move the cursor to the other end of the selection |
| `gv` | Restore the last Visual selection |
| `y` | Yank the selection and leave Visual |
| `Y` | Yank the whole lines the selection touches and leave Visual |
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
| `` `` `` or `''` | Back to where the last jump started |
| `` `. `` | The last change |
| `` `^ `` | Where Insert was last left |
| `` `[ `` `` `] `` | The start and end of the last yank |
| `"{reg}` | Use register `{reg}` for the next yank or paste |
| `p` | Paste |
| `Ctrl-o` `Ctrl-i` | Jump backward, forward through the jumplist |
| `g;` `g,` | Step back, forward through the changelist |

The marks named with punctuation are kept for you rather than set by hand, the way Vim keeps its own. Marks and the jumplist are per pane. Registers are shared across every pane, so a yank in one pastes in another.

### `g` commands

| Keys | Action |
|---|---|
| `gt` `gT` | Next tab, previous tab |
| `g<` `g>` | Move the current tab left, right |
| `gx` | Open what is under the cursor: a URL goes to the browser, a directory to the Dir page, a path to the editor over this pane, at the line a `path:line` reference names. `Alt-.` does the same from any mode (see the file browser below for where it looks without a cursor) |
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
| `Enter` | Focus a rich block, handing keys to its WebView |
| `F8` / `Shift-F8` | Open the next, the previous file reference the last command printed |

`F8` reads the newest command output that holds a `path:line` or `path:line:col` reference to a file that exists (a compiler's errors, a test runner's failures, a linter's findings), resolved against the directory the command ran in, and opens each in turn in the editor at its line, the way Emacs' `next-error` walks a compilation. It keeps reading the same terminal while the editor has the focus, and starts over when newer output arrives.

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
| `Shift-Alt-=` | Zoom the focused pane (toggle) |
| `Ctrl-Shift-q` | Close the focused pane |
| `Shift-Alt-o` | Close every other pane |
| `Alt-h/j/k/l` | Move focus between panes |
| `Alt-1` .. `Alt-9` | Focus pane N |
| `Ctrl-Alt-1` .. `Ctrl-Alt-9` | Close pane N |
| `Shift-Alt-h/l` | Scroll page up, page down |
| `Shift-Alt-k/j` | Scroll line up, line down |
| `Shift-Alt-a` / `Shift-Alt-e` | Scroll to top, to bottom |
| `Ctrl-Shift-Alt-n` | New window |
| `Ctrl-Shift-n` / `Ctrl-Shift-w` | New tab, close tab |
| `Ctrl-PageUp` / `Ctrl-PageDown` | Previous tab, next tab |
| `Ctrl-[` / `Ctrl-]` | Previous tab, next tab |
| `Ctrl-1` .. `Ctrl-9` | Go to tab N |
| `Ctrl-Shift-c` / `Ctrl-Shift-v` | Copy selection, paste |
| `Ctrl-,` | Open settings |
| `Ctrl-=` / `Ctrl--` / `Ctrl-0` | Font bigger, smaller, reset |
| `Ctrl-Shift-d` | Open Dir as a tab in the focused pane (toggle) |
| `Ctrl-Shift-g` | Open Git as a tab beside the work (toggle) |
| `Ctrl-Shift-u` | Put back the last thing closed: a tab, or a whole pane |
| `Ctrl-Shift-e` | Pick from every page tab: the open ones, then the closed ones |
| `Alt-x` | Command palette |
| `Ctrl-Shift-r` | History palette |
| `Ctrl-Shift-z` | Pane switcher (then press the digit shown on a pane) |
| `Ctrl-Shift-Up/Down` | Previous, next prompt block |
| `Ctrl-Backspace` | Delete the word before the cursor |

A single-chord binding whose action is not one of the built-in window actions is looked up against the command palette instead, so `"M+q" "mux_new_session"` works.

## Pages

A page is a tab Winter paints itself: Dir, Git, Grep, the editor, a PDF, Processes, System, and Keys. Each opens as a tab of its own in a pane's strip, a peer of the terminal tabs beside it, and the terminal it was opened from keeps running one tab away. Close a page's tab with `Ctrl-Shift-q`, or press its own chord again to step back to the tab you came from. The window chords all still work from a page: split, zoom, close, and `Alt-h/j/k/l` to move focus.

`Ctrl-Shift-u` (`Reopen Closed Tab`) puts back the last thing that was closed. For a page, that is the page itself coming back and not a fresh one of its kind: the editor's buffers and the edits never written to them, where a listing's cursor was and what it had folded, the page a document was turned to. It re-reads what it was showing on the way back.

For a pane, it is the pane: the split comes back where it was, with a shell in the directory the old one was in and every page it was holding back in its strip. The shell itself cannot come back, since closing a pane ends the process; everything around it can. Pressing the key again reaches whatever was closed before that, up to the last eight. A page toggled off by its own chord is not one of them, because the same chord toggles it back on and keeping those would only bury the closes you cannot undo any other way.

`Ctrl-Shift-e` (`Go To or Reopen Tab...`) shows every page tab as a list to pick from, in the fuzzy panel every other list in Winter is picked from: the ones open somewhere first, labelled with the tab they are in, then what was closed recently, newest first, each saying how long ago it was closed. Every row names the kind of page, what it is showing, a `[+]` where it is holding edits that are on no disk, and the directory it is looking at, so typing `main` finds the editor that was on `main.rs` and `win` finds the listing of a tree with that in its path. `Enter` on an open page goes to it rather than opening a second one; `Enter` on a closed one puts it back and leaves the rest closed. `Esc` changes nothing.

Closing a pane or a tab with a page holding unwritten edits asks first, naming what would go with it. Answering `y` closes it and keeps the page anyway, so the edits are still a `Ctrl-Shift-u` away; anything else leaves everything as it was. A page holding unsaved edits is never the entry the stash drops when it fills up, and the first close of a session says in the status bar which key brings it back.

Every page asks the same way, in the middle of the window. A question with a known set of answers opens a picker: what the list is of on top, a filter line under it, and the matches below, narrowing as you type. `Up`/`Down` or `Ctrl-p`/`Ctrl-n` move through them, `Enter` takes the highlighted one, and `Esc` answers nothing. A question with no list to offer, or one whose answer is a name that does not exist yet, opens the same panel without the list: the question, the line it is answered on, and how to answer it. A question that cannot be undone is answered by a single key rather than a line, and says so instead of drawing a caret. Either way the question is where the eye already is, and the status bar is left to what it was saying.

Every page answers to `Alt-Shift-,` and `Alt-Shift-.` for the top and the bottom of what it is showing, which are emacs' `M-<` and `M->` under the fingers, so a page is read with either set of habits: the Vim `gg` and `G` reach the same two rows.

Right-clicking inside a page opens that page's own menu, rather than the terminal's: over a listing there is nothing to paste into, and the entries that would mean something act on rows the terminal knows nothing about. The click moves the cursor to the row it landed on before the menu opens, so what is chosen acts on what was clicked. What the menu offers follows that row, because a commit cannot be staged and a section heading has no path to copy. Each entry is a key the page already binds, shown by name, so the menu is a way of finding a command rather than a second set of them.

Every page searches the same way. `/` asks for text and moves the cursor to the next row holding it, `n` and `N` step through the rest of the matches and wrap at the ends, and a search that finds nothing says so instead of sitting still. Matching is literal and pays no attention to case: a directory listing matches on the entry's name, every other page on the whole row as it is painted, so a chord is as findable as the command beside it. A search moves the cursor and nothing else: marks, folds, and everything staged stay as they were. Grep is the one exception to the first part, since its rows already are a search: `/` there asks for a new one.

**Dir** (`Ctrl-Shift-d`, or `Dir: Open Working Directory`) lists a directory, rooted at the working directory of the pane it opened from.

| Key | Action |
|---|---|
| `j` `k` or `Down` `Up` | Move down, up |
| `Home` / `End` | First entry, last entry |
| `Enter` | Enter a directory, or open a file in the editor |
| `Ctrl-o` | Open the file under the cursor in `$EDITOR`, in a new tab |
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
| `_` / `+` | New file, which opens in the editor; new directory |
| `R` | Rename the entry under the cursor |
| `C` / `Alt-m` | Copy, move the targets: one takes a new name, several take a directory chosen from a list |
| `x` | Move the targets to the trash, after confirming |
| `X` | Delete the targets for good, after confirming |
| `*` | Set permission bits, as octal |
| `&` | Open the entry with the system handler |

A directory's size is not something the filesystem knows, so `Shift-Alt-s` walks each one in the background, a directory at a time, showing `...` until a total arrives. Turning it off stops whatever is still walking. Totals are kept while you stay in the same directory and dropped when the root moves.

An operation acts on the marked entries, or on the entry under the cursor when nothing is marked, never both: the header shows how many are marked and what the last operation reported. A name typed into a prompt is a name, so `../elsewhere` is refused rather than reaching outside the listing, and nothing overwrites an existing entry. Copy and move take a new name for one target and a destination directory for several. When another pane on screen is showing a directory (a listing, or a tool rooted somewhere), it is offered first: the prompt for one target starts out holding its path, and the list for several puts it at the top. A whole path typed out in full is taken as a directory to copy or move into, under the entries' own names.

`x` sends what it removes to the desktop trash (the freedesktop.org one, in `~/.local/share/Trash`), with a record of where each entry came from, so a file manager can restore it; `X` deletes for good. Where there is no trash to send to, `x` says so rather than deleting.

In a git checkout, each changed entry carries git's letter after its name (`M` modified, `A` added, `?` untracked, `U` in conflict, colored as the git view colors them), and a directory holding changes carries a dot. The marks are read in the background whenever the listing is, so they follow edits and stages made anywhere. Moving the root clears the marks, since a mark held over would count toward an operation in a listing it was never part of.

`Ctrl-x Ctrl-q` turns every listed name into editable text, in the manner of Emacs' `wdired`, to rename many entries at once. The names take the editor's keys: Vim in Normal and Visual mode, the Emacs chords while typing. Each name is a line that stands for its entry, so nothing adds or removes one: `dd` empties a name rather than pulling the ones below it up, and `o`, `J`, and a line-joining `Ctrl-k` do nothing. `ZZ`, `Ctrl-s`, or `Enter` while typing applies the differences as renames, refusing the whole batch if any name is not a name or two land on the same one, before anything moves. A batch of more than one rename is listed for a yes first. Swapping two names, or a longer cycle, goes through: entries waiting for a name another still holds are parked under a temporary one until it is free; `q` with nothing part-typed, `ZQ`, or a second `Ctrl-x Ctrl-q` throws them away. The same Emacs chords edit the answer in any prompt a tool asks, such as a search or a new name, and `Up` and `Down` (or `Ctrl-p` and `Ctrl-n`) bring back what that same question was answered with before, newest first, the way a shell's history does.

Directories sort before files whatever the key, and moving the root, toggling a view option, or folding keeps the cursor on the entry it was already on. A block cursor sits on the active row's first non-blank column and follows it as you move, so a page shows where a selection would start before you ask for one. `v` starts selecting from there, for copying out of a listing: `hjkl` and the arrows move and extend, `w` and `b` step words, `0`, `$`, `g` and `G` jump, `V` switches to whole lines, a second `v` drops the selection, `y` or `Enter` copies and leaves, and `Esc` cancels. While selecting it answers every key ahead of the tool, so the motions mean what they do in Vim rather than what the listing binds them to; chorded keys still reach the window, so splitting, zooming, and moving focus work from inside it. Dragging with the mouse selects the same rows.

Each entry carries an icon for what it is. The `icons` setting chooses where it comes from: `"svg"` (the default) draws bundled artwork keyed by extension, exact filename, or directory name, which needs nothing of the terminal font; `"font"` draws a Nerd Font glyph for the entry's broad category, which scales, themes, and copies like any other character but needs a patched font, as the status bar's own mode glyphs already do; `"none"` draws neither.

**Git** (`Ctrl-Shift-g`, or `Git: Status`) shows the working tree of the repository the pane's directory sits in, as foldable sections: the changes, then the stashes, then what the upstream has that you do not, then the recent commits. A file changed both in the index and in the working tree appears in both, which is what lets one half be staged without the other. Every command runs from the repository root, off the event-loop thread, and the view re-reads the tree after anything that changed it; a failure is reported in the header rather than swallowed.

| Key | Action |
|---|---|
| `j` `k` or `Down` `Up` | Move down, up |
| `Alt-n` / `Alt-p` | Next, previous entry, skipping blank lines |
| `g` | Jump: `t` untracked, `u` unstaged, `s` staged, `r` recent, `g` top, `j`/`k` next, previous entry |
| `/` | Search the view, or the log or listing filling it |
| `n` / `N` | Next, previous match |
| `Tab` / `Shift-Tab` | Fold the section, fold everything |
| `Enter` | Open the file under the cursor in the editor; from a hunk, at the line it changes |
| `Ctrl-o` | Open it in `$EDITOR` instead, in a new tab |
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
| `P` / `F` / `f` | Push, pull with rebase, fetch all |
| `Ctrl-Shift-p` | Quick push: the current branch to the remote it tracks (else `origin`), setting it to track, with no questions |
| `G` | Re-read the working tree |
| `+` | Show more of the recent commits, 50 at a time, while the list runs on |
| `Ctrl-Shift-c` | Commit what is staged, writing the message in the commit editor |
| `Ctrl-Shift-s` | Stage all: untracked files when the cursor is in that section, tracked changes otherwise |
| `!` | Run any git command |
| `Alt-b` | Blame the file under the cursor |
| `Alt-y` | Show every ref |
| `Alt-g` | Open the remote in a browser |
| `Ctrl-Space` | Set the mark on a diff line, or clear it |
| `Alt-s` / `Alt-u` | Stage, unstage only some lines: from the mark to the cursor, or the line under the cursor |

The view is read by color as much as by name, the way magic-vscode's own git view is: a section heading wears the hue of what sits under it (cyan staged, yellow unstaged, green untracked, red contested), every other heading and the header block's own labels are blue, a file says what changed to it in that change's own hue, a branch is magenta and one on a remote green, a tag cyan, and anything the upstream has not seen yet — the commits ahead of it and the count saying how many — is red.

When git is part-way through something — a merge, a rebase, a cherry-pick, a revert, a bisect — a `State:` line in the header says so, what it is working on, how far along it is where git counts the steps, and the keys that finish it. Nothing in a porcelain status reports any of that, so without the line the only sign of an open rebase is a section of conflicts and no reason for them. It is read from the files git writes it in, under the repository's own directory.

Closing a tab, a pane, the other tabs or panes, or the whole window asks first when a terminal has a program running in it (an agent such as Claude Code, an editor, a build; a shell sitting at its prompt does not count), and when a page tab holds edits that are on no disk. Answer `y` to close; any other key keeps things as they are. Naming the running program uses the process list, so on platforms without `/proc` the question says "a program" instead, and a program that does not own the screen may not be noticed there.

With more than one pane, the status view and the commit editor open in another pane rather than over the one you are working in: the one beside it in the same row (to its right first, else its left) if there is one, otherwise the next in order, and asking for the status view again goes to the one already open there. With a single pane, one is split off to its right for them.

The status view watches the repository through the platform's file notifications and reads git's status again three seconds after the changes go quiet, redrawing only when the status differs: an edit, a stage from another terminal, a commit, a fetch. Build output and dependency directories at the top of the tree (`target`, `node_modules`, `dist`, `build`, and the like) are not watched, and inside `.git` only the index, `HEAD`, the references and the merge state count. Where the platform cannot be watched, it reads on a three second timer instead. A log, a diff, or any other output view is left alone while you read it, and an open file diff is re-read on each pass since the status cannot say that a file changed again.

A triangle opens every row a fold key acts on, pointing down while what hangs off it shows and right while it is hidden: section headings, file rows, and a commit's hunk headers all carry one. A hunk in the status view does not, since `Tab` there folds the file it belongs to rather than the hunk itself. Opening a row brings what it opened into view: a block that would run past the pane's bottom pulls its own row to the top, where the most of it fits, while one that already shows in full leaves the page where it was.

On a section heading, `s`, `u`, and `x` act on every file in that section. On a hunk, or on any line inside it, they act on that hunk alone: the patch is built from the hunk and fed to `git apply` on standard input, so the rest of the file is untouched. `Alt-s` and `Alt-u` go finer, to single lines: the changes left out are written so the patch still applies (an addition not chosen is dropped, a removal not chosen stays as context), and the header's counts are worked out again.

On a file in conflict, `.` then `o` or `t` settles it by taking one side whole, ours or theirs, and stages the result, which is what marks the conflict resolved. Discarding an untracked file deletes it, since `git restore` cannot reach a path that is not in the index.

A command that takes a name asks for it as a list rather than a blank line: checking out, merging or rebasing onto a branch offers every branch, deleting one offers the local branches, deleting a tag offers the tags, and removing or pruning a remote offers the remotes. Where there is nothing to list, the question falls back to being typed.

Every key that waits for a second one names what the second key may be, on the same card the terminal's own key hints use: centred over the window, above whatever is under it, so it cannot be pushed off the bottom by a long listing. It shows at once rather than after a pause, since the menus are how the tools are learned. Any key the menu does not offer closes it without doing anything, which is what makes a mistyped second key harmless.

These keys open a menu, and the next key picks from it:

| Key | Menu |
|---|---|
| `b` | Branch: checkout, create, delete |
| `c` | Commit: in an editor, amend, amend keeping the message |
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

A log, a listing, a blame, or a diff fills the view; `j`/`k` move through it, `/` searches it, `+` asks for more of a log, `y` copies the first field of a line, which is the commit hash in a log or a blame, and `q` goes back to the status. In a blame, where every row is a line of one file, `Enter` opens that file at the row under the cursor and `Ctrl-o` opens it in `$EDITOR` there.

A commit read with `Enter` opens shut, the way the status view does: the summary, then a `Changes` heading counting the files it touched, then one band per file, each counting the hunks it holds, each saying in words what the commit did to it (`modified`, `new file`, `deleted`, `renamed`) after its fold triangle and before its icon and path, with the patch waiting behind them. It folds the same way too. `Tab` on the heading takes the whole file list away and brings it back, `Tab` on a file band opens it to its hunk headers and shuts it again, `Tab` on a hunk opens that hunk's body, and `Shift-Tab` acts on the whole commit at once, opening every file and hunk of it, or shutting all of them when none is folded.

**Grep** (`Ctrl-Shift-s`, or `Grep: Search Files`) finds the lines under the pane's working directory that hold some text, grouped under one heading per file.

| Key | Action |
|---|---|
| `/` | Search for text, starting from what you searched for last |
| `j` `k` or `Down` `Up` | Move down, up |
| `n` / `N` | Next, previous match, skipping the file names between them |
| `Home` / `End` | First row, last row |
| `Alt-n` / `Alt-p` | Next, previous file |
| `Enter` | Open the file in the editor, at the line that matched |
| `Ctrl-o` | Open it in `$EDITOR` instead, at the same line |
| `y` | Copy the match's `path:line` |
| `G` | Run the same search again |
| `Esc` | Stop a running search |
| `Ctrl-x Ctrl-q` | Edit the matched lines in place, and write them back to their files |

Where [ripgrep](https://github.com/BurntSushi/ripgrep) is installed the search runs through it, which reads the tree's `.gitignore` files and is faster on a large tree; otherwise Winter walks the tree itself. Either way the rules are the same: literal text in any case, hidden files read, and version-control and build directories skipped.

`Ctrl-x Ctrl-q` turns the matched lines into editable text, as Emacs' `wgrep` does, with the editor's keys over them. `ZZ`, `Ctrl-s`, or `Enter` while typing writes every changed line back to its file, each file read and written once through the editor's own save, so line endings and permissions survive. A line whose file no longer says what the search found there (changed since, or cut short in the list) is left alone and counted in the report, rather than written over. `ZQ`, or `q` with nothing part-typed, throws the edits away.

The walk runs off the event loop, so the pane stays live while a large tree is read, and `Esc` abandons it. `/` here asks for a new search rather than searching the rows, since the rows already are one, and `n` and `N` step through what it found. Matching is literal text, whatever the case, the same as every other tool's search.

The walk skips what would swamp the results rather than reading everything: `.git`, `.hg`, `.svn`, `node_modules`, and `target`, files over a megabyte, anything that does not read as text, and symlinks, which are never followed. It stops at 500 matches and says so in the header, because a query loose enough to pass that is one to narrow.

**Processes** (`Ctrl-Shift-,`, or `Processes: Monitor`) lists every process on the machine, refreshed every two seconds. CPU use is a share of one core, so a busy multi-threaded process passes 100. The columns are PID, NAME, CPU%, MEM%, RSS, VRAM, STATE, USER, and the command line. The tree lives in the NAME column, as an indent and a fold marker before the name, so the command stays whole. The list is the tree of who started whom by default, sorted by CPU within each level; `v` flattens it into one sorted list. Linux reads `/proc`, macOS and the BSDs read `ps`, and Windows asks PowerShell, so the working-directory column is missing on Windows and the state is always `run` there. A `VRAM` column of per-process video memory appears when `nvidia-smi` is on the path and answers, on any platform; there is no vendor-neutral way to read it, so AMD and Intel GPUs show no column.

| Key | Action |
|---|---|
| `j` `k` or `Down` `Up` | Move down, up; the vim motions (`gg`, `G`, `Ctrl-d`, `Ctrl-u`) work too |
| `s` / `S` | Sort by the next column, reverse the direction |
| Click a heading | Sort by that column; click the sorted one again to reverse it |
| Click a process | Move the cursor onto it |
| `v` | Switch between the tree and one sorted list |
| `Enter` `Space` `Tab` | Fold or unfold the children of the process under the cursor |
| `h` / `l` | Fold, unfold |
| `/` | Filter by name, command, pid, or user; a match keeps the processes it descends from |
| `Esc` | Clear the filter |
| `x` / `X` | Terminate, kill (asks first) |
| `t` / `T` | Terminate, kill the process and everything under it (asks first) |
| `z` / `Z` | Suspend, resume (not on Windows) |
| `R` | Renice: a nice value from -20 to 19, or a priority class on Windows |
| `r` / `p` | Refresh now, pause or resume the refreshing |
| `y` / `Y` | Copy the pid, the command line |

A strip under the list describes the process under the cursor: its pid, parent, owner, state, CPU, memory and video memory shares, executable path, working directory, and the whole command line. It is left out of a pane shorter than twelve rows so the list keeps the room.

Signals go through the system's own `kill`, `renice`, and `taskkill`, so Winter needs no special privilege and is refused exactly where those are: another user's process needs `sudo` from a terminal. On Windows a terminate only asks a process's windows to close, which a console or background process declines; `X` forces it.

**System** (`System: Monitor` in the command palette; it has no default chord) shows the machine as a whole, refreshed every two seconds: host, operating system, uptime and load average; processor use as a total and a grid of cores with their clocks; memory and swap; each NVIDIA GPU's video memory, load and temperature; and the local disks with a bar for each. Bars turn yellow at 75 percent and red at 90. Linux reads `/proc` and `df`, macOS and the BSDs read `sysctl`, `vm_stat`, `ps` and `df`, and Windows asks PowerShell. macOS has no per-core counter without a system library, so it shows the total only, worked out from what the processes are using. The GPU section needs `nvidia-smi` on the path and is left out otherwise.

| Key | Action |
|---|---|
| `j` `k` or `Down` `Up` | Scroll down, up; the vim motions (`gg`, `G`, `Ctrl-d`, `Ctrl-u`, `Ctrl-f`, `Ctrl-b`) work too |
| `Ctrl-e` / `Ctrl-y` | Scroll one line down, up |
| `n` / `N` | Jump to the next, previous section |
| `r` / `p` | Refresh now, pause or resume the refreshing |

**Editor** opens the file a page was pointing at, as editable text. It is not reached by a chord of its own: `Enter` on a file in Dir, Git, or Grep opens it, at the line that page knew about. It opens as a tab right after the page it was reached from, so closing it puts the listing back where you left it.

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
| `Ctrl-d` / `Ctrl-u` / `Ctrl-f` / `Ctrl-b` | Half a screen, a screen, either way |
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
| `Ctrl-a` / `Ctrl-x` | Add one to the number at the cursor, take one off |
| `.` | Do the last command that changed the text again |
| `u` / `Ctrl-r` | Undo, redo |
| `Ctrl-s` | Write the file |
| `Z Z` / `Z Q` | Write and close; close, discarding edits |
| `Ctrl-o` | Hand the file to `$EDITOR` in a new tab, at the cursor's line |
| `]b` / `[b` | The next open buffer, the one before, in the order they were opened |
| `Ctrl-Alt-[` / `Ctrl-Alt-]` | Back through the buffers you last worked on, and forward again |
| `gb` | List the open buffers, most recently used first, and go to one |
| `Ctrl-/` | Comment the line, or the selected lines, out; or back in when they all are |
| `Alt-/` (typing) | Complete the word from the words around it, then the other buffers; again for the next |

One editor holds as many files as you open into it. Opening a file while the editor is up (from the file browser, or `gx` on a path) adds it as another buffer rather than covering the editor with a second one, and opening a file that is already open goes to it. `]b` and `[b` step through them in the order they were opened, and the header says which of how many you are on. `Ctrl-Alt-[` and `Ctrl-Alt-]` walk them by how recently you worked on them instead, which is usually what you want when two files are in play: one press of `Ctrl-Alt-[` goes back to the file you just left, and pressing it again goes further back rather than bouncing between the two. Walking over a file does not make it the recent one, so the order stays put until you stop and work in it. `gb` lists them in that same order, the file in front at the top and the one before it a row below, and it works in any of the editor's modes. Closing the editor's tab closes every buffer in it, and asks first about any with unsaved edits, so nothing is dropped quietly.

### Commit messages

In a Winter pane, `git commit` edits its message in the built-in editor instead of `$EDITOR`: Winter sets `GIT_EDITOR` to `winter --edit-wait`, which opens `COMMIT_EDITMSG` in a new editor tab, in Insert mode, and holds git until that tab closes. The staged diff is a second tab in the same pane, one tab away, and goes when the editor does. `Ctrl-C Ctrl-C` (or `ZZ`) saves and commits; `Ctrl-C Ctrl-K` (or `ZQ`, or discarding when asked) empties the message so git stops with "Aborting commit due to empty commit message". The header lists the two chords. `Ctrl-Alt-P` swaps the message above git's `#` comment lines for an earlier one from the repository's history (the last ten distinct messages), going further back each press, and `Ctrl-Alt-N` comes forward again to what you had typed. With no running window to ask, `winter --edit-wait` runs `$VISUAL` / `$EDITOR` instead, so git is never left without an editor.

An interactive rebase's plan (`git-rebase-todo`) opens the same way, as text the usual keys edit, with chords for the plan behind `Ctrl-C`: `Ctrl-C` then `p`, `r`, `e`, `s`, `f`, or `d` sets the line's action to pick, reword, edit, squash, fixup, or drop and steps on to the next line, `Ctrl-C j` and `Ctrl-C k` move the line down and up, `Ctrl-C Ctrl-C` (or `ZZ`) finishes, and `Ctrl-C Ctrl-K` empties the plan so git aborts the rebase. A plan git would refuse, one whose first commit squashes or fixes up into nothing, is not finished but said what is wrong with.

What the buffers share is what Vim shares: the registers, the last search, the last char search, and the last change `.` repeats. What belongs to each file stays with it: its marks, its undo history, where the cursor and the window are in it, and what it looked like on disk.

A count typed before a command applies to it, as in Vim: `3x`, `2dd`, `5j`, `2d3w`. Insert mode is an Emacs-style field. Moving: `Ctrl-a`, `Ctrl-e`, `Ctrl-b`, `Ctrl-f`, `Ctrl-n`, `Ctrl-p`, `Alt-b` and `Alt-f` by word, `Alt-m` to the first non-blank, `Ctrl-v` and `Alt-v` by page, `Alt-<` and `Alt->` to the start and end, and `Ctrl-l` to centre the cursor. Deleting: `Ctrl-d`, `Ctrl-h`, `Alt-d` and `Alt-Backspace` by word, `Ctrl-k` to the end of the line (at the end, the line break), `Ctrl-u` to the start, and `Ctrl-w` back a word. What the kills take goes on a kill ring, as in Emacs: kills made one straight after another join into one entry, so `Ctrl-k Ctrl-k` takes a whole line and `Ctrl-y` brings it back whole, and `Alt-y` straight after a `Ctrl-y` swaps what it put in for the kill before. Deletes and yanks made in Normal mode go on the ring as well, so `Ctrl-y` reaches them too. Editing: `Ctrl-t` and `Alt-t` transpose characters and words, `Alt-u` and `Alt-c` upper-case and capitalise the word ahead, and `Ctrl-o` opens a line and stays put. A word here is a run of letters and digits. The Alt chords the window owns (`Alt-h` `j` `k` `l`, the digits, `Alt-x`) are left to it, so there is no `Alt-l` for lower-casing.

Selecting with `v` or `V` puts the operators on what is selected rather than on a motion: `d`, `c`, `y`, `>`, `<`, `J`, `~`, `r`, and `p` all act on the selection and end it, `i` and `a` grow it to a text object, and `o` moves to its other end so it can be grown from either side. `Esc` drops it.

The mouse moves the cursor too: a click puts it on the character under the pointer, dragging selects from where the button went down, and the wheel scrolls, carrying the cursor with it so the two never part company.

Registers work as Vim's do. `"a` through `"z` name one, and `"A` through `"Z` append to it. With no register named, a yank also goes to `"0`, a delete of whole lines or across lines to `"1` (the older ones shifting up to `"9`), and a smaller delete to `"-`. `"_` is the black hole, which takes nothing, not even into the unnamed register, so `"_dd` deletes a line without losing what `p` would put back. `"+` and `"*` are the system clipboard, so `"+yy` copies a line out of Winter and `"+p` puts back whatever was copied anywhere else.

Searching is literal text rather than a pattern, and a search typed in lower case matches either case, while one typed with a capital in it matches exactly. `*` and `#` look for the word under the cursor without typing it out.

It colors what it is reading: comments, strings, numbers, keywords, and the names of types, for the common languages (Rust, Python, Go, TypeScript and JavaScript, C and C++, C#, Java, Kotlin, Scala, Swift, Dart, Zig, Ruby, PHP, Lua, Perl, R, Julia, Elixir, Erlang, Haskell, OCaml, Lisp and Clojure, Nim, SQL, shell, PowerShell, Vim script, CSS and SCSS, HTML and XML, LaTeX) and the configuration formats (JSON and JSONC, YAML, TOML, KDL, INI and `.conf`, `.env`, Nix, Terraform, Protocol Buffers, GraphQL, Dockerfile, Makefile, CMake, and the git and editor dotfiles). Files known by name rather than extension, such as `Makefile`, `Dockerfile`, or `.bashrc`, are recognized too. The colors come from the theme's own palette rather than a scheme of their own, so a file reads as the terminal beside it does, and a file in a language it does not know is painted as the plain text it may well be rather than guessed at. It is a lexer and not a parser: it colors what the characters themselves say and never decides that something is wrong with the code.

If the file changes on disk while it is open with nothing unsaved in it, it is read again the next time the editor comes back to the front, so what is on screen is what is on disk. With unsaved edits it leaves your work alone and asks before overwriting at save time.

Past that it edits text and nothing more: completion is only the words already in the open files, and there is no language server. That is what `Ctrl-o` is for, and it saves before handing over, so `$EDITOR` opens what you were looking at rather than what was last written. A file it cannot open as text says so and stays closed: anything holding a NUL byte, anything that is not UTF-8, and anything over four megabytes, all of which it would otherwise write back as something the original was not. A UTF-8 byte order mark at the head of a file is kept aside rather than shown, and written back on save.

Saving writes through a temporary file beside the original and renames it over the top, so an interrupted write leaves the previous contents whole rather than a truncated file. Undoing back to what was last written leaves nothing to save, so closing does not ask about edits that are no longer there. A file's permission bits, its line endings, and whether it ended with a newline all survive the trip, so saving one edited line does not show up as a whole-file diff. If something else has written to the file since it was opened, saving asks before overwriting rather than throwing that away.

**File browser** (`Ctrl-Shift-f`, or `File: Browse Files`) walks the working directory in the command palette rather than in a pane, for the times you know roughly where a file is and want it open rather than listed.

It opens where the terminal points, when it points at a file or directory that exists: the Normal-mode cursor, else the mouse selection, else the shell's cursor (on a path, or just past one you typed), else the rightmost path on the last line printed. A file opens the browser on its directory with that file selected; a directory opens the browser inside it. Paths resolve against the shell's working directory, and `~`, `..`, and `path:line:col` suffixes are understood. With nothing found, or over a tool page, it opens on the working directory as before.

| Key | Action |
|---|---|
| Typing | Filter this directory's entries |
| `/` after a name | Go into that directory and keep typing from there: `src/`, `../`, `~/`, or a filter like `sr/` that selects `src/`. `/` with nothing typed goes to the root |
| `Up` `Down` or `Ctrl-p` `Ctrl-n` | Move through what is showing |
| `Enter` / `Right` on a directory | Go into it: the browser stays up, rooted there |
| `Enter` on a file | Open it in the editor, over the focused pane |
| `Ctrl-H`, or `Left` | Back up to the parent directory (`Backspace` only deletes typed characters) |
| `Ctrl-Shift-f` again, or `Esc` | Put it away |

Directories come first, then files, with the hidden ones after each group and `../` at the top, so a listing opens on what is usually wanted and typing reaches the rest. The path being browsed is shown above the list, and a directory's row says `dir` where a file's says its size. It reads one directory at a time rather than walking the tree, so it opens instantly however large the tree below it is.

**PDF** opens wherever a file does, on any path ending in `.pdf`: `Enter` in Dir, Git, or Grep, the file browser, or a `gx` reference. Like the editor it has no chord of its own and opens as a tab beside the page it was reached from, so closing it puts the listing back. The document itself is drawn by a web engine rather than by Winter, because a rendered page is not text and cannot be painted as rows; the header above it is Winter's own, in the terminal's font and theme, and says which file and which page of how many. The renderer is packed into the binary, along with the fourteen standard fonts a PDF is allowed to name without carrying, so a document opens with nothing installed and no network access and draws the right shapes on a machine with no fonts of its own. The viewer can read nothing but the file it was opened on.

| Key | Action |
|---|---|
| `{count}` before a motion | Repeat it that many times, as vim does: `3j`, `12k` |
| `j` `k` or `Down` `Up` | Scroll down, up |
| `h` `l` or `Left` `Right` | Scroll left, right, for a page zoomed past the pane |
| `Ctrl-e` / `Ctrl-y` | A line down, up, without moving what is on screen otherwise |
| `Ctrl-d` / `Ctrl-u` | Half a screen down, up |
| `Ctrl-f` / `Ctrl-b`, `Space` / `b`, `PageDown` / `PageUp` | A screen down, up |
| `gg` / `G`, or `Home` / `End` | First page, last page |
| `{count}gg` / `{count}G` | Go to that page |
| `]` / `[` | Next page, previous page |
| `zt` / `zz` / `zb` | Put this page at the top, middle, bottom of the pane |
| `Ctrl-g` | Say which file, which page of how many, and how far through |
| `+` / `-` | Zoom in, out |
| `=` | Fit the page to the pane's width again |
| `i` | Put a cursor in the text, for the motions below |
| `/{text}` / `?{text}` | Search down the document, back up it; the cursor lands on what is found |
| `n` / `N` | The same search again, the same way or the other |
| `Esc` | Abandon a half-typed command |

The motions are vim's, and they compose the way vim's do: a count is typed before the motion it multiplies and is spent by it, `g` and `z` wait for the key that completes them, and the pane's own key-hint card says what finishes one while it is pending. The header shows a count as it is typed, so one entered by accident is visible before it moves anything. The two places a count is read as a position rather than a repeat are `gg` and `G`, exactly as in vim: `5G` is page five, not five pages on.

Where the motions differ from vim's, it is because a document is not a buffer. There is no cursor to move, so `h` and `l` scroll sideways instead of stepping characters, and `Ctrl-d` moves half the pane rather than half a file's worth of lines. `zt`, `zz`, and `zb` move the window and not the page being read, which is what they do in vim too.

`i` puts a cursor in the document's own text, and from there the keys are vim's text motions rather than the window's. `Esc` takes it back out, one step at a time: a half-typed command first, then the selection, then the cursor itself.

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

The motions divide into inclusive and exclusive the way vim's do, which is the difference between `yw` and `ye` over the same word: both land in the same place, but `ye` takes the last letter and `yw` stops before the next word starts. The window keys keep working with a cursor in the text, so `Space`, `Ctrl-d`, `]` and the rest still page through the document the cursor is sitting in.

A PDF carries no structure to ask about any of this, so the text is modelled rather than read. The page is cut into blocks along the empty bands that run right across it: a band down the page is the gutter of a two-column paper, so the motions read down one column and then down the other, and a band across it separates a banner or a footnote from the body, so a title reads before the columns under it rather than in the middle of them. Within a block, glyph runs are clustered into lines by how close their baselines are, a gap wide enough to have been a space becomes one, and a paragraph break is a line gap noticeably wider than that page's usual. Where a document's layout defeats the model, the motions follow the model rather than the page.

Search is Winter's one convention that holds whichever way a document is being read: `/` and `?` ask for text in the terminal's own prompt, and `n` and `N` walk the matches, wrapping around the document once. A pattern typed in lower case matches either case and a capital in one matches exactly, which is the rule the editor searches its files by. A search made with no cursor in the text puts one on what it found, with the whole match drawn under it, since a page that moved with nothing marking what matched would be half an answer.

A document is drawn by a web engine, and a web engine takes the keyboard for the whole window while it is on screen. Winter does not leave it there: the surface hands every key straight back, and it is resolved the way a key typed into any other tool is, its page first and the window chords after. So splitting, zooming, moving between panes and switching tabs all work from inside a PDF, and closing its tab closes it, exactly as it does everywhere else.

**Keys** (`Keys: Show Every Command`) lists every command and the chord bound to it, read from the keymap in force, so it cannot disagree with what the keys actually do. `/`, `n`, and `N` search it, over both columns, so a chord you pressed by accident is as easy to look up as a command you are hunting for. `Enter` runs the command under the cursor: the page hands the pane back first, so a command that opens a tool of its own has somewhere to open.

## Configuration

Config lives in `~/.config/winter-term/` (`%APPDATA%\winter-term` on Windows), split in two:

- `settings.kdl` for appearance and behaviour
- `keybindings.kdl` for the `window` and `editing` chords

Both are written in [KDL](https://kdl.dev/). Winter installs a reference copy of each, and the shipped copies in `crates/winter-term/samples/` are the real defaults rather than examples: anything you do not mention keeps its default, anything you do mention replaces it.

Changes apply on save, without a restart. `winter --reload` asks a running instance to reload as well.

### Updating Winter without losing your session

After installing a new binary, run `winter --reload` (or the `reload` command in the palette) in a running window. On Unix (tested on Linux) the window replaces itself with the new binary in place: every program running in a terminal pane (Claude Code, an editor, a build, a remote shell) keeps running with its process id and its pty, the layout and focus come back, and each pane's screen and scrollback are rebuilt from a copy taken at the moment of the reload. Each program is then asked to redraw by a resize, so full-screen programs repaint themselves. What does not come back: page tabs other than Dir, Git, Grep, and the editor, block history and folds, and the text of anything a program wrote in the instant between the copy and the replacement. If the new binary cannot be started, the window stays as it is and says so.

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
| `window-controls-side` | Which side the window buttons sit on |
| `pane-border-width` | Divider thickness between panes |
| `dim-inactive` | Dim panes that are not focused |
| `url-underline` | Underline detected URLs and OSC 8 links |
| `palette-match-underline` | Underline matched characters in the palette |
| `paste-on-right-click` | Right click pastes instead of opening a menu |
| `restore-session` | Reopen the previous layout on launch, with its Dir, Git, Grep, and editor tabs where they were looking |
| `scrollback-lines` | Scrollback ceiling |
| `shell` | Shell to spawn, instead of the system default |
| `shell-linux` / `shell-macos` / `shell-windows` | Per-platform override of `shell` |
| `window-title-template` | Window title, e.g. `"{{ app_name }} - {{ pane_title }}"` |
| `rainbow-parens` | Color brackets by nesting depth |
| `sentence-highlight` | Alternating bands over sentences, as a reading aid |
| `wrap-indent` | Hang the continuation of a soft-wrapped line under its start (primary screen only; a full-screen app lays out its own continuations) |
| `wrap-words` | Break soft wraps at word boundaries instead of mid-word |
| `prompt-edit-bindings` | `"emacs"` (default) or `"none"`, see above |
| `icons` | Page entry icons: `"svg"` (default), `"font"`, or `"none"` |
| `cursor { ... }` | `blink`, `hide-in-inactive`, plus the shape per mode: `insert`, `normal`, `visual`, `block-focus` |
| `status-bar { ... }` | `show` (default on), `show-mode` (default off), and the per-mode icons |
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

Without it Winter still works, but the whole session is one rolling block: no per-command boundaries, no exit-code tags, no folding. The marks also sharpen two behaviors elsewhere: with them, Winter knows exactly when a command owns the pane - a REPL or an agent CLI running on the primary screen is sent `Esc` from Normal mode, and a fresh prompt stops that the moment the shell does - and where the editable command line begins, so a Vim operator aimed at the shell's own `❯ ` decoration is refused ("that is the shell's own prompt, not the command line") instead of mistranslating into readline chords that edit the wrong characters.

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
