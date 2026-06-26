# winter-core

A PTY-driven terminal that parses a session into a block list.

The pipeline is byte stream to `Terminal` (a `vte` parser) to `Scrollback` (a list of `CommandBlock`s). Scrollback is a block list rather than a flat line buffer: OSC 133 marks delimit per-command blocks, and OSC 9001 (TBP) emits rich content blocks nested inside a command's output.

```rust
use portable_pty::CommandBuilder;
use winter_core::{run_to_completion, Segment};

let mut command = CommandBuilder::new("bash");
command.arg("-c");
command.arg("echo hello");

let terminal = run_to_completion(command)?;
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

`Scrollback` is independent of `vte` and the PTY, so it can be driven directly from tests; `Performer` is the adapter that turns escape sequences into its method calls.

## Bounded by construction

A terminal stays open for days, so retention is capped: the block list holds 8 MiB of output and a live block holds 10,000 patches. Blocks past the budget are elided rather than removed, because a block's index is a stable identifier that callers hold.

## License

MIT. See [LICENSE](LICENSE).
