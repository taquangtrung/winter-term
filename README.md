# Winter

A web-native terminal: native-class text speed, with output modeled as a sequence of typed, MIME-tagged **blocks** that a real web engine can render inline (tables, charts, math, PDFs, images), all with a `text/plain` fallback everywhere else.

See [`docs/usage-guide.md`](docs/usage-guide.md) for how to use Winter, and [`docs/terminal-block-protocol-spec.md`](docs/terminal-block-protocol-spec.md) for the protocol (TBP).

## A vim-based terminal that works with any shell

The modal layer lives in the terminal, not in your shell's line editor. Normal mode, motions, search, registers, and marks work identically in bash, zsh, fish, a Python REPL, or over `ssh` to a machine you cannot configure. No plugins, no `bindkey`, nothing to install on the far end.

That is the difference from shell-side vi mode (`bindkey -v`, `set editing-mode vi`), which only exists where you have configured it, and from the copy modes in Alacritty and WezTerm, which you enter to grab some text and then leave. Winter's Normal mode is a persistent mode with the actual vim vocabulary:

- Operators and text objects: `d`, `c`, `y`, `v` composed with `iw`, `ap`, `i"`, `f{char}`, and counts.
- Registers, marks, the jumplist (`Ctrl-o`/`Ctrl-i`), the changelist (`g;`/`g,`), and dot-repeat (`.`).
- Regex search with `/` and `?`, `n`/`N`, and search-motion operators.
- Blockwise visual mode, surround operators, and `gv`.
- Prompt-line editing: the same operators act on the shell's current command line.
- Which-key hints for pending prefixes, so the bindings are discoverable rather than memorized.

Four modes, not one transient one: Normal owns the keyboard, Insert owns the PTY, Visual extends a selection, and Block-Focus hands keys to a rich block. Full keymap in [the usage guide](docs/usage-guide.md#keybinding-reference).

**Blocks, not a flat scrollback.** OSC 133 marks turn each command into a navigable, foldable, exit-code-tagged unit; TBP layers typed rich content on top of that. Blocks need [shell integration](#shell-integration), one line in your rc file.

## Install

Every release attaches installers for all three platforms to its [GitHub Release](https://github.com/taquangtrung/winter-term/releases).

| Platform | Install |
|---|---|
| **Linux** | Download the `.deb` and `sudo dpkg -i winter-term_<version>_amd64.deb` |
| **Arch** | `yay -S winter-term` (or any AUR helper) |
| **Windows** | Download and run `winter-terminal-<version>-setup.exe`, or `scoop install winter-term`, or `winget install QuangTrungTa.WinterTerminal` |
| **macOS** | Download the `.dmg` and drag `Winter.app` to Applications |

Rust developers can also `cargo install winter-term`, but it is the least pleasant path: it compiles 311 dependencies, needs Rust 1.96 or newer, needs the Linux development packages below, and installs a bare binary with no desktop entry or icon. Prefer a real package.

On Linux, building needs GTK 3 and WebKit2GTK development packages: `libgtk-3-dev`, `libwebkit2gtk-4.1-dev`, `libsoup-3.0-dev`, `libjavascriptcoregtk-4.1-dev`, and `libxdo-dev` on Debian and Ubuntu.

### macOS notes

The `.dmg` is built on Apple silicon and is **arm64 only**; Intel Macs need a source build for now. The app is **not code-signed or notarized**, so the first launch needs a right-click then "Open" to get past Gatekeeper, or:

```bash
xattr -dr com.apple.quarantine /Applications/Winter.app
```

For the `winter` command on your `PATH`:

```bash
sudo ln -sf /Applications/Winter.app/Contents/MacOS/winter /usr/local/bin/winter
```

## Platform support

| Platform | Status | Packaging |
|---|---|---|
| Linux (X11/Wayland) | Primary, developed and tested here | `.deb`, AUR |
| Windows | Supported, tested in CI | Inno Setup `.exe`, Scoop, winget |
| macOS | Builds and tests in CI, not regularly exercised | `.dmg` (arm64, unsigned) |

Flatpak is not offered yet. A sandboxed terminal has to spawn shells on the host rather than inside the sandbox, which needs an application change (`flatpak-spawn --host`), not just a manifest.

## Shell integration

Command blocks, exit codes, and directory tracking come from OSC 133 and OSC 7 marks that your shell emits. Winter works without them, but the whole session is then one rolling block: no per-command navigation, no folding, no exit codes. Add one line to your rc file:

```bash
# ~/.bashrc  (or winter.zsh in ~/.zshrc, winter.fish in ~/.config/fish/config.fish)
[ -r /usr/share/winter-term/shell-integration/winter.bash ] && \
    . /usr/share/winter-term/shell-integration/winter.bash
```

Each script no-ops outside Winter and re-sources safely, so it is fine to add unconditionally. If you already source another terminal's OSC 133 integration (kitty, WezTerm, iTerm2), Winter reads those marks too and you can skip this.

The scripts live in `/Applications/Winter.app/Contents/Resources/shell-integration/` on macOS, `<install dir>\shell-integration` on Windows, and `clients/shell-integration/` in a source checkout.

## Security model for rich blocks

Anything that can write to a PTY can emit a TBP block: a `cat` of a downloaded file, output piped from `curl`, a program on the far side of `ssh`. Nothing on the wire authenticates the emitter, so a block's requested trust tier is treated as a ceiling to clamp, never a grant.

Two settings govern this, both deny-by-default:

```kdl
security {
    // Ceiling applied to the tier a block asks for. "restricted" (the default)
    // renders under a CSP with scripting off. Raising this to "trusted" grants
    // scripting to *any* stream reaching a pane, not just to tools you trust:
    // the terminal cannot tell them apart.
    block-max-trust "restricted"

    // Let block content load subresources from the network, which live Vega
    // charts need. Off by default, so rendering a block never makes a request
    // you did not ask for.
    block-remote-assets #false
}
```

`clipboard-read` (OSC 52 clipboard reads) is likewise opt-in. See [`docs/usage-guide.md`](docs/usage-guide.md) for the full settings reference.

## Documentation

| Document | What is in it |
|---|---|
| [`docs/usage-guide.md`](docs/usage-guide.md) | The modes, the full keymap, every settings key, the multiplexer |
| [`docs/terminal-block-protocol-spec.md`](docs/terminal-block-protocol-spec.md) | TBP v1: framing, verbs, trust tiers |
| [`docs/architecture.md`](docs/architecture.md) | What each crate owns and how bytes become pixels |
| [`CONTRIBUTING.md`](CONTRIBUTING.md) | Building from source, running the demos, sending a patch |
| [`docs/releasing.md`](docs/releasing.md) | Maintainer runbook for cutting a release |
| [`SECURITY.md`](SECURITY.md) | Threat model and private disclosure |

## Contributing

Run `make lint` and `make test` before opening a pull request. See [CONTRIBUTING.md](CONTRIBUTING.md) for the details and [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) for how we treat each other.

## Security

Report vulnerabilities privately: see [SECURITY.md](SECURITY.md). Every byte arriving from a PTY is treated as attacker-controlled, so crashes and trust-tier escapes reachable from terminal output are in scope.

## License

MIT. See [LICENSE](LICENSE).

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work by you shall be licensed as above, without any additional terms or conditions.
