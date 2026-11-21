# Todo

The Todo page lists the comments across the project that hold a todo keyword, under the file each is in. Open it with `Project: List Todos`.

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
