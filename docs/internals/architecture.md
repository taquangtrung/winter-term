# Winter architecture

Winter is a terminal-first IDE. Its terminal models a session as a list of typed, MIME-tagged blocks rather than flat scrollback. This document describes the code as it stands: what each crate owns, how PTY bytes become pixels, and the invariants the design leans on.

## Workspace layout

Crates are grouped by layer under `crates/`, and each depends only on the layers below its own. No tool depends on the app or on another tool; what two share moves down into `libs` or `base`. An extension depends on the `sdk` crates alone, as one written outside Winter would. So an edit to one tool does not rebuild the window, and the protocol, the block model, and the emulator stay usable without it. A tool asks the window for anything through the `Page` and `Editor` traits in `winter-model`. `crates/app/winter-term/tests/layers.rs` checks the layers against `cargo metadata`.

```text
crates/app       the window, which ties the rest together
     │
     │ depends on
     ▼
crates/tools     a tool each: the editors, and the terminal's runtime
     │
     │ depends on
     ▼
crates/libs      libraries on the core: languages, git, sessions, fields, extensions
     │
     │ depends on
     ▼
crates/base      the core: winter-model and what it stands on

crates/sdk       what an extension author builds against
     ▲
     │ depends on
     │
crates/ext       extensions shipped with Winter, on the sdk alone
```

| Folder | Crate | Owns |
|---|---|---|
| `app` | `winter-term` | The window: panes, tabs, the terminal tab, the bars, popups, and renderer, the pickers' sources, config, and the editors not yet in crates of their own. |
| `tools` | `winter-dir` | The directory listing, and the file moves, copies, and deletes it runs. |
| `tools` | `winter-git` | The Git view. |
| `tools` | `winter-monitor` | The system pages: process, resource, and port monitors, and startup programs. |
| `tools` | `winter-pdf` | The PDF viewer, drawn with hayro, with selection and SyncTeX. |
| `tools` | `winter-terminal` | The terminal runtime: PTY sessions, the parser, blocks, charts, image protocols. |
| `tools` | `winter-text` | The text editor, diff and merge, and syntax colors from the theme's scope table. |
| `libs` | `winter-field` | Text fields (prompts, search boxes, renames in place) as Cherry documents. |
| `libs` | `winter-lsp` | Language servers, and the problems they report. |
| `libs` | `winter-spell` | Hunspell dictionaries and spelling suggestions. |
| `libs` | `winter-synctex` | SyncTeX both ways, between a PDF page and its LaTeX. |
| `libs` | `winter-mux` | Sessions that outlive the window, over a local socket or SSH. |
| `libs` | `winter-vcs` | Git as the tools share it: commands, their output read, a tree watched. |
| `libs` | `winter-wasm` | Runs installed extensions as sandboxed components. |
| `base` | `winter-base` | OS plumbing: per-user paths, atomic writes, helper processes, remote places. |
| `base` | `winter-emulator` | Parser, scrollback, cell grid, VT screen. |
| `base` | `winter-keys` | Toolkit-free keys: chords in Emacs notation, sequences, key tables. |
| `base` | `winter-model` | Modes, layout, key resolution, the `Page` and `Editor` traits, the Vim vocabulary. |
| `base` | `winter-proto` | TBP v1: wire types, codec, trust tiers. |
| `base` | `winter-theme` | The theme, theme files, and the syntax scopes they color. |
| `sdk` | `winter-ext-api` | The API extensions are written against. |
| `sdk` | `winter-ext-guest` | Builds an extension as a WebAssembly component. |
| `ext` | `winter-copilot` | GitHub Copilot. |
| `ext` | `winter-docker` | The Docker view. |
| `ext` | `winter-claude`, `winter-antigravity`, `winter-zai` | The AI quota segments. |
| `ext` | `winter-simple-ext` | An example extension. |

A crate's directory is named for its package, such as `crates/base/winter-emulator`. The exception is `winter-client`, in `clients/client-rs` beside the Python and shell clients. `vendor/portable-pty` carries one Windows fix.

