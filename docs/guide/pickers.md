# Pickers

Every list Winter offers is a picker: the command palette, the file finders, project search, PDF text search, the recent list, the buffer list, Scout, the outline, and every question with a list of answers. A picker opens at once and fills as rows arrive.

## Typing a query

The query is words separated by spaces, matched in any order: `pick rs` finds `src/picker/mod.rs`.

- A word matches anywhere, in any case.
- `/like this/` is a regular expression.
- `!test` leaves out rows holding `test`.
- `"fn main"` matches the text exactly, spaces included.

## Keys

| Key | Action |
|---|---|
| `Enter` | Run the selected row |
| `F4` or `C-Enter` | Show the row's other actions |
| `F6` | Show or hide the preview beside the list |
| `C-S-s` | Save the rows to a list of their own |
| `C-n` `C-p`, `Down` `Up` | Move the selection |
| `M-p` `M-n` | An earlier, a later query |
| `Esc` or `C-g` | Close |

`Picker: Resume Last Session` brings back the last picker with its query and selection. Picker keys can be rebound under the `picker` block of `keybindings.kdl`.

## Finding things

| Keys | Picker |
|---|---|
| `M-x` | Command palette |
| `C-M-z` | Files in the project |
| `M-S-z` | Files under the working directory |
| `C-M-g` | Search the project as you type |
| `C-S-z` | Recent tabs, buried buffers, files, and directories |
| `C-S-f` | Browse files (see [Files](files.md)) |
| `C-S-o` | The file's outline (see [Text editor](text-editor.md#outline)) |

A **project** is the nearest folder at or above the tab's directory that holds a `.winter` folder or a git repository. Its files are the ones git lists, minus ignored ones.

**Project search** (`C-M-g`) runs ripgrep once you pause on two or more characters.

- The pattern is searched as written; `*` stands for anything.
- Case is ignored unless the pattern has a capital.
- ` @ ` starts a file filter: `foo @ src/*.rs`.
- `M-%` replaces the pattern in every line listed, after asking what with. A line changed since the search is left alone.
- `C-S-s` keeps the results in a list; `Enter` on a line opens it there.

**PDF text search** (`PDF: Search Text`) finds text inside the project's PDFs as you type. `Enter` opens the PDF at the page.

**Buffer: Switch to…** lists every buffer: terminals, buried buffers and terminals, then shown ones. `[+]` marks unsaved edits.

**Scout** (`Scout: This Tab`, or `gs` in a terminal's Normal mode) searches the tab in front as you type, showing each match in place. `Enter` goes there, `C-o` comes back, and `Esc` puts the pane back.

**Todo** (`Project: Find Todo…`) lists the TODO comments across the project; see [Todo](todo.md) for the page that lists them.

**Keys of this tab** (`Help: Keys of This Tab`) lists the keys of the tab in front.
