# Changelog

All notable changes to Winter are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

While the major version is `0`, the TBP wire format, the KDL config schema, and every crate's public API may change in a minor release.

## [0.1.0]

First release. Winter is a terminal emulator that models a session as a list of typed, MIME-tagged blocks rather than a flat scrollback, with a persistent Vim layer that works against any shell.

### Added

- **Terminal Block Protocol (TBP) v1**: one OSC 9001 escape carrying a MIME bundle, so a program can hand the terminal structured content while staying invisible to terminals that do not implement it. Every bundle carries a `text/plain` fallback, which is what keeps the same program readable under `tmux`, over `ssh`, and in CI. Specified in [`docs/terminal-block-protocol-spec.md`](docs/terminal-block-protocol-spec.md) and implemented by the `winter-proto` reference codec.
- **Live blocks**: `open`, `patch`, and `close` fold RFC 6902 patches into a block that re-renders in place and re-reserves grid rows as its content grows. Patch folding is best-effort, so a malformed operation is skipped rather than freezing the display.
- **Vim-style modal navigation over the scrollback.** Four per-pane modes (Insert, Normal, Visual, Block-Focus) with operators, text objects, registers, marks, the jumplist and changelist, dot-repeat, regex search, blockwise Visual, surround, and which-key hints. The modal layer lives in the terminal rather than the shell's line editor, so it works identically in bash, zsh, fish, a Python REPL, or over `ssh` to a machine you cannot configure.
- **Prompt-line editing**: Vim operators aimed at the line the shell is currently editing are translated into the equivalent readline keystrokes, with `prompt-edit-bindings` (`"emacs"` default, `"none"`) to decline them when the shell is in vi mode and has those chords bound elsewhere.
- **Block-aware scrollback** driven by OSC 133 marks: per-command boundaries, exit-code tags, folding, working directories from OSC 7, and block navigation. Shell integration scripts for bash, zsh, and fish ship in `clients/shell-integration/` and are installed by the `.deb` and the Windows installer.
- **Session multiplexer**: `winter mux serve/new/attach/list/kill/proxy`, session persistence across server restarts, PTY size arbitration across attached clients, and remote attach over an SSH tunnel.
- **GPU text rendering** on wgpu and glyphon, with sixel and raster image blocks, SVG, markdown and CSV blocks, ligatures, rainbow parens, and a WebView pass for rich content.
- **Client SDKs** for Rust ([`clients/client-rs`](clients/client-rs)), Python ([`clients/client-py`](clients/client-py)), and shell ([`clients/client.sh`](clients/client.sh)).
- **Configuration in KDL**: `settings.kdl`, `keybindings.kdl`, and user themes under `themes/<name>.kdl`, all hot-reloaded on save. `winter --reload` reloads a running instance.
- **Documentation**: [`docs/usage-guide.md`](docs/usage-guide.md) for the modes, the full keymap, and every settings key; [`docs/terminal-block-protocol-spec.md`](docs/terminal-block-protocol-spec.md) for the protocol; [`docs/architecture.md`](docs/architecture.md) for how the workspace fits together; and [`SECURITY.md`](SECURITY.md) for the threat model and disclosure channel.
- **Packaging** for all three platforms: a `.deb`, an arm64 `.dmg`, and a Windows installer, plus downstream manifests for the AUR, Scoop, and winget under `packaging/`. Pushing a `vx.y.z` tag builds all three on their native runners and opens a draft GitHub Release.
- **Continuous integration** covering rustfmt, clippy, tests on Linux, Windows, and macOS, the advertised MSRV, the Python client, `cargo audit`, and a crates.io publish dry run.

### Security

Winter's threat model starts from the assumption that every byte arriving from a PTY is attacker-controlled: a `cat` of a downloaded file, output piped from `curl`, or a program on the far side of an `ssh` can all write arbitrary escape sequences.

- **A trust tier on the wire is a request, never a grant.** The tier a block asks for is clamped against `security.block-max-trust`, which defaults to `restricted`, so nothing arriving from a PTY reaches scripting without the user configuring it.
- **Rendering a block makes no network request the user did not ask for.** Remote subresources for Vega and Vega-Lite blocks require opting in through `security.block-remote-assets`; the default renders the spec inline.
- **OSC 52 clipboard reads are opt-in** (the top-level `clipboard-read` setting), because the query is silent on the querying side.
- **Every crate sets `#![forbid(unsafe_code)]`.**
- **Every unbounded accumulator has an explicit cap** with a documented rationale: retained block output, live-block patch count, scrollback rows, the mux client outbox, and the APC payload buffer.
- **Both parsers driven entirely by untrusted input are fuzzed.** `crates/winter-render/tests/vt_fuzz.rs` covers the VT escape parser and cell grid; `crates/winter-core/tests/tbp_fuzz.rs` covers the OSC 133 block state machine and the TBP codec. Both generate streams from a seeded PRNG biased toward the shapes that break terminals (extreme CSI parameters, inverted scroll regions, truncated OSC, wide characters at the right margin, resizes interleaved with output, invalid UTF-8) and assert structural invariants after every chunk. A failure prints a seed that reproduces it.

[0.1.0]: https://github.com/taquangtrung/winter-term/releases/tag/v0.1.0
