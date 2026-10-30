# winter-core

A terminal that parses a session into a block list.

The pipeline is byte stream to `Terminal` (a `vte` parser) to `Scrollback` (a list of `CommandBlock`s). Scrollback is a block list rather than a flat line buffer: OSC 133 marks delimit per-command blocks, and OSC 9001 (TBP) emits rich content blocks nested inside a command's output.

```rust
use winter_core::{Segment, Terminal};

// What a shell with prompt marks writes for `echo hello`.
let mut terminal = Terminal::new();
terminal.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07echo hello\r\n\x1b]133;C\x07hello\r\n\x1b]133;D;0\x07");
for block in terminal.scrollback().blocks() {
    println!("{} -> {:?}", block.command, block.exit_code);
    for segment in &block.output {
        if let Segment::Text(text) = segment {
            print!("{text}");
        }
    }
}
```

Each `CommandBlock` carries the command line, its working directory (OSC 7), its exit code (OSC 133 `D`), and an ordered list of `Segment`s: plain text, an OSC 8 hyperlink span, a one-shot TBP block, or a live block updated in place by RFC 6902 patches.

The crate runs no program: bytes come from wherever the caller reads them, a PTY in Winter. `Scrollback` is independent of `vte` too, so it can be driven directly from tests; `Performer` is the adapter that turns escape sequences into its method calls.

## The screen model

`Grid` holds styled cells and a cursor. `Screen` drives it from a byte stream via `vte`, covering printing, cursor motion, SGR colors, erase, and scroll. `winter-render` draws a `Grid`; nothing here depends on a GPU or a window.

```rust
use winter_core::Screen;

let mut screen = Screen::new(80, 24);
screen.feed(b"\x1b[1;32mgreen\x1b[0m\r\n");

println!("{}", screen.grid().to_text());
```

## Bounded by construction

A terminal stays open for days, so retention is capped: the block list holds 8 MiB of output, a live block folds each patch in as it arrives rather than keeping it, and the grid keeps at most `MAX_SCROLLBACK` (10,000) rows of history. Blocks past the budget are elided rather than removed, because a block's index is a stable identifier that callers hold.

## License

MIT. See [LICENSE](LICENSE).
