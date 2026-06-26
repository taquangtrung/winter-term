# Winter architecture

Winter is a terminal emulator that models a session as a list of typed, MIME-tagged blocks rather than a flat scrollback buffer. This document summarizes how the workspace is put together: what each crate owns, how bytes from a PTY become pixels, and which invariants the design leans on. It describes the code as it stands, not a plan.

## Workspace layout

Five crates, in dependency order. Nothing below depends on anything above it, so the protocol and block model stay usable without dragging in a GPU stack.

| Crate           | Owns                                      |
|-----------------|-------------------------------------------|
| `winter-proto`  | TBP v1: wire types, codec, trust tiers.   |
| `winter-core`   | PTY driving and block-list scrollback.    |
| `winter-render` | Cell grid, VT screen, wgpu text renderer. |
| `winter-term`   | App: window, panes, modes, layout, mux.   |
| `winter-client` | Emitting TBP blocks from Rust programs.   |

Each crate directory is named for its package, so `winter-core` lives in `crates/winter-core`. The exception is `winter-client`, which sits in `clients/client-rs` beside the Python and shell clients rather than under `crates/`.

```text
   winter (app binary)
       │
       ├──> winter-render ──> wgpu, glyphon, resvg, pulldown-cmark
       │
       └──> winter-core ──> portable-pty, vte
                │
                └──> winter-proto ──> serde, base64
```

Every crate sets `#![forbid(unsafe_code)]`. The three library crates also set `#![deny(missing_docs)]`.

## The byte pipeline

A pane spawns a PTY child and reads its output on a background thread. Those bytes go through exactly one `vte` parse pass, in `CombinedPerformer` (`crates/winter-term/src/terminal/pane.rs`), which fans each parsed event out to two consumers at once.

```text
        PTY child (shell)
                │
                │ bytes
                ▼
        CombinedPerformer          one vte parse pass
                │
        ┌───────┴────────┐
        ▼                ▼
   winter_render     winter_core
      ::Grid          ::Scrollback
   (cell grid)        (block list)
        │                │
        ▼                ▼
   GpuRenderer       BlockQueue
   (wgpu text)      (rich content)
                         │
                         ▼
                  WebView tiles (wry)
```

The single-pass design is the reason the visual grid and the block list can never disagree about ordering: a block is anchored to the grid row the cursor was on when its escape arrived, and those anchors are drained together with the block itself.

`CombinedPerformer` also handles what `vte` 0.13 will not. It tracks `ESC _` APC strings by hand so the Kitty graphics protocol can be parsed, and it keeps separate Kitty keyboard flag stacks for the main and alternate screens so a full-screen program cannot leak pushed flags back into the shell prompt.

`winter_render::Screen` is a second, standalone VT driver over the same `Grid`. It is not used by the app; it backs the headless demo path in `crates/winter-term/src/main.rs` and the tests.

## Two renderers, one screen

Plain text and rich content take different paths and are composited into the same window.

- **Text** goes to the GPU. `renderer::GpuRenderer` draws the `Grid` with `cosmic-text` and `glyphon` over a wgpu surface, alongside WGSL shaders for backgrounds, cursors, and images.
- **Rich content** goes to a WebView. `crates/winter-term/src/terminal/webview.rs` creates a `wry` child WebView per rich block, positioned as a tile over the grid, wrapped in an HTML shell that applies a Content-Security-Policy chosen by the block's trust tier.
- **Raster images and SVG** bypass the WebView. They are decoded and drawn natively as GPU image placements.

Because a rich block's rendered height is not known when its escape arrives, a block reserves `BLOCK_RESERVE_ROWS` (12) rows in the grid at emit time so following shell output flows below it rather than under it. Raster images, whose pixel dimensions are known up front, reserve their exact height instead, capped at `MAX_IMAGE_ROWS` (24).

## The block model

`winter_core::Scrollback` is a state machine over semantic terminal events, deliberately independent of `vte` and the PTY so it can be driven directly from tests. `crates/winter-core/src/parser.rs` is the adapter that turns escape sequences into its method calls.

A session is a `Vec<CommandBlock>`. Each block carries the command line, its working directory, its exit code, and an ordered `Vec<Segment>`, where a segment is plain text, an OSC 8 hyperlink span, a one-shot TBP content block, or a live block. Block boundaries, commands, and exit codes come from OSC 133 shell integration marks; working directories come from OSC 7. Without shell integration the whole session is one rolling block, which is a degradation rather than a failure.

Live blocks are the interesting case. A tool opens a block, streams RFC 6902 JSON patches at it, and closes it. `LiveBlock::current_spec` folds the patches over the initial spec on demand. Folding is deliberately best-effort: a malformed patch operation is skipped rather than aborting the whole patch, because freezing a progress display at its initial state is a worse outcome for a terminal than showing a slightly stale one.

## Resource budgets

A terminal is expected to stay open for days, so nearly every unbounded accumulator in the design has an explicit cap with a documented rationale. These are the load-bearing ones.

- `MAX_RETAINED_OUTPUT_BYTES`, 8 MiB: without it a pane that streams output grows the block list until the OOM killer intervenes. The grid caps its own scrollback rows, but that cap never reached the block list.
- `BUDGET_CHECK_INTERVAL_BYTES`, 1 MiB: measuring the budget is a walk of every retained block, so it is amortized over this much growth rather than run on every write.
- `MAX_LIVE_BLOCK_PATCHES`, 10,000: a tool streaming frequent updates to one long-lived block (a progress bar, a log tail) would otherwise accumulate an entry per update for the block's whole lifetime.
- `MAX_SCROLLBACK`, 10,000 rows: the grid's own history ceiling.
- `CLIENT_OUTBOX_LIMIT`, 4 MiB: a mux client this far behind is dropped rather than buffered without bound. The server's memory is not traded for the illusion of a live connection.
- `APC_MAX_PAYLOAD`, 4 MiB: guards against a malformed or unterminated APC sequence bloating memory.
- `PATCH_MIN_INTERVAL`, 100 ms: a patch arriving sooner is held and applied once this elapses, capping the update rate a fast-streaming tool can force.

