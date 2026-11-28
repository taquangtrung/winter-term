# Dir

Dir (`C-S-d`, or `Dir: Open Working Directory`) lists a directory as a foldable tree, starting where the pane was working. The path runs along the header; click a folder to list it. `../` goes up and `./` reads the listing again.

`Sidebar: Files` keeps a lighter tree of the project in a sidebar, for opening files; `t` there opens the project in Dir (see [Sidebars](basics.md#sidebars)).

## Keys

| Key | Action |
|---|---|
| `j` `k` / `Home` `End` | Move, first or last entry |
| `Enter` | Enter a directory, or open a file |
| `C-o` | Open the file in `$EDITOR` |
| `l` / `h` | Open the directory in place, close it |
| `Backspace` | Go up to the parent |
| `M-n` / `M-p` | Next, previous entry at the same level |
| `M-S-b` / `M-S-f` | Back, forward through directories visited |
| `Tab` / `S-Tab` | Fold the entry, fold everything |
| `z u` / `z f` / `z t` | Open, close, toggle the subtree |
| `C-c 0` to `C-c 9` | Open the tree to that depth |
| `.` | Show or hide dotfiles |
| `,` | Show or hide size, age, and permissions |
| `M-S-s` | Show directory sizes |
| `s` | Sort by name, time, size |
| `M-S-r` / `C-r` | Read the listing and its directory sizes again |
| `m` / `u` | Mark, unmark |
| `t` / `U` | Mark everything, unmark everything |
| `_` / `+` | New file, new directory |
| `R` | Rename |
| `C` / `M-m` | Copy, move |
| `x` / `X` | Move to the trash, delete for good (both ask first) |
| `*` | Set permissions, in octal |
| `&` | Open with the system's application |
| `=` | Compare the two marked entries |
| `a` | The actions menu |
| `Z` / `c` | Extract or compress, compress the marked entries into one archive |
| `C-x C-q` | Rename many entries by editing their names |

## Notes

- An operation acts on the marked entries, or on the entry under the cursor when nothing is marked.
- Copy and move offer the directory shown in another pane first.
- The gutter shows a check on a marked entry, else its git state.
- The **actions menu** (`a`) lists every action for the targets, each with one letter: `o` open, `e` open in its application, `h` open in Hex, `r` `c` `m` rename, copy, move, `d` `D` trash, delete, `p` permissions, `u` `z` extract, compress, and in a repository `M` `git mv` and `a` `git add -f`.
- **Renaming in place** (`C-x C-q`) makes every name editable text with the editor's keys. `ZZ`, `C-s`, or `Enter` applies the renames (asking first for more than one); `ZQ`, `q`, or `C-x C-q` again throws them away. Swapping two names works.
- `Dir: Open Two-Pane File Manager` opens two listings side by side: copy and move go to the other one, and `=` compares the two directories.
