# winter-render

The terminal screen model: a 2D cell grid driven by VT sequences, plus the GPU renderer that draws it.

`Grid` holds styled cells and a cursor. `Screen` drives it from a byte stream via `vte`, covering printing, cursor motion, SGR colors, erase, and scroll. `renderer::GpuRenderer` draws a `Grid` to a wgpu surface, using `cosmic-text` and `glyphon` for glyph rendering.

```rust
use winter_render::Screen;

let mut screen = Screen::new(80, 24);
screen.feed(b"\x1b[1;32mgreen\x1b[0m\r\n");

println!("{}", screen.grid().to_text());
```

This crate is the CPU side of the text grid and does not own a window or an event loop. It is usable on its own for anything that needs VT semantics over a cell buffer, with or without the GPU renderer.

Scrollback is capped at `MAX_SCROLLBACK` (10,000 rows) so history cannot grow without bound.

## License

MIT. See [LICENSE](LICENSE).