Retention uses elision rather than removal: when a block exceeds the byte budget its output is dropped but the block itself stays in the list, because a block's index is a stable identifier held by the app layer for folds, WebView tiles, and image placements. Removing entries would silently renumber every one of them.

## The protocol

TBP is one OSC escape per message: `OSC 9001 ; <verb> ; <params> ; <base64 payload> ST`. Verbs are `emit`, `open`, `patch`, `close`, and `caps`. The payload is base64-encoded JSON, which keeps the whole escape opaque to terminals that do not implement TBP; they ignore it wholesale.

The payload is a MIME bundle, modeled on Jupyter's `display_data`: the emitter supplies several representations of the same content, and the terminal picks the richest one it can render, falling back toward `text/plain`. Every bundle carries a `text/plain` fallback, so a program using the client libraries stays safe under `tmux`, `ssh`, or CI. `clients/client-rs` prints the fallback and turns later `update`/`close` calls into no-ops when Winter is not the active terminal.

## Trust and clamping

`TrustTier` is the security boundary, and its central rule is stated in the type itself: a tier on the wire is a request, never a grant.

Every byte reaching the terminal is attacker-controlled. A `cat` of a downloaded file, output piped from `curl`, or a program on the far side of `ssh` can all spell `trust=trusted`, and nothing on the wire authenticates the emitter. So `TrustTier::clamp_to` lowers a requested tier to the policy ceiling and never raises it. The tiers are declared in ascending capability order (`Isolated < Restricted < Trusted`) and the clamp is `min`, which makes the ordering load-bearing: the doc comment warns against comparing tiers any other way.

The default ceiling is `Restricted`, so nothing arriving from a PTY can reach `Trusted` scripting without the user configuring it. A test asserts exactly that. Network fetches follow the same posture: the Vega renderer's CDN scripts are only ever injected when the user has opted in, because rendering a block must not make a request the user did not ask for.

## Interaction model

`crates/winter-term/src/model/` is a pure layer: modes, split-tree geometry, key resolution, the command palette, and the settings page, with std-only dependencies and no side effects. This is what makes the vim layer testable without a window, and it is where most of the 967 test functions live.

There are three per-pane modes. Insert encodes keys to bytes for the PTY. Normal intercepts keys as motions, operators, and layout actions, which is what lets the vim vocabulary work against any shell without shell-side configuration. Block-Focus forwards keys to a rich block until `Esc`.

Layout is a binary split tree per tab (`crates/winter-term/src/model/layout.rs`): leaves are panes, internal nodes are splits with a ratio clamped to `[0.1, 0.9]` so a pane can never be squeezed to zero. `PaneId`s are allocated by the owner so they stay unique across tabs.

`crates/winter-term/src/app/mod.rs` wires this to `winit`, with submodules splitting the `App` by concern: `init` for GPU bootstrap, `actions` for dispatch, `render` for frame composition, `navigation` for motions and search, `pointer` for mouse and clipboard, `blocks` for fold and yank.

## Multiplexer

`crates/winter-term/src/mux/` is a headless PTY session manager. The server owns PTY processes and outlives client disconnects; clients attach to named sessions over a Unix domain socket, or over an SSH tunnel to a remote server.

```text
   Winter (client) ──> Unix socket ──> mux server ──> PTY
   Winter (client) ──> SSH tunnel  ──> remote mux  ──> PTY
```

Frames are 4-byte big-endian length prefixes followed by UTF-8 JSON, chosen to stay debuggable and language-agnostic. Two details carry design weight. On attach the server replays buffered output as a `Scrollback` message before live output resumes, so a fresh client rebuilds its screen rather than starting blank. And PTY geometry is server-arbitrated at the smallest geometry among attached clients, with a `Resized` message to every client, so a pane larger than the session letterboxes its grid instead of rendering a stream wrapped for a width it does not have.

`crates/winter-term/src/control.rs` reuses the same framing for a separate one-shot control socket, used by `winter --reload` to ask a running instance to save its session and relaunch. The module doc is explicit that this is framing reuse only, not a second multiplexer.

## Configuration and persistence

Configuration is KDL, split across `settings.kdl` for appearance and behavior and `keybindings.kdl` for bindings, under `~/.config/winter-term/` (`%APPDATA%` on Windows), with a legacy single-file `winter.kdl` still read when neither is present. User themes are separate KDL files: an optional `base "dark"|"light"` plus a `colors` block layered over it.

On clean exit Winter writes a session snapshot to `$XDG_STATE_HOME/winter-term/session.json` and restores the split layout and per-pane working directories on the next launch. PTY children are spawned fresh, not reattached; panes labeled with the `mux:` or `mux-remote:` prefix are re-attached to their session instead of respawning a local shell.

## Entry points

The `winter` binary dispatches on its first argument: no argument opens the window, `mux` routes to the multiplexer subcommands (`serve`, `list`, `new`, `kill`, `attach`, `proxy`), and `--reload` talks to the control socket of a running instance.

## Testing

967 `#[test]` functions live beside the code, concentrated in the pure model layer. Integration tests cover the PTY (`crates/winter-core/tests/pty.rs`), shell integration, and mux end-to-end behavior. Two fuzz suites feed adversarial streams at the parsers: `crates/winter-render/tests/vt_fuzz.rs` for the VT parser and cell grid, and `crates/winter-core/tests/tbp_fuzz.rs` for the TBP block parser.
