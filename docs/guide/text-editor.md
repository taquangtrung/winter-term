# Text editor

The text editor opens a file as editable text, with Vim keys in Normal mode and Emacs keys while typing. `Enter` on a file in Dir, Git, or a saved list opens it at the line that listing knew about, in a tab next to the listing. The file's path runs along the top as breadcrumbs; click a folder to list it in Dir.

## Keys

| Key | Action |
|---|---|
| `h` `j` `k` `l`, the arrows | Move |
| `w` `b` `e`, `W` `B` `E` | By words |
| `0` `^` `$` `g_` | Line start, first non-blank, end, last non-blank |
| `{` `}` / `%` | Paragraphs / the matching bracket |
| `f` `F` `t` `T`, `;` `,` | To a character on the line |
| `gg` / `G` / `20G` | First, last, twentieth line |
| `C-d` `C-u` `C-f` `C-b` | Half a screen, a screen |
| `m{a-z}` / `'{a-z}` | Set a mark, go back to it |
| `/` `?`, `n` `N`, `*` `#` | Search |
| `i` `a` `I` `A`, `o` `O` | Start typing |
| `v` / `V` | Select by character, by line |
| `R` | Type over the text |
| `x` `r` `~` | Delete, replace, flip the case of a character |
| `d` `c` `y` + motion or object | Delete, change, yank |
| `>>` `<<` | Indent, outdent |
| `p` / `P` | Paste after, before |
| `J` | Join the next line |
| `C-a` / `C-x` | Add one to, subtract one from the number |
| `.` / `u` / `C-r` | Repeat, undo, redo |
| `C-s` (`C-S-s` while typing) | Save; a save while typing goes back to Normal mode |
| `ZZ` / `ZQ` | Save and close, close without saving |
| `C-o` | Open the file in `$EDITOR` |
| `]b` / `[b` / `gb` | Next tab, previous tab, the buffer list |
| `C-/` | Comment or uncomment |
| `M-/` | Complete the word from nearby words |
| `:` | The [command line](#the-command-line) |

Coming back to the editor (from another pane, tab, or window) puts it in Normal mode, except a commit message the first time it opens. While typing (Insert mode), the Emacs keys work: `C-a` `C-e` `C-b` `C-f` `C-n` `C-p` move, `M-b` `M-f` move by words, `C-k` kills to the line's end, `C-y` yanks, `M-y` cycles the kill ring, `C-w` deletes a word back (so do `C-DEL` and `M-DEL`, Ctrl+Backspace and Alt+Backspace), and `C-t` transposes. The same keys work in every text field in Winter, plus `C-g` to cancel and `C-/` to undo.

## Clipboard and registers

- `y`, `d`, `c`, and `x` put what they take on the system clipboard, and `p` pastes from it. `"+` names it too.
- `"a` to `"z` are Winter's own registers, shared with the terminals' Normal mode.
- `"_` takes nothing, so `"_dd` deletes without touching the clipboard.
- `Clipboard: Paste from Clipboard Ring…` lists everything copied this session.

## Search

Search is literal text. A lower-case query matches any case; a capital makes it exact. `*` and `#` search for the word under the cursor.

## Acting on what is at the cursor

`M-a` lists what can be done with what is under the cursor: open a URL or a `path:line`, show a commit hash, or, with a language server, go to a name's definition, find its uses, or rename it.

## The command line

`:` opens a Vim command line for changes over many lines. One `u` undoes the whole command.

- `:g/pattern/command` runs the command on each matching line; `:v/pattern/command` on each other line.
- `:normal keys` types the keys on each line: `:'<,'>normal I# ` comments the selected lines.
- `:d` deletes lines; `:s/pattern/replacement/g` replaces; `:w` saves.
- A range is `%` (the file), `'<,'>` (the selection), `3,8`, `.`, or `$`.
- Patterns are Rust regular expressions (`\d`, `(a|b)`, `$1`), not Vim's.

## Selecting and undoing

- `S-M-<right>` grows the selection to the next enclosing piece of code (word, string, brackets, line, function); `S-M-<left>` shrinks it back.
- Typing after an undo keeps what was undone as a branch. `C-M-/` lists those branches so you can bring one back.

## Multiple cursors

Several cursors edit at once; every key that types, edits, or moves acts at each. Bind these commands in `keybindings.kdl` with `in="editor"`:

- `Cursors: Add at the Next Occurrence` (like VS Code's `C-d`), `Cursors: Add at Every Occurrence`.
- `Cursors: Add on the Line Below`, `Cursors: Add on the Line Above`.
- `Cursors: Skip to the Next Occurrence`, `Cursors: Remove the One Added Last`.

`Esc` or a click leaves only the main cursor.

`C-;` changes every occurrence of the word under the cursor at once; `Esc` or `RET` ends it. `M-n` and `M-p` step between occurrences.

## Language servers

A language server answers the editor's questions about your code. None starts by itself:

- `LSP: Start the Language Server for This File` starts one for the file in front.
- `LSP: Install a Language Server…` lists the servers for the file and installs the one you choose.
- `lsp { auto-start "rust" }` starts a language's server when one of its files opens.
- `LSP: Stop`, `LSP: Restart`, and `LSP: Show Running Language Servers` do what they say.

Winter knows rust-analyzer, pyright and its relatives, gopls, clangd, typescript-language-server, texlab, marksman, and more. To add one, give it a `server` block in `settings.kdl`:

```kdl
lsp {
    server "elixir-ls" {
        languages "elixir"
        extensions "ex" "exs"
        roots "mix.exs"
    }
}
```

While a server serves the file:

| Key | Action |
|---|---|
| `M-.`, `F12`, or `gd` | Go to the definition |
| `M-<left>` / `M-<right>` | Back, forward over jumps |
| `C-c C-/` or `gr` | List the references |
| `M-F12` / `S-M-F12` | Peek at the definition, the references, in a card |
| `C-c C-[` / `C-c C-]` / `C-c C-\` | Type definition, implementation, declaration |
| `M-,`, `M-0`, or `K` | Describe the name under the cursor |
| `C-S-SPC` | Show the call's signature |
| `C-c C-r` or `F2` | Rename everywhere |
| `M-\` or `C-.` | Fixes and refactorings |
| `C-S-M-i` | Format the file |
| `S-M-n` / `S-M-p`, or `]d` / `[d` | Next, previous problem |

`editor-format-on-save #true` formats on every save. Problems are underlined and marked in the gutter; the status bar counts them. `Editor: Toggle Inlay Hints` hides or shows the dim type and parameter hints.

## Completion

The completion popup opens with `C-SPC`, or by itself after three letters or a `\`, `/`, `{`, or `#`. `C-n` and `C-p` move through the list. `Tab` or `RET` takes the highlighted entry, or the first when none is; `Esc` closes it. To have `RET` break the line instead, put `insert_newline "RET" in="editor" when="completion"` in `keybindings.kdl`.

Entries come from the language server, nearby words, file paths, snippets, keywords, and LaTeX and Markdown helpers. Choose which a language uses with `completion { backends { rust "snippets" "keywords" "words" } }`, and turn off the automatic popup with `completion { auto #false }`. Your own snippets go in `snippets/<language>/` beside `settings.kdl`, one file per snippet.

## GitHub Copilot

Copilot ships as an extension and is on by default (`copilot { enable }`).

- `Copilot: Sign In` signs in through the browser.
- Its suggestion shows dim at the cursor. `Tab` (or `C-RET`) takes it all, `M-f` its next word, `C-.` its next line.
- `M-n` / `M-p` show other suggestions, `C-g` hides it, and `C-\` asks for one.

## Markdown and LaTeX

| Markdown | Action |
|---|---|
| `C-c C-n` / `C-c C-p` | Next, previous heading |
| `C-c C-u` | Parent heading |
| `C-c -` / `C-c =` | Promote, demote the heading or list item |
| `C-c b` / `C-c i` / ``C-c ` `` | Bold, italic, inline code |
| `C-c x` | Toggle the task box |
| `C-c n` | Renumber the list |
| `M-RET` | New list item or heading below |
| `C-c w` | Count the words |

| LaTeX | Action |
|---|---|
| `C-c C-n` / `C-c C-p` | Next, previous section |
| `C-c C-e` | Start an environment |
| `C-c e` | Rename the enclosing environment |
| `C-c b` / `C-c i` | `\textbf`, `\textit` |
| `C-c C-f` then a letter | `\textbf`, `\textit`, `\emph`, `\texttt`, `\textsf`, `\textsc` |
| `C-c w` | Count the words with `texcount` |

Markup markers (`**`, `\textbf{`, a heading's `#`s) are hidden and their text drawn as it reads. `editor-markup-markers "show-on-cursor"` shows them on the cursor's line, and `"visible"` always.

## Outline

`C-S-o` lists the file's headings, sections, or blocks, each with its line number, in Markdown, LaTeX, HTML, XML, reStructuredText, JSON, YAML, TOML, INI, KDL, nginx, SSH configuration, and Swift files. In code it lists the functions, types, and modules the file defines. The cursor follows the entry you pick and goes back if you cancel. `Sidebar: Outline` keeps the same list in a sidebar (see [Panels](basics.md#sidebars)).

## Spelling

Prose is checked against an American English dictionary: LaTeX, BibTeX, and Markdown text as they open, a program's comments with `M-S-i`.

- `]s` / `[s` go to the next, previous misspelling.
- `z=` suggests spellings; `zg` adds the word to `spelling/words.txt`.

## Merge conflicts

In a file with conflict markers, `M-n` and `M-p` step between conflicts. With the cursor in one:

| Key | Keeps |
|---|---|
| `C-c o` / `C-c t` | Ours, theirs |
| `C-c b` / `C-c r` | Both, ours first or theirs first |
| `C-c a` / `C-c d` | The ancestor, neither |
| `C-c m` | A list of every choice |

## Comparing

`Diff: Compare Buffer with File on Disk`, `…with Clipboard`, `…with Another Open Buffer…`, `…with File…`, and `…with Revision…` open the [Diff](diff.md) editor.

## Sending code to a terminal

`C-S-RET` runs the selection, or the line, in a terminal beside the editor, for a REPL or a shell. With nothing selected it moves on to the next line, so pressing it again walks through a script.

## Writing help

- `M-s i` inserts a math or Greek symbol.
- `M-s e`, `M-s ]`, `M-s [` go to misspellings across the whole project.
- `S-M-t` looks the word up in your GoldenDict dictionary.

## Display

- **Wrapping:** long lines wrap at words; `M-z` turns it off for the tab. `editor-wrap-indent` sets the indent of wrapped rows.
- **Sticky scroll:** the function or heading you are inside stays pinned at the top. `Editor: Toggle Sticky Scroll` turns it off.
- **Scroll margin:** `scroll-margin` rows (5 by default) stay above and below the cursor, like Vim's `scrolloff`.
- **Colors:** `#rrggbb` colors are drawn on that color. `Editor: Toggle Color Swatches` turns it off.
- **Git changes:** the gutter marks added, changed, and deleted lines since the last commit.

## Files

- Saving keeps the file's permissions, line endings, and final newline.
- A file changed by another program is reloaded if you have no edits, your cursor staying on its line. With edits, they are kept, a notice says the file changed, and saving asks before overwriting.
- Every save keeps a copy; `File: Local History…` lists them so you can compare and take changes back.
- The editor refuses files that are binary, not UTF-8, or over four megabytes; open them in [Hex](viewers.md#hex).
- `File: Open Scratchpad` opens a scratch file for notes.