The protocol, emulator, keys, theme, spell, lsp, and client crates forbid `unsafe`. The app denies it, allowing it only around a few system calls and the environment `main` sets up before any other thread starts, each with a `SAFETY` comment. Every crate outside `sdk` and `ext` denies missing docs, except `winter-dir`, `winter-monitor`, `winter-synctex`, `winter-vcs`, and `winter-wasm`.

## The byte pipeline

The terminal lives in two places. `winter-terminal` is the runtime (PTY session, parser, block queue, charts, image protocols), with no window in it. `crates/app/winter-term/src/window/terminal/` is what a terminal tab adds in the window: blocks, modes, selection, search, the pointer. A change to what output means goes in the first; a change to what a key or a click does goes in the second.

A PTY's output is read on a background thread and parsed once, by `vte` in `CombinedPerformer` (`winter-terminal/src/session/performer.rs`), which hands every event to both the grid and the block list.

```text
        PTY child (shell)
                │
                │ bytes
                ▼
        CombinedPerformer          one vte parse pass
                │
        ┌───────┴────────┐
        ▼                ▼
    winter_emulator      winter_emulator
      ::Grid          ::Scrollback
   (cell grid)        (block list)
        │                │
        ▼                ▼
   GpuRenderer       BlockQueue
   (wgpu text)      (rich content)
                         │
                         ▼
                  canvas (wgpu images)
```

One pass is why the grid and the block list never disagree on order: a block is anchored to the row the cursor was on when its escape arrived.

Before `vte`, a `ByteFilter` lifts out APC strings, which `vte` drops, for the Kitty graphics protocol, and caps OSC strings. The main and alternate screens keep separate Kitty keyboard flag stacks, so a full-screen program cannot leak its flags into the shell.

`winter_emulator::Screen` pairs a parser with a `Grid` for the headless path and the fuzz suite. It runs the same `impl Perform for Grid` the app does, so tests exercise the real parsing.

## Drawing

Winter draws everything itself, on the GPU, in one window.

