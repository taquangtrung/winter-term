# Modes

Every terminal tab is in one of three modes, and each tab keeps its own.

| Mode | Who gets the keys | Enter it with |
|---|---|---|
| **Insert** | The program, as in any terminal | `i`, `a`, or `o` from Normal |
| **Normal** | Winter: motions, operators, layout | `Esc` or `M-ESC` from Insert |
| **Visual** | Normal, but motions extend a selection | `v`, `V`, or `C-v` from Normal |

Insert is the default, so Winter acts like a plain terminal until you leave it. Normal mode works with any shell, over `ssh`, and in a REPL, with nothing installed on the far end.

## Getting into Normal mode

Press `Esc`, or `M-ESC` anywhere. Programs with modes of their own keep `Esc`: in Vim, Neovim, Helix, Kakoune, Emacs, or while a tab completion is open, only `M-ESC` enters Normal mode.

## Getting out

`M-ESC`, `i`, `a`, or `o` return to Insert. `Esc` in Normal mode first clears what Winter holds (a half-typed command, a search, a selection); once nothing is left, it goes to the program. So to send `Esc` to a program such as Claude Code, press `Esc` twice.

## Editing the shell's command line

In Normal mode, operators on the line the shell is editing become readline keys, so `dw` on the prompt deletes a word in your shell. `C-/` undoes your prompt edits and `C-\` redoes them.

This assumes your shell uses its default Emacs keys. If your shell is in vi mode (`bindkey -v`, `set editing-mode vi`), set `prompt-edit-bindings "none"` and let the shell do the editing.

The editors have modes of their own, shown in the status bar; see each tool's page.
