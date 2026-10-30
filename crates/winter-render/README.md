# winter-render

The GPU renderer: draws a `winter_core::Grid` and the window chrome around it (tabbar, pane strips, status bar, palette, toasts) to a wgpu surface, using `cosmic-text` and `glyphon` for glyph rendering.

The grid and the VT parsing that drives it live in [`winter-core`](../winter-core); this crate only draws them. It does not own a window or an event loop: the app hands it plain view structs (`PaneView`, `StatusBar`, `PaletteView`, ...) each frame.

## License

MIT. See [LICENSE](LICENSE).
