# Changelog

All notable changes to Winter are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

While the major version is `0`, the TBP wire format, the KDL config schema, and every crate's public API may change in a minor release.

## [Unreleased]

### Added

- **macOS packaging.** `make package` now builds `Winter.app` and a `.dmg` (`packaging/macos/Info.plist`, plus an `.icns` generated from the 512px source with the `sips` and `iconutil` that ship with macOS). `make install` copies the bundle to `/Applications`. The `.dmg` is arm64-only and unsigned; see the README for the Gatekeeper note.
- **Tag-driven release automation** (`.github/workflows/release.yml`): pushing `vx.y.z` builds the `.deb`, `.dmg`, and `.exe` on their native runners, extracts the matching `CHANGELOG` section as the release body, and opens a draft GitHub Release with all three attached.
- **Adversarial stream fuzzing** for the two parsers driven entirely by untrusted input: `crates/winter-render/tests/vt_fuzz.rs` for the VT escape parser and cell grid, and `crates/winter-core/tests/tbp_fuzz.rs` for the OSC 133 block state machine and the TBP codec. Both generate streams from a seeded PRNG biased toward the shapes that break terminals (extreme CSI parameters, inverted scroll regions, truncated OSC, wide characters at the right margin, resizes interleaved with output, invalid UTF-8) and assert structural invariants after every chunk; a failure prints a seed that reproduces it. These found both crashes above.
- **Downstream packaging manifests**: an AUR `PKGBUILD`, a Scoop manifest, and the winget submission metadata, all under `packaging/`.

- `prompt-edit-bindings` setting (`"emacs"` default, `"none"`). Vim operators on the prompt line are realized as readline chords, which a shell in vi mode has bound to something else; `"none"` makes Winter decline them rather than fire the wrong command, leaving the line to the shell's own Vim editing. An unrecognized value keeps the default rather than disabling a working feature.

### Changed

- `App::window_event` is now a dispatcher rather than a dispatcher with four handlers inlined. Its `KeyboardInput`, `MouseInput`, `CursorMoved`, and `MouseWheel` arms moved verbatim into `on_keyboard_input`, `on_mouse_input`, `on_cursor_moved`, and `on_mouse_wheel`, taking the function from 837 lines and 11 levels of nesting to 151 lines and 7. Behavior is unchanged.
- CI now runs the Linux jobs on every push and pull request, and the Windows/macOS test jobs weekly or on demand. Doc-only changes skip CI entirely. This cuts a routine run from roughly 145 billed Actions minutes to 20, which matters on a private repository where macOS runners bill at 10x.

- Shell integration scripts for bash, zsh, and fish (`clients/shell-integration/`), emitting the OSC 133 command marks and OSC 7 working-directory reports Winter needs for command blocks, folding, and exit codes. Installed by the `.deb` under `/usr/share/winter-term/shell-integration/` and by the Windows installer under `<install dir>\shell-integration`. Previously nothing emitted these marks, so a fresh install saw the whole session as one rolling block.

### Fixed

- **Two crashes reachable from any PTY write.** `CSI H` followed by `CSI 999 M` (delete more lines than the scroll region holds, from row 0) underflowed a `usize` in `Grid::delete_lines` and killed the terminal. Separately, a cursor saved with DECSC before a window resize that shrank the grid was restored unclamped, so the next `CSI P`/`CSI @`/`CSI X` indexed past the cell buffer. Both are triggerable by `cat` of a crafted file, output piped from `curl`, or a program on the far side of `ssh`.
- OSC 7 working directories are now percent-decoded. A spec-conforming shell integration encodes the URI, so a directory containing a space or a non-ASCII character arrived as `%20`/`%C3%A9` and was used verbatim in the title bar and by the recent-directories palette.

## [0.1.0]

First tagged release. Everything below is the state of the project at the point release engineering was put in place; earlier work is in the commit history rather than itemized here.

### Added

- Terminal Block Protocol (TBP) v1: an OSC 9001 escape carrying a MIME bundle, with a `text/plain` fallback for every block and graceful degradation under tmux, ssh, and CI. Specified in `docs/terminal-block-protocol-spec.md` and implemented by the `winter-proto` reference codec.
- Live blocks: `open`/`patch`/`close` fold RFC 6902 patches into a block that re-renders in place, re-reserving grid rows as its content grows.
- Vim-style modal navigation over the scrollback: Normal and Insert modes, operators, text objects, registers, marks, the jumplist and changelist, dot-repeat, regex search, and prompt-line editing.
- Session multiplexer: `winter mux serve/new/attach/list/kill/proxy`, session persistence across server restarts, size arbitration across attached clients, and remote attach over ssh.
- GPU text rendering (wgpu plus glyphon), sixel and raster image blocks, ligatures, and rainbow parens.
- Client SDKs for Rust (`clients/client-rs`), Python (`clients/client-py`), and shell (`clients/client.sh`).
- Configuration in KDL: `settings.kdl`, `keybindings.kdl`, and user themes under `themes/<name>.kdl`, hot-reloaded on change.
- `winter --help` and `winter --version`.
- `security` config block: `block-max-trust` and `block-remote-assets`.
- Continuous integration covering rustfmt, clippy, tests on Linux/Windows/macOS, the advertised MSRV, the Python client, and a crates.io publish dry run.
- A `LICENSE` file, matching the `MIT` declared in every manifest.

### Security

- **Trust tiers are now clamped by terminal policy.** The tier a block requests on the wire was previously granted verbatim, so any byte stream reaching a PTY (a `cat` of a downloaded file, output piped from `curl`, a program on the far side of `ssh`) could ask for `trust=trusted` and be rendered in a WebView with scripting enabled and no CSP. Requested tiers are now clamped against `security.block-max-trust`, which defaults to `restricted`.
- **Block rendering no longer makes unsolicited network requests.** Vega and Vega-Lite blocks injected three CDN `<script>` tags at render time. Remote subresources now require opting in via `security.block-remote-assets`; the default renders the spec inline instead.

### Fixed

- Bounded the per-session retained output. The GPU grid capped its own scrollback rows, but the parsed block list did not, so a long-lived pane grew without bound. Output past the budget is now elided from the oldest blocks, leaving block indices stable.
- `winter mux serve` no longer displaces a server that is already listening. The socket path was unlinked unconditionally, so a second server silently took it over and stranded the first server's sessions.
- Config parse errors are surfaced in the status bar. A malformed `settings.kdl` reported only to stderr was invisible to a GUI launched from a desktop menu, which silently fell back to defaults.
- `rust-version` now states the floor that is actually built and tested (1.96). The previous claim of 1.80 could not build: the dependency graph alone requires 1.95.

[Unreleased]: https://github.com/taquangtrung/winter-term/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/taquangtrung/winter-term/releases/tag/v0.1.0
