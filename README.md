# Winter

> **Early development.** Winter is not production-ready. APIs, wire formats, and config schema may change without notice. Expect rough edges and missing features.

A web-native terminal: native-class text speed, with output modeled as a sequence of typed, MIME-tagged **blocks** that a real web engine can render inline (tables, charts, math, PDFs, images), all with a `text/plain` fallback everywhere else.

See [`docs/usage-guide.md`](docs/usage-guide.md) for how to use Winter, and [`docs/terminal-block-protocol-spec.md`](docs/terminal-block-protocol-spec.md) for the protocol (TBP).

## A vim-based terminal that works with any shell

The modal layer lives in the terminal, not in your shell's line editor. Normal mode, motions, search, registers, and marks work identically in bash, zsh, fish, a Python REPL, or over `ssh` to a machine you cannot configure. No plugins, no `bindkey`, nothing to install on the far end.

That is the difference from shell-side vi mode (`bindkey -v`, `set editing-mode vi`), which only exists where you have configured it, and from the copy modes in Alacritty and WezTerm, which you enter to grab some text and then leave. Winter's Normal mode is a persistent mode with the actual vim vocabulary:

- Operators and text objects: `d`, `c`, `y`, `v` composed with `iw`, `ap`, `i"`, `f{char}`, and counts.
- Registers, marks, the jumplist (`Ctrl-o`/`Ctrl-i`), the changelist (`g;`/`g,`), and dot-repeat (`.`).
- Regex search with `/` and `?`, `n`/`N`, and search-motion operators.
- Blockwise visual mode, surround operators, and `gv`.
- Prompt-line editing: the same operators act on the shell's current command line, with `Ctrl-/` and `Ctrl-\` for undo and redo of your edits.
- Which-key hints for pending prefixes, so the bindings are discoverable rather than memorized.

