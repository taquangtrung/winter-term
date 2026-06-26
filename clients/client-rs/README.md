# winter-client (Rust)

Emit rich Terminal Block Protocol (TBP) blocks from Rust, built on the same [`winter-proto`](../../crates/winter-proto)
codec the terminal itself decodes with. Falls back to `text/plain` when Winter
is not the active terminal, so programs using this crate stay safe under
tmux, ssh, and CI.

```rust
use winter_client::{display_markdown, live_block, DisplayOptions};

display_markdown("# hello", DisplayOptions::default())?;

let mut block = live_block("text/markdown", "# progress: 0%")?;
block.update("# progress: 50%")?;
block.close()?;
```

## Develop

```bash
cd clients/client-rs
cargo test
```
