# Keyboard Shortcuts

## The Keyboard Shortcuts page

`Preferences: Keyboard Shortcuts` lists every command and every editor's own keys: each command's name, its keys, whether the binding is built in or yours, and its id. It reads the keys in force, so it always matches what the keys do.

| Key | Action |
|---|---|
| `j` / `k` | Next, previous command |
| `gg` / `G` | First, last command |
| `Enter` | Run the command |
| `i` | Filter by name, id, or key; `Esc` leaves the filter |
| `M-k` | In the filter, record keys to find what they run |
| `c` | Give the command new keys |
| `a` | Add another key |
| `S-c` | Give it keys on this platform only |
| `d` | Take its keys away |
| `x` | Reset it to the built-in keys |

The footer shows where the selected command runs, where its keys come from, and any command sharing its keys; rest the pointer on another command to see its. Right-click a command, or press its key from the table above, to run it, change or add keys, or take them away. To give new keys, press them (a sequence like `M-g o` works), then `Enter` to save, `BS` to take the last key back, or `Esc` to cancel. A key another command in the same place already uses is shown in amber.

Every change is written to `keybindings.kdl`, leaving the rest of the file as you wrote it. Vim's motions are listed for reference but are built in.

## Describe Key

`Help: Describe Key` tells you what a key does without doing it: run it, then press a key. `C-,` says `C-, runs Preferences: Settings`; in an editor, its own keys say what they do there.