- **Text:** `GpuRenderer` resolves each pane's grid into misty's cells and solid fills, and misty draws them in the grid's font.
- **Everything else** goes on the canvas (`window/frame/canvas.rs`): pixels, SVG, Markdown, text, or a Vello scene, drawn into a texture at the size shown and placed as a cropped GPU image. Vega-Lite charts become SVG; HTML in a block becomes Markdown, with nothing in it run. HTML pages (the Markdown preview, extensions' views) are laid out by Blitz and drawn with Vello on the window's wgpu device. Winter's own pages are misty views (`window/frame/page_views.rs`).
- **Popups, menus, and notifications** are drawn last, on top.

A rich block's height is unknown when it arrives, so it reserves `BLOCK_RESERVE_ROWS` (12) rows and the shell's output flows below it. Images, SVG, and charts reserve their exact height, up to `MAX_IMAGE_ROWS` (24).

## The block model

`winter_emulator::Scrollback` is a state machine over semantic terminal events, independent of `vte` and the PTY so tests drive it directly; `parser.rs` turns escapes into its calls. A session is a list of `CommandBlock`s, each with its command line, working directory, exit code, and segments: text, OSC 8 links, TBP blocks, and live blocks. OSC 133 marks the blocks and OSC 7 their directories. Without shell integration the session is one rolling block.

A live block takes a stream of RFC 6902 JSON patches, each folded into its state as it arrives, so an update costs only its own size. A malformed operation is skipped rather than failing the patch: a slightly stale progress display beats a frozen one.

## Resource budgets

A terminal stays open for days, so every unbounded accumulator has a cap.

| Cap | Value | Bounds |
|---|---|---|
| `MAX_RETAINED_OUTPUT_BYTES` | 8 MiB | The block list's output. |
| `BUDGET_CHECK_INTERVAL_BYTES` | 1 MiB | How often that budget is measured, as measuring walks every block. |
| `MAX_COMMAND_BYTES`, `MAX_CWD_BYTES` | 4 KiB each | A command line or directory a program never ends. |
| `MAX_BLOCKS` | 50,000 | Blocks; past it, output carries on in the last one. |
| `MAX_PATCH_OPS` | 256 | Operations in one patch; more needs a whole `replace`. |
| `MAX_SCROLLBACK` | 10,000 rows | The grid's history. |
| `CLIENT_OUTBOX_LIMIT` | 4 MiB | A mux client this far behind is dropped. |
| `APC_MAX_PAYLOAD` | 4 MiB | An APC string; a longer one is dropped, never printed. |
| `MAX_OSC_BYTES` | 16 MiB | An OSC string, in every parser, as `vte` buffers without limit. |

A block over the byte budget loses its output but stays in the list, since its index is a stable id the app holds for folds and canvas blocks.

## The protocol and trust

TBP is one OSC escape per message: `OSC 9001 ; <verb> ; <params> ; <base64 payload> ST`, with the verbs `emit`, `open`, `patch`, `close`, and `caps`. A terminal without TBP ignores the escape whole. The payload is a MIME bundle with several forms of the same content, and the terminal draws the richest it can. Every bundle carries `text/plain`, so a program using the client libraries stays readable under `tmux`, `ssh`, or CI. [terminal-block-protocol-spec.md](../reference/terminal-block-protocol.md) is the specification.

Every byte reaching the terminal is untrusted, and any program can spell `trust=trusted`. So Winter has nothing a tier could grant: every block is drawn natively, and nothing in one is run or fetched. `TrustTier` stays in the protocol for clients that draw blocks in a web engine; `TrustTier::clamp_to` only ever lowers it.

## The window

`winter-model` is the window-free layer: modes, layout, key resolution, the page and editor contract, painted rows, the Vim vocabulary, and the Settings page's rows. It has no side effects, so most of the tests live there. The picker framework is misty's `misty::picker`; Winter's sources are in `window/picker/sources/`.

**Modes.** Each terminal tab has three. Insert sends keys to the PTY; Normal takes them as motions, operators, and layout actions; Visual is Normal with motions extending a selection. `Esc` in Normal stays in Normal, and only `i`, `a`, or `o` return to Insert, so `Esc` never drops the next key into the shell.

**Layout** (`winter-model/src/layout/`). A window is a main area and three panels. The main area is a binary split tree: leaves are panes, each a strip of tabs, and every split keeps its ratio within `[0.1, 0.9]`. Each panel is a `Dock<K>`: its views by kind, the one shown, whether it is shown, and its size in pixels. `Panels::fit` gives up the right sidebar first in a small window. One focus covers the window, a tab or a view. Panels belong to the window, so a workspace switch keeps them. In the app (`window/panels/`), a view is a misty page in no pane, fed each frame by `sync_panel_views`. A text editor's outline comes from the best source it has: the format's own outline (Markdown, LaTeX), the language server's symbols, Cherry's outline, or the grammar's definitions.

**Shown.** Each frame `sync_shown` finds what is shown and tells each page through `Page::set_shown`. Nothing not shown does work, and nothing polls: views follow files through `DirWatch` and `TreeWatch`.

**The `App`.** `window/mod.rs` runs on misty's event loop, with submodules splitting the `App` by concern: `runtime`, `input`, `frame`, `layout`, `editor`, `picker`, `settings`, `workspace`, `appearance`, `terminal`, and the bars and popups around the panes. Shared state is grouped into types (`EditorHost`, `WindowParts`, `RuntimeState`, `InputState`, `TerminalState`, and smaller ones for search, menus, the pointer, the selection, and Vim), and a method touching one group alone belongs to that group.

**Crashes and freezes.** Every handler runs inside `survive` (`window/runtime/event_loop.rs`): a panic is written with its backtrace to `crash.log`, a notice says so, and the window carries on. Profiling, off by default (`winter --profile` or `Profiling: Profile the Window (Toggle)`), adds two things. Warnings and errors go to `winter.log` as well as standard error. And a watchdog (`watchdog.rs`) keeps the event loop's turns past 100 ms; one past five seconds writes the slow turns and every thread's stack (from `/proc`, and `gdb` where installed) to `crash.log`. The watchdog sleeps while the window waits. `Profiling: Write a Report` and `winter profile` gather the same account; the control socket answers on a thread of its own, so a frozen window still answers.

## Normal mode

Winter owns the keyboard in Normal mode, so the Vim keys work on any shell, over `ssh` too, with nothing installed on the far end.

- **Keys.** `resolve_with` turns the mode, a key, and a `PendingPrefix` into an `Action`. The prefix is explicit state (a count, a pending operator, `[` or `]`, `C-w`), so a half-typed sequence can be drawn: the which-key popup is a view of it.
- **Motions** are pure functions over a `&Grid`. Small-word motions cross punctuation as they cross a blank, and big-word motions take it into the run beside it; text objects keep Vim's classes. The spans are written once over a `TextRows` trait that the grid and the text editor's buffer both answer, so `iw` means the same in both.
- **State** is per tab (the jumplist, the changelist, `.`, marks), except registers, which are shared so a yank in one tab pastes in another. The jumplist keeps 100 entries and forks like undo; the changelist only appends.
- **The prompt line** belongs to the shell, so an operator on it is sent as readline keys: arrows to move, then a kill. That assumes Emacs-mode bindings; `prompt-edit-bindings "none"` leaves the line to a shell in vi mode. Undo uses readline's own.

## Pages and editors

`window/layout/open_tabs.rs` holds every tab by `TabId` as one enum: a terminal, a page, or an editor. What only a terminal has lives in its entry and goes with it. The split tree, focus, and zoom deal only in ids.

A `Page` (`winter-model/src/editor/`) paints itself as styled rows (`paint.rs`: spans in semantic styles, never colors, so the theme resolves them), takes keys, and reports its mode and notices. An `Editor` is a page whose contents are a buffer, and adds what a buffer needs: saved views, burying, its file, and unsaved changes. The window reaches both through `PageRef` and `PageMut`. A page tab is one per window, never buried, and rebuilt from Winter's state each time it opens. Editors live in `crates/app/winter-term/src/editors/` and the tool crates. The traits hold only what any page or editor may do; one kind's own hooks are reached by type through `with_kind`.

**Keys.** An editor gets a key after the global chords. A key it declines goes to the layout chords alone, so splitting and focus still work while a terminal's Vim motion never acts on an editor.

**Gutter and breadcrumbs.** The window splits an editor's pane into a gutter strip and the content's grid (`layout::bands`), with an optional breadcrumb row above. The editor counts columns in the content's grid alone.

**Talking to the window.** An editor never reads the keyboard for a question or draws its own messages. It returns requests in its outcomes: a job, a prompt answered back by tag (a single key when the answer cannot be undone), a setting. Its notices are taken after every call (`Editor::take_notice`) and go through `App::notify` into one `Notifier`, which keeps the toasts on screen and the history. `Editor::status` gives its mode and the segments at the status bar's right. `/` is one literal matcher over each editor's rows, so `n` and `N` mean the same everywhere.

**Slow work** leaves the window's thread one of three ways:

1. **A job** (`window/runtime/jobs`): a git command, a shell command, a file read. Its answer comes back to the editor that asked.
2. **A `Background`** (`editors/background.rs`): one piece of work no job type fits, answering once. Replacing or dropping it cancels it, so a stale answer never lands.
3. **A thread of its own**, for work that lives as long as its editor and answers often: a build, the PDF worker, a file watcher.

Jobs past a fixed number in flight are dropped rather than queued, since each asks about what is on screen now. An editor that must stay current names its next deadline, and the event loop sleeps until the earliest one.

## Commands

Every command run by name is one entry in `window/commands/mod.rs`: its id, its palette title, and what it does. The palette, the key bindings, and `App::run_command` all read that one table, so no command is listed and dead or live and unlisted. Extensions' commands follow, by their own ids.

## Extensions

An extension is a Rust crate on `winter-ext-api`, compiled in behind a Cargo feature or installed as a WebAssembly component. `winter-wasm` makes a component look like a compiled-in extension, so nothing else tells them apart. An extension adds commands, status bar segments, completion back-ends, views, inline suggestions, servers, and settings, and hears events: files opened, saved, or edited, its servers' messages, its timers, its segments clicked. Everything it does goes through a narrow `Host` (`window/extension_host.rs`) on the window's thread, with slow work handed to `spawn`. A panic stops only that extension.

**What it can reach.** It runs only programs its `run` grant names, fetches only `https` from hosts it is granted, sets one-shot timers, and watches files it may read. A secret it asks for is kept in the system's secret store and added to requests by Winter, never handed to the extension. Its timers and watches run only while one of its segments or views is shown.

**Installing.** `winter extension install` reads the package embedded in the component without running it, shows what it asks for, and once approved copies the component to `extensions/<id>/<id>.wasm` and records its SHA-256 and grants in `extension-approvals.kdl`. Winter loads a component only when its hash is the approved one, from the bytes it hashed. Each runs in its own sandbox: its data directory, the directories and hosts granted, 256 MiB, a second per call on the window's thread, and a minute for slow work. Compiled code is cached in `wasm-cache`. Bundled extensions load the same way from beside the program, and a user's own replaces one by id. The WIT contract, `winter:extension@0.1.0`, only grows within a release line ([extension-api.md](../reference/extension-api.md)).

**Servers.** An extension never starts a process. It declares a server (`ServerSpec`), and the window runs it through the language-server hub, if granted, from `extension-servers/<id>`, a directory the extension cannot write to. Inline suggestions cross the sandbox as a request and an answer, never the files. An extension's settings are a block of `settings.kdl` named for its id.

Copilot, Docker, and the AI quota segments are built on this alone. `winter-simple-ext` is the template; `winter-wasm/tests` runs it, and every past API release, through the runtime.

## Multiplexer

`winter-mux` is a headless PTY session manager. Its server owns the PTYs and outlives its clients, which attach to named sessions over a Unix socket or an SSH tunnel.

```text
Winter (client)
     │
     │ Unix socket, or SSH tunnel to a remote server
     ▼
mux server
     │
     ▼
PTY
```

Frames are a 4-byte big-endian length and UTF-8 JSON, with PTY bytes in base64. A connection opens with a `Hello` naming the protocol version, and a mismatch closes it with an error. Session names are limited to letters, digits, `-`, `.`, and `_`, so a name is safe on the remote command line `ssh` builds. On attach the server replays buffered output as `Scrollback`, so a new client rebuilds its screen. The PTY takes the smallest size among attached clients, and a larger pane letterboxes its grid.

The server never polls: listener, client, and session threads report into one channel the event loop blocks on, and a socket-free state machine (`server/dispatch.rs`) handles each event. A session is named with the generation it was created in, so a late report from a killed program never reaches the new session that reused its name.

`crates/app/winter-term/src/control.rs` reuses the framing for a one-shot control socket: `winter --reload` (reload config, or replace the running binary), `--edit-wait`, and `winter workspace` talk to a running window through it.

## Configuration and persistence

Configuration is KDL: `settings.kdl` and `keybindings.kdl` under `~/.config/winter-term/` (`%APPDATA%` on Windows). A user theme is a KDL file of its own: an optional `base "dark"|"light"` and a `colors` block over it.

On a clean exit Winter writes `$XDG_STATE_HOME/winter-term/session.json`; with `restore-session #true` (off by default) the next launch restores the layout, each terminal's directory, and the editors that can say where they were. A reload that replaces the binary hands over through `session-handover-<pid>.json`. Terminals start fresh shells, except `mux:` and `mux-remote:` ones, which reattach to their sessions.

## Entry points

The `winter` binary dispatches on its first argument: none opens the window; `mux` runs the multiplexer (`serve`, `list`, `new`, `kill`, `attach`, `attach --host`, `proxy`); `workspace` the workspace commands; `--reload` and `--edit-wait` talk to a running window; `--version` and `--help` print and exit.

## Testing

More than three thousand tests live beside the code, most in the model layer. Integration tests cover the PTY (`winter-term/tests/headless.rs`), shell integration (`tests/shell_integration.rs`), the crate layers (`tests/layers.rs`), and the mux end to end (`winter-mux/tests/mux_end_to_end.rs`). Two fuzz suites feed adversarial streams to the parsers: `winter-emulator/tests/vt_fuzz.rs` for the VT parser and grid, and `tbp_fuzz.rs` for TBP blocks.
