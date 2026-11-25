# Compile

Compile (`C-S-b`, or `Compile: Run a Task…`) runs a build and turns every place its output points at into a link.

- With one task, `C-S-b` runs it at once; with several, it lists them, the last one run first. `Compile: Run a Command…` asks for any command.
- The build shows in the bottom panel's Output view, and your focus stays where it was; its errors and warnings go to the Problems view (see [Panels](basics.md#the-bottom-panel)).
- `l` or `RET` on a build in Output reads the whole of it there, following its end until you move up; `G` follows again. `t` shows it in a tab among the panes, with the keys below.
- A build stays in Output when its tab closes, with its problems; `d` on it there removes it. Output keeps the 20 newest finished builds.
- Open files are saved before a build starts.
- When a build fails, a card says how many errors and warnings it had, and its first error opens. `compile { jump-to-error #false }` stops the jump.
- `Compile: Run the Last Build Again` reruns the newest build from anywhere.

## Keys

| Key | Action |
|---|---|
| `Enter` or a double click | Open the place on this line |
| `C-o` | Open it in `$EDITOR` |
| `y` | Copy its `path:line` |
| `M-n` / `M-p` | Next, previous error or warning |
| `e` | Show only errors and warnings, or everything |
| `C-r` | Run again |
| `c` | Run another command here |
| `i` | Type a line to the running build |
| `C-g` | Stop the build |
| `C-c C-v` | Open the PDF a LaTeX build made |

`F8` and `S-F8` step through the build's places from any tab.

## Tasks

The tasks offered are the `.tex` document you are in, then `.winter/tasks.kdl` in the project's root, then what the project's files suggest (`cargo`, `make`, `go`, `package.json` scripts, `latexmk`).

```kdl
task "build" "cargo build --workspace" default=#true
task "check" "cargo check" on-save=#true
task "docs" "make html" cwd="docs" {
    env SPHINXOPTS="-W"
}
```

`default=#true` is the task the project builds with; `on-save=#true` runs on every save.

## LaTeX

`LaTeX: Compile Document` builds the document with `latexmk`, and lists LaTeX's warnings and overfull boxes after the output. A PDF tab showing the document reloads by itself. In the `compile` block of `settings.kdl`, `latex-command` sets the command and `latex-on-save` rebuilds on save.
