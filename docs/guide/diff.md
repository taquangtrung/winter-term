# Diff

Diff compares two texts side by side, three against the middle one, or merges two into a third, in a tab of its own. It opens from:

- the text editor's `Diff:` commands;
- `=` on two or three marked files in Dir;
- `=` on a file in Git (the last commit against your working copy);
- `Merge: Merge Another File into This One…`;
- `Merge: Resolve Conflicts in a 3-Way Merge`, for a file with conflicts.

Each text has its color: A red, B green, the third blue. The current region is marked in the gutter, with its changed words drawn louder.

| Key | Action |
|---|---|
| `n` or `] c` / `p` or `[ c` | Next, previous region |
| `J` | Jump to a region by number |
| `j` `k` `C-d` `C-u` `gg` `G` | Scroll every text together |
| `<` / `>` | Scroll sideways |
| `a` / `b` | Copy the region from A to B, from B to A (`ab`, `ac`, and so on with three) |
| `ra` / `rb` / `rc` | Put a region back as it was |
| `+` / `o` / `r` | In a merge: keep both, the ancestor's, the original |
| `$$` | Step through the conflicts left |
| `##` / `#c` | Ignore whitespace, case |
| `!` | Compare again |
| `@` / `\|` / `~` | Hide changed words, stack the texts, swap A and B |
| `ea` / `eb` / `ec` | Edit a text in place, until `C-c C-c` |
| `wa` / `wb` / `wc` | Write a text back to its file |
| `D` | Show a unified diff |
| `q` | Quit, asking about unwritten texts |
