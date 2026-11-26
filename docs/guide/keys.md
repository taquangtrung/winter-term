# Keys

The Normal, Insert, and Visual keys below are built in. The window keys at the end, and every editor's own keys, can be rebound in `keybindings.kdl` or on the [Keyboard Shortcuts](keyboard-shortcuts.md) page.

## Motions

These work in Normal and Visual mode. Most take a count: `5j` moves down five lines.

| Keys | Motion |
|---|---|
| `h` `j` `k` `l` | Left, down, up, right (the arrows work too) |
| `w` `b` `e` | Next word, previous word, end of word |
| `W` `B` `E` | The same, a word being any run of non-blanks |
| `ge` `gE` | End of the previous word |
| `0` or `\|` | First column |
| `^` or `_` | First non-blank |
| `$` | End of the line |
| `g_` | Last non-blank |
| `{` `}` | Previous, next paragraph |
| `%` | Matching bracket |
| `H` `M` `L` | Top, middle, bottom of the screen |
| `gg` `G` | First line, last line |
| `PageUp` `PageDown` or `C-b` `C-f` | Page up, down |
| `C-u` `C-d` | Half a page up, down |
| `C-y` `C-e` | Scroll a line up, down |
| `zz` `zt` `zb` | Put this line at the middle, top, bottom |

## Character search

| Keys | Action |
|---|---|
| `f{char}` `F{char}` | Jump to the next, previous `{char}` on the line |
| `t{char}` `T{char}` | Jump just before it |
| `;` `,` | Repeat the last one, forward or back |

When the character appears more than once, labels let you pick the spot with one key.

## Operators and text objects

| Keys | Action |
|---|---|
| `d` `c` `y` | Delete, change, yank, followed by a motion or text object |
| `yy` / `Y` | Yank the line, yank to its end |
| `"{reg}y{motion}` | Yank into register `{reg}` |
| `D` `C` | Delete, change to the end of the line |
| `dd` `cc` `S` | Delete, change the whole line |
| `x` / `s` | Delete, substitute the character under the cursor |
| `r{char}` | Replace the character under the cursor |
| `~` | Flip its case |
| `.` | Repeat the last change |

A text object follows `i` (inside) or `a` (around): `diw` deletes a word, `ci"` changes the text in quotes.

| Object | Selects |
|---|---|
| `w` `W` | Word, big word |
| `p` | Paragraph |
| `s` | Sentence |
| `"` `'` `` ` `` | Text in those quotes |
| `(` `)` `b` | Text in parentheses |
| `[` `]` | Text in brackets |
| `{` `}` `B` | Text in braces |
| `<` `>` | Text in angle brackets |

In a terminal, `d` and `c` only reach the line the shell is editing. `y` reads the whole scrollback, so `yiw` or `yap` copies without entering Visual mode first.

## Surround

| Keys | Action |
|---|---|
| `ys{object}{char}` | Surround the object with `{char}` |
| `cs{from}{to}` | Change the surrounding `{from}` to `{to}` |
| `ds{char}` | Delete the surrounding `{char}` |

## Visual mode

| Keys | Action |
|---|---|
| `v` `V` `C-v` | Select by character, line, block |
| `M-i` | Select the paragraph by lines |
| `o` | Go to the other end of the selection |
| `gv` | Select the last selection again |
| `y` / `Y` | Yank the selection, yank its whole lines |
| `"{reg}y` | Yank into register `{reg}` |

## Search

| Keys | Action |
|---|---|
| `/` `?` | Search forward, back, in a find box at the pane's top right |
| `n` `N` | Next, previous match |
| `*` `#` | Search for the word under the cursor |
| `Esc` | Close the search, the cursor staying on the match |

`n` and `N` still work after the box closes.

## Marks, registers, and jumps

