# winter-term

The native application crate for Winter, a web-native terminal emulator.

> **Early development.** Not production-ready; APIs and behaviour may change without notice.

## What this crate is

`winter-term` is the top-level package. It wires the supporting crates into a runnable terminal and ships two targets:

- the **`winter` binary**, the entry point, which runs in window mode, headless demo mode, or as a multiplexer server;
- the **`winter_app` library**, holding `App`, layout primitives, input and action types, interaction modes, pane management, and KDL config parsing.

The `winter_app` library is published because the binary needs it, not because it is a stable API. It carries no semver guarantee: treat it as an implementation detail of the application and pin an exact version if you depend on it.

The heavy lifting lives in the supporting crates:

| Crate | Role |
|---|---|
| [`winter-proto`](../winter-proto) | TBP wire types and OSC codec |
| [`winter-core`](../winter-core) | PTY driver, `vte` parser, block-list scrollback |
| [`winter-render`](../winter-render) | VT cell grid and the wgpu text renderer |

The multiplexer is not a separate crate; it is the `mux` module inside this one.

## Install

```bash
cargo install winter-term
```

The binary is called `winter`, not `winter-term`.

Requires Rust 1.96 or newer and a C toolchain. On Linux, `libgtk-3-dev` and `libwebkit2gtk-4.1-dev` are needed for WebView tile support, which is what renders rich blocks.

Prebuilt packages (`.deb`, AUR, Scoop, winget, `.dmg`) are usually a better path than `cargo install`, which produces a bare binary with no desktop entry or icon. See the [repository](https://github.com/taquangtrung/winter-term) for those.

## Usage

```bash
# Open a GPU-accelerated terminal window:
winter

# Run a command headlessly and print the parsed block list:
winter bash -c 'echo hello'

# Multiplexer subcommands:
winter mux serve
winter mux attach default
```

Configuration lives in `~/.config/winter-term/`, split across `settings.kdl` and `keybindings.kdl`. The commented samples in [`samples/`](samples) are the defaults, and `keybindings.kdl` is parsed through the same pipeline that reads yours.

## License

MIT. See [LICENSE](LICENSE).
