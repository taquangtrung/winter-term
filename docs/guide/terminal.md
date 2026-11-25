# Terminal

## Shell integration

Blocks, exit codes, folding, and the working directory come from marks your shell emits. Add one line to your shell's rc file:

```bash
# bash
source /usr/share/winter-term/shell-integration/winter.bash

# zsh
source /usr/share/winter-term/shell-integration/winter.zsh

# fish
source /usr/share/winter-term/shell-integration/winter.fish
```

Without it Winter still works, but the whole session is one block: no per-command blocks, exit codes, or folding.

`terminal-gutter` adds a column beside a terminal showing each command's exit status. `Terminal: Toggle Gutter` turns it on and off.

## Rich blocks

Programs can print images, SVG, charts, Markdown, HTML, tables, and JSON, which Winter draws inline. Every block also carries plain text, so the same program reads fine under `tmux`, `ssh`, or in CI.

| Content | Drawn as |
|---|---|
| PNG, JPEG, GIF, WebP | The image |
| SVG | The drawing, sharp at any size |
| Vega-Lite | A bar, line, area, or point chart |
| HTML, Markdown | Rich text and tables |
| CSV, JSON | A table, pretty-printed JSON |
| Plain text | The text |

Nothing in a block is run or fetched. The protocol is in [`terminal-block-protocol.md`](../reference/terminal-block-protocol.md), and client libraries are under `clients/`.

## Multiplexer

`winter mux` keeps terminal sessions running after the window closes:

```bash
winter mux serve                         # run the session server
winter mux list                          # list sessions
winter mux new <name>                    # create one
winter mux attach <name>                 # attach to one
winter mux attach --host <host> <name>   # attach to one on another machine, over ssh
winter mux kill <name>                   # end one
```

A session's size follows its smallest attached window.