| Keys | Action |
|---|---|
| `m{a-z}` | Set a mark |
| `` `{a-z} `` / `'{a-z}` | Jump to it, exactly or at the line's first non-blank |
| ``` `` ``` or `''` | Back to where the last jump started |
| `` `. `` / `` `^ `` | The last change, where Insert was last left |
| `"{reg}` | Use that register for the next yank or paste |
| `p` | Paste |
| `C-o` `C-i` | Back, forward through the jumps |
| `g;` `g,` | Back, forward through the changes |

Marks and jumps belong to each tab. Registers are shared, so a yank in one tab pastes in another.

## `g` keys

| Keys | Action |
|---|---|
| `gt` `gT` | Next, previous tab |
| `g<` `g>` | Move the tab left, right |
| `gx` or `M-.` | Open what is under the cursor: a URL in the browser, a directory in Dir, a file at its `path:line` |
| `gy` | Yank the block under the cursor |
| `gs` | Scout over the pane (see [Pickers](pickers.md)) |
| `gn` `gN` | Select the next, previous match |
| `gp` `gP` | Jump to this prompt, the previous one |

## Blocks

Each command and its output is a block, once [shell integration](terminal.md#shell-integration) is set up.

| Keys | Action |
|---|---|
| `]b` `[b` | Next, previous block |
| `gy` | Yank the block under the cursor |
| `q` | Label every block; press a label to act on it |
| `F8` / `S-F8` | Open the next, previous `path:line` the last command printed |
| `C-S-b` | Run one of the project's builds (see [Compile](compile.md)) |
| `F6` / `S-F6` or `F7` | From a `.tex` file, show its line in the PDF, here or in the other pane |
| `F6` or `F7` | From a Markdown file, show it rendered in the pane beside |

## Window, pane, and tab keys

These work in every mode.

| Keys | Action |
|---|---|
| `M-S--` / `M-S-\` | Split across, down; the new pane opens a launcher |
| `M-S-=` | Zoom the pane, or unzoom |
| `M-S-w` / `M-S-o` | Close the pane, close every other pane |
| `C-S-q` / `M-S-q` | Close the tab, close every other tab in the pane |
| `C-x M-S-[` / `C-x M-S-]` | Close the tabs to the left, to the right |
| `M-h/j/k/l` | Move to the pane left, below, above, right |
| `M-g M-h/j/k/l` | Move the tab to the pane left, below, above, right |
| `M-1` to `M-9` | Go to pane N |
| `C-M-1` to `C-M-9` | Close pane N |
| `S-PageUp` / `S-PageDown`, `M-S-h/l` | Scroll a page up, down |
| `M-S-k/j` | Scroll a line up, down |
| `M-S-a` or `M-S-,` / `M-S-e` or `M-S-.` | Scroll to the top, bottom |
| `C-S-n` / `C-S-w` | New terminal, close tab |
| `C-S-t` | Reopen the last closed tab |
| `C-M-S-n` | New window |
| `C-]` / `C-[`, `C-PageDown` / `C-PageUp` | Next, previous tab |
| `C-M-]` / `C-M-[` | Next, previous tab by recent use |
| `C-M-S-]` / `C-M-S-[` | The same, among tabs of one kind |
| `C-1` to `C-9` | Go to tab N |
| `C-S-c` / `C-S-v` | Copy, paste |
| `C-,` | Settings |
| `C-=` / `C--` / `C-0` | Font bigger, smaller, reset |
| `C-M-S-r` | Reload |
| `F11` | Full screen (on macOS, the system's `C-s-f`) |
| `C-S-d` | Dir |
| `C-S-f` | Browse files |
| `C-S-g` | Git |
| `C-x k` or `C-M-S-q` | Kill the tab's buffer |
| `C-x C-b` | Switch to a buffer |
| `M-.` / `C-M-S-e` | Open the path under the cursor, open it with another program |
| `M-'` | Copy the tab's directory |
| `M-x` | Command picker |
| `C-S-r` | Recent workspaces |
| `C-M-r` | Reopen the last picker |
| `C-M-m` | Notifications |
| `C-M-'` | Scratch |
| `C-j` | Scout this tab |
| `C-S-o` | Outline of the file |
| `C-S-h` / `C-S-l` / `C-S-j` | Left sidebar, right sidebar, bottom panel (see [Panels](basics.md#panels)) |
| `C-S-z` | Recent tabs, buffers, files, and directories |
| `C-M-z` / `M-S-z` | Find a file in the project, in the working directory |
| `C-M-g` | Search the project as you type |
| `C-S-Up` / `C-S-Down` | Previous, next prompt |
| `C-x (` / `C-x )` / `C-x e` | Start, stop, replay a keyboard macro |
| `F1 h` / `F1 k` | Keys of this tab, what a key runs |

Commands with no default key are in the command picker (`M-x`), and yours to bind. The history picker has no default key now; bind `toggle_history_palette` to give it one. Each command shows the key that runs it in the tab the picker was opened from, so a key bound only in some files shows only there.

## Panel keys

These work in a panel's view; see [Panels](basics.md#panels) for the panels themselves.

| Keys | Action |
|---|---|
| `j` / `k` | Next, previous row |
| `gg` / `G` | First row, last row |
| `RET` | Open the row |
| `l` / `h` | Open, close a folder or group |
| `r` / `M-S-r` | Look again |
| `t` | Open the view's tool among the panes |
| `.` | Files: show or hide what git ignores |
| `s` / `u` | Git: stage, unstage the file |
| `d` | Output: remove the build |
| `]` / `[` | Output: read the next, previous build; Problems: the next, previous source |

## Rebinding keys

`keybindings.kdl` holds one `commands` list. Each line names a command, then its keys:

```kdl
commands {
    new_terminal "C-S-n"
    font_increase "C-=" "C-+"
    open_remote "M-g r" in="git"
    toggle_archive "M-z" in="dir"
    kill_buffer "s-k" platform="macos"
}
```

- Keys are written as in Emacs: `C-` Ctrl, `M-` Alt, `S-` Shift, `s-` Super (Cmd on macOS), in that order. Other keys are `RET`, `TAB`, `SPC`, `ESC`, `DEL`, and names such as `<up>`. Letters are lower case, so Shift-s is `S-s`.
- A binding can be a sequence of up to four keys, `M-g M-h`. After the first, a hint shows what can follow; `ESC` cancels.
- `in="dir"` binds an editor's own command, by the id the Keyboard Shortcuts page shows. `"none"` removes a command's keys.
- `platform="macos"`, `"linux"`, or `"windows"` binds a key on that platform alone.
- `when="…"` binds a single key only where a condition holds, ahead of the tab's own keys, as VS Code's `when` does: `accept_completion "TAB" in="editor" when="completion"`. Conditions are joined with `&&` and negated with `!`: `completion` (the completion popup is open), `selection`, `lsp` (a language server serves the file), a mode (`insert`, `normal`, `visual`, `replace`), a focus (`editor`, `git`, `terminal`), and `lang == rust`. A compared value may list several with `|`: `lang == latex|bibtex`. Where several hold, the one with the most conditions wins, then yours.
- A mistake in the file (a key that does not parse, a command that does not exist, two bindings that clash) is reported when the file loads.

On Linux and Windows, most of Winter's keys use `C-S-`, so a `C-<letter>` reaches the shell, except `C-j`, `C-[` (the shell's `Esc`), and `C-]`, which Winter keeps. On macOS the usual Cmd keys are added too (`s-t`, `s-w`, `s-c`, `s-v`, `s-,`, `s-1` to `s-9`, `s-d`). Option types characters unless `option-as-alt` makes it Alt: `"left"` (the default), `"right"`, `"both"`, or `"none"`.

Vim's motions are built in and cannot be rebound.

## Typing into every pane

`Terminal: Type into Every Pane (Toggle)` sends what you type to every terminal on screen at once, like tmux's synchronized panes. The mode shows `[ALL PANES]` while it is on. Bind `broadcast_input` to give it a key.