Three modes, not one transient one: a Normal mode that owns the keyboard, an Insert mode that owns the PTY, and a Block-Focus mode for rich content. Full keymap in [the usage guide](docs/usage-guide.md#keybinding-reference).

> **If your shell is already in vi mode, set `prompt-edit-bindings "none"`.** Prompt-line operators reach the shell as readline chords (`Ctrl-A`, `Ctrl-K`, `Ctrl-U`, `Ctrl-W`), which assumes the default emacs mode. A shell in vi mode (`bindkey -v`, `set editing-mode vi`) has those bound elsewhere, so that setting tells Winter to decline them and leave the line to your shell, which gives you Vim editing there anyway. Everything else, all navigation over the screen and scrollback, works the same either way.

**Blocks, not a flat scrollback.** OSC 133 marks turn each command into a navigable, foldable, exit-code-tagged unit; TBP layers typed rich content on top of that. Blocks need [shell integration](#shell-integration), one line in your rc file.

## Install

Every release attaches installers for all three platforms to its [GitHub Release](https://github.com/taquangtrung/winter-term/releases).

| Platform | Install |
|---|---|
| **Linux** | Download the `.deb` and `sudo dpkg -i winter-term_<version>_amd64.deb` |
| **Arch** | `yay -S winter-term` (or any AUR helper) |
| **Windows** | Download and run `winter-terminal-<version>-setup.exe`, or `scoop install winter-term`, or `winget install QuangTrungTa.WinterTerminal` |
| **macOS** | Download the `.dmg` and drag `Winter.app` to Applications |

Building from source, `make package` produces the right artifact for whatever OS you are on, and `make install` installs it.

Rust developers can also `cargo install winter-term`, but it is the least pleasant path: it compiles 311 dependencies, needs Rust 1.96 or newer, needs the Linux dev packages listed below, and installs a bare binary with no desktop entry or icon. Prefer a real package.

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

Flatpak is not offered yet. A sandboxed terminal has to spawn shells on the host rather than inside the sandbox, which needs an application change (`flatpak-spawn --host`), not just a manifest. Tracked in [`docs/TODOs.md`](docs/TODOs.md).

## Building from source

### Prerequisites

- **Rust** stable, 1.96 or newer. This is the floor CI enforces, and the dependency graph alone requires 1.95.
- **uv**, for the Python client tests. Optional.
- On Linux: GTK 3 and WebKit2GTK development packages (`libgtk-3-dev`, `libwebkit2gtk-4.1-dev`, `libsoup-3.0-dev`, `libjavascriptcoregtk-4.1-dev`, `libxdo-dev` on Debian/Ubuntu).

### Quick start

```bash
make build        # build all Rust crates
make rust-test    # the Rust test suite
make test         # everything: Rust + Python
make lint         # clippy (deny warnings) + rustfmt check
make help         # all targets
```

## Shell integration

Command blocks, exit codes, and directory tracking come from OSC 133 and OSC 7 marks that your shell emits. Winter works without them, but the whole session is then one rolling block: no per-command navigation, no folding, no exit codes. Add one line to your rc file:

```bash
# ~/.bashrc
[ -r /usr/share/winter-term/shell-integration/winter.bash ] && \
    . /usr/share/winter-term/shell-integration/winter.bash
```

```zsh
# ~/.zshrc
[ -r /usr/share/winter-term/shell-integration/winter.zsh ] && \
    . /usr/share/winter-term/shell-integration/winter.zsh
```

```fish
# ~/.config/fish/config.fish
test -r /usr/share/winter-term/shell-integration/winter.fish
    and source /usr/share/winter-term/shell-integration/winter.fish
```

Running from a source checkout, point those at `clients/shell-integration/` instead. On macOS the scripts live inside the bundle, at `/Applications/Winter.app/Contents/Resources/shell-integration/`; on Windows, in `<install dir>\shell-integration`. Each script no-ops outside Winter and re-sources safely, so it is fine to add unconditionally. If you already source another terminal's OSC 133 integration (kitty, WezTerm, iTerm2), Winter reads those marks too and you can skip this.

## Try it

### 1. The integrated native pipeline (headless, no display needed)

The `winter` binary runs a command under a PTY and prints **both** views of its output: the live screen grid (`render` crate) and the parsed block list (`core`).

```bash
make demo CMD='ls -la'
# or directly:
cargo run -p winter-term -- bash -c 'echo hi; printf "\033[1;32mgreen\033[0m\n"'
```

### 2. Emit a rich block from a tool, watch the core parse it

The `dump_session` example runs a command under a PTY and prints the parsed `CommandBlock` list. Let the command itself emit a TBP block (so it rides the PTY the example reads), with `WINTER=1` to enable emission:

```bash
# Shell client -> SVG block:
printf '<svg width=10/>' > /tmp/plot.svg
cargo run -p winter-core --example dump_session -- \
  bash -c "WINTER=1 $PWD/clients/client.sh svg /tmp/plot.svg"

# Python client -> SVG block:
cargo run -p winter-core --example dump_session -- \
  bash -c "WINTER=1 PYTHONPATH=$PWD/clients/client-py/src python3 -c \
    'import winter; winter.display_svg(\"<svg width=10/>\", text=\"fallback\")'"
```

The emitted SVG appears as a `Content` block with its MIME bundle and a `text/plain` fallback. (Without `WINTER=1`, the clients print the plain-text fallback instead, the safe degradation path.)

## Security model for rich blocks

Anything that can write to a PTY can emit a TBP block: a `cat` of a downloaded file, output piped from `curl`, a program on the far side of `ssh`. Nothing on the wire authenticates the emitter, so a block's requested trust tier is treated as a ceiling to clamp, never a grant.

Two settings govern this, both deny-by-default:

```kdl
security {
    // Ceiling applied to the tier a block asks for. "restricted" (the
    // default) renders under a CSP with scripting off. Raising this to
    // "trusted" grants scripting to *any* stream reaching a pane, not just
    // to tools you trust: the terminal cannot tell them apart.
    block-max-trust "restricted"

    // Let block content load subresources from the network. Needed for live
    // Vega and Vega-Lite charts, which pull their runtime from a CDN. Off by
    // default so rendering a block never makes a request you did not ask for.
    block-remote-assets #false
}
```

`clipboard-read` (OSC 52 clipboard reads) is likewise opt-in. See [`docs/usage-guide.md`](docs/usage-guide.md) for the full settings reference.

## Contributing

Run `make lint` and `make test` before opening a pull request. Notable changes go in [`CHANGELOG.md`](CHANGELOG.md) under `Unreleased`.

CI runs the Linux jobs (rustfmt, clippy, tests, MSRV, the Python client, and a crates.io publish dry run) on every push and pull request. The Windows and macOS test jobs run weekly and on demand from the Actions tab, because a macOS runner bills at ten times a Linux one; if you touch anything platform-specific, trigger a manual run.

Crates publish in dependency order, since a `cargo publish` resolves path dependencies against the real index: `winter-proto`, then `winter-render`, `winter-core`, `winter-client`, and `winter` last. Bump the version in exactly two places in the root `Cargo.toml`: `[workspace.package].version` and each internal entry under `[workspace.dependencies]`.

### Cutting a release

1. Bump the version in the root `Cargo.toml` (both places above) and run `cargo check` so `Cargo.lock` follows.
2. Move the `Unreleased` items in [`CHANGELOG.md`](CHANGELOG.md) under a new `## [x.y.z]` heading. The release workflow uses that section verbatim as the release body, so write it for users.
3. Commit, then `git tag vx.y.z && git push origin vx.y.z`.
4. [`.github/workflows/release.yml`](.github/workflows/release.yml) builds the `.deb`, `.dmg`, and `.exe` and opens a **draft** release with them attached. Review and publish it.
5. Update the downstream manifests against the published assets: `packaging/aur/PKGBUILD` (see its header), `packaging/scoop/winter-term.json` (version plus the installer's SHA256), and winget (see `packaging/winget/README.md`).

`workflow_dispatch` runs the same build without cutting a release, for rehearsing a change to the packaging.

## License

MIT. See [LICENSE](LICENSE).

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work by you shall be licensed as above, without any additional terms or conditions.
