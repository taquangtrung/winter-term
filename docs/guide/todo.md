# Todo

The Todo page lists the comments across the project that hold a todo keyword, under the file each is in. The keyword counts when it opens the comment (`// TODO x`, a block comment's inner line), or anywhere in it with `:` or `(` after it (`// slow; FIXME: cache it`, `TODO(me)`). A keyword in code, in a string, or as a plain word of a sentence (`// the TODO list`) is left out. Comments are read as the text editor colors them, so a file it does not color holds none. Open it with `Project: List Todos`. `Sidebar: Todo` keeps the same comments in a sidebar (see [Sidebars](basics.md#sidebars)).

| Key | Action |
|---|---|
| `j` `k` | Next, previous comment |
| `J` `K` or `M-n` `M-p` | First comment of the next, previous file |
| `g g` `G` | First, last comment |
| `Enter` | Open the comment's line |
| `o` | Open it beside |
| `]` `[` | Only the next, previous keyword |
| `r` | Look again |

Each comment shows its keyword as a badge and the words after it. The footer shows the selected comment's file, line, and the line as written.

The tabs at the top count the comments for each keyword; choose one to see only those. The page looks again each time it comes back into view.

Set the keywords with `todo-keywords "TODO" "FIXME" "NEXT"` in `settings.kdl`. The text editor colors the same keywords.

`Project: Find Todo…` lists the same comments in a picker; see [Pickers](pickers.md).
