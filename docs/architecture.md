# Winter architecture

Winter is a terminal-first IDE built on a terminal emulator that models a session as a list of typed, MIME-tagged blocks rather than a flat scrollback buffer. This document summarizes how the workspace is put together: what each crate owns, how bytes from a PTY become pixels, and which invariants the design leans on. It describes the code as it stands, not a plan.

## Workspace layout

Nine crates, plus a vendored `portable-pty` with one Windows fix (`vendor/portable-pty`). Nothing below depends on anything above it, so the protocol and block model stay usable without dragging in a GPU stack.

| Crate                | Owns                                              |
|----------------------|---------------------------------------------------|
| `winter-proto`       | TBP v1: wire types, codec, trust tiers.           |
| `winter-core`        | Parser, scrollback, cell grid, VT screen.         |
| `winter-render`      | wgpu text and chrome renderer.                    |
| `winter-spell`       | Hunspell dictionaries, prose tokens, suggestions. |
| `winter-lsp`         | Language servers, and the store of problems found. |
| `winter-extension`   | The API extensions are written against.          |
| `winter-ext-example` | An example extension, built behind a feature.     |
| `winter-term`        | App: window, panes, tabs, modes, layout, mux.     |
| `winter-client`      | Emitting TBP blocks from Rust programs.           |

Each crate directory is named for its package, so `winter-core` lives in `crates/winter-core`. The exception is `winter-client`, which sits in `clients/client-rs` beside the Python and shell clients rather than under `crates/`.

```text
   winter (app binary)
       │
       ├──> portable-pty, blitz, hayro, syntect (PTYs, the preview, PDFs,
       │    highlighting)
       │
       ├──> winter-render ──> wgpu, glyphon, resvg, vello, kdl
       │         │
       │         ▼
       ├──> winter-core ──> vte
       │         │
       │         ▼
       ├──> winter-proto ──> serde, base64
       │
       ├──> winter-spell (no dependencies)
       │
       ├──> winter-lsp ──> serde_json
       │
       └──> winter-extension (no dependencies)
```

The protocol, core, render, spell, lsp, and client crates set `#![forbid(unsafe_code)]`. The app crate sets `#![deny(unsafe_code)]` and allows it only around a few system calls (sending signals, reaping children, the Windows console), each with a `SAFETY` comment. The protocol, core, render, spell, lsp, and app library crates also set `#![deny(missing_docs)]`.

## The byte pipeline

The terminal lives in two places, on the same model and window split as the rest: `crates/winter-term/src/terminal/` is the runtime (the PTY session, its parser, the block queue, and the image protocols), with no window in it, and `crates/winter-term/src/window/terminal/` is what a terminal tab offers on top of it in the window (blocks, modes and selection, Vim navigation and edits, search, the pointer). A change to what a program's output means goes in the first; a change to what a key or a click does to a terminal tab goes in the second.

A terminal tab spawns a PTY child and reads its output on a background thread. Those bytes go through exactly one `vte` parse pass, in `CombinedPerformer` (`crates/winter-term/src/terminal/session/performer.rs`), which fans each parsed event out to two consumers at once.

```text
        PTY child (shell)
                │
                │ bytes
                ▼
        CombinedPerformer          one vte parse pass
                │
        ┌───────┴────────┐
        ▼                ▼
    winter_core      winter_core
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

The single-pass design is the reason the visual grid and the block list can never disagree about ordering: a block is anchored to the grid row the cursor was on when its escape arrived, and those anchors are drained together with the block itself.

Before `vte` sees the bytes, a `ByteFilter` (`crates/winter-term/src/terminal/session/byte_filter.rs`) lifts out what `vte` 0.15 drops: `ESC _` APC strings, so the Kitty graphics protocol can be parsed (`crates/winter-term/src/terminal/kitty.rs`), while capping OSC strings. It hands everything else on as runs of the stream's own bytes, so `vte` parses printable text in bulk rather than one byte at a time. `CombinedPerformer` keeps separate Kitty keyboard flag stacks for the main and alternate screens, so a full-screen program cannot leak pushed flags back into the shell prompt.

`winter_core::Screen` pairs a `vte::Parser` with a `Grid` so a byte stream can be fed to one directly. Only that pairing is demo-and-test-only: it backs the headless path in `crates/winter-term/src/main.rs` and the fuzz suite. The parsing itself is not a second implementation. `impl Perform for Grid` lives in `crates/winter-core/src/screen.rs`, and `CombinedPerformer` delegates to it, so the CSI, SGR, and erase handling exercised through `Screen` is exactly what the app runs.

## One renderer, one screen

Everything is drawn by Winter itself, on the GPU, into the one window.

- **Text** goes to the GPU. `renderer::GpuRenderer` draws the `Grid` with `cosmic-text` and `glyphon` over a wgpu surface, alongside WGSL shaders for backgrounds, cursors, and images.
- **Everything else** is drawn on the canvas (`crates/winter-term/src/window/frame/canvas.rs`): one kind of content (`model::picture::Content`: encoded pixels, pixels already drawn, an SVG, Markdown, monospace text, or a Vello scene), drawn into a texture at the size it is shown, and placed as a GPU image cropped to where it may show. Inline blocks are placed in the band of rows they reserve; an editor's content (the image viewer's picture, a PDF's pages and the cursor and selection over them) in its pane. A Vega-Lite spec is drawn as an SVG by `crates/winter-term/src/chart/`, and HTML is turned into Markdown, with nothing in it run. The Markdown preview editor is laid out by Blitz and drawn as a Vello scene on the window's own wgpu device; it sits behind the default `preview` feature, so a build can leave Blitz out. Popups, menus, and notifications are drawn in the same pass, after, so they always sit on top.

Because a rich block's rendered height is not known when its escape arrives, a block reserves `BLOCK_RESERVE_ROWS` (12) rows in the grid at emit time so following shell output flows below it rather than under it. Raster images, SVG, and charts, whose sizes are known up front, reserve their exact height instead, capped at `MAX_IMAGE_ROWS` (24); text is laid out and its band fitted once it is drawn.

## The block model

`winter_core::Scrollback` is a state machine over semantic terminal events, deliberately independent of `vte` and the PTY so it can be driven directly from tests. `crates/winter-core/src/parser.rs` is the adapter that turns escape sequences into its method calls.

A session is a `Vec<CommandBlock>`. Each block carries the command line, its working directory, its exit code, and an ordered `Vec<Segment>`, where a segment is plain text, an OSC 8 hyperlink span, a one-shot TBP content block, or a live block. Block boundaries, commands, and exit codes come from OSC 133 shell integration marks; working directories come from OSC 7. Without shell integration the whole session is one rolling block, which is a degradation rather than a failure.

Live blocks are the interesting case. A tool opens a block, streams RFC 6902 JSON patches at it, and closes it. Each patch is folded into the block's current state as it arrives, so an update costs its own size, however many came before it. Folding is deliberately best-effort: a malformed patch operation is skipped rather than aborting the whole patch, because freezing a progress display at its initial state is a worse outcome for a terminal than showing a slightly stale one.

## Resource budgets

A terminal is expected to stay open for days, so nearly every unbounded accumulator in the design has an explicit cap with a documented rationale. These are the load-bearing ones.

- `MAX_RETAINED_OUTPUT_BYTES`, 8 MiB: without it a terminal that streams output grows the block list until the OOM killer intervenes. The grid caps its own scrollback rows, but that cap never reached the block list.
- `BUDGET_CHECK_INTERVAL_BYTES`, 1 MiB: measuring the budget is a walk of every retained block, so it is amortized over this much growth rather than run on every write.
- `MAX_COMMAND_BYTES`, 4 KiB, and `MAX_CWD_BYTES`, 4 KiB: a program can mark a command's start and never its end, or report an absurd working directory; neither may grow past these.
- `MAX_BLOCKS`, 50,000: past it, output carries on in the last block, which the byte budget bounds, rather than a loop of prompt marks starting blocks without end.
- `MAX_PATCH_OPS`, 256 (in `winter-proto`): the most operations one patch applies; an emitter with more sends a whole-document `replace`.
- `MAX_SCROLLBACK`, 10,000 rows: the grid's own history ceiling.
- `CLIENT_OUTBOX_LIMIT`, 4 MiB: a mux client this far behind is dropped rather than buffered without bound. The server's memory is not traded for the illusion of a live connection.
- `APC_MAX_PAYLOAD`, 4 MiB: guards against a malformed or unterminated APC sequence bloating memory. An APC string past it is dropped to its end, never printed.
- `MAX_OSC_BYTES`, 16 MiB (`winter-core/src/osc_guard.rs`): an OSC string longer than this is cut off, in every parser, since `vte` buffers one without limit.

Retention uses elision rather than removal: when a block exceeds the byte budget its output is dropped but the block itself stays in the list, because a block's index is a stable identifier held by the app layer for folds and the blocks drawn on the canvas. Removing entries would silently renumber every one of them.

## The protocol

TBP is one OSC escape per message: `OSC 9001 ; <verb> ; <params> ; <base64 payload> ST`. Verbs are `emit`, `open`, `patch`, `close`, and `caps`. The payload is base64-encoded JSON, which keeps the whole escape opaque to terminals that do not implement TBP; they ignore it wholesale.

The payload is a MIME bundle, modeled on Jupyter's `display_data`: the emitter supplies several representations of the same content, and the terminal picks the richest one it can render, falling back toward `text/plain`. Every bundle carries a `text/plain` fallback, so a program using the client libraries stays safe under `tmux`, `ssh`, or CI. `clients/client-rs` prints the fallback and turns later `update`/`close` calls into no-ops when Winter is not the active terminal.

## Trust

Every byte reaching the terminal is attacker-controlled. A `cat` of a downloaded file, output piped from `curl`, or a program on the far side of `ssh` can all spell `trust=trusted`, and nothing on the wire authenticates the emitter. Winter's answer is to have nothing a tier could grant: every block is drawn natively, nothing in one is run, and nothing is fetched for one. The protocol still carries `TrustTier`, and `TrustTier::clamp_to` still only ever lowers a requested tier, for clients and other terminals that render blocks in a web engine; Winter accepts the tier and draws the block the same way whatever it asks for.

## Interaction model

`crates/winter-term/src/model/` is the window-free layer: modes, split-tree geometry, key resolution, the picker, and the settings page, with no side effects of its own. It is not yet free of the layers above it: the picker reads the command table in `window/commands.rs`, and the editor contract names a language-server edit type and the app's `Config`. Its way out to the OS is `model/picker/disk.rs`: the file browser reads directories through a `DirSource` trait the app hands it the real filesystem for, and the history picker opens over a history file's bytes, so both are tested over fixed data. This is what makes the vim layer testable without a window, and it is where most of the test suite lives.

The mode machine is per tab and has three states. Insert encodes keys to bytes for the PTY. Normal intercepts them as motions, operators, and layout actions. Visual is Normal with motions extending a selection.

One transition is deliberately asymmetric: `Esc` in Normal stays in Normal. Only `i`, `a`, or `o` return to Insert, so the key that means "stop what I am doing" everywhere else cannot drop the next keystroke into the shell mid-navigation.

Layout is one binary split tree per window (`crates/winter-term/src/model/layout.rs`): leaves are panes, each holding a strip of tabs, and internal nodes are splits with a ratio clamped to `[0.1, 0.9]` so a pane can never be squeezed to zero. A `TabId` names one tab, not a pane; the owner allocates them so they stay unique across the window.

`crates/winter-term/src/window/mod.rs` wires this to `winit`, with submodules splitting the `App` by concern: `runtime` for bootstrap, lifecycle, and the event loop, `input` for key and mouse routing, dispatch, and key sequences, `frame` for frame composition, `layout` for panes and tabs, `editor` for editor tabs and what the window does for them, `picker` and `settings` for the picker and the settings page, `workspace` for workspaces, `appearance` for themes and settings in force, `macro_record` for keyboard macros, `tabbar`, `menus`, `strip`, `status`, and `notices` for what surrounds the panes, and `terminal` for blocks, navigation, search, and the pointer.

The state those submodules share is grouped into types rather than sitting flat on `App`. Five groups gather the rest: `EditorHost` (what the window keeps for its editors beyond the tabs: jobs, questions, builds, language servers, extensions), `Chrome` (the menus, the overlay holding the keyboard, jump labels, the status bar's hover, notifications), `RuntimeState` (the event loop's housekeeping and clocks), `InputState` (modifiers, the wheel, pending keys, input methods, focus, clipboards), and `TerminalState` (what terminal tabs keep beyond their sessions). Beside them sit `SearchState` (the live `/` search), `MenuState` (the menubar and context menu), `PointerState` (hover, drags, click timing), `SelectionState` (the selection span and the Visual mode behind it), and `VimState` (jumplist, changelist, marks, registers). A method that touches one group alone is that group's, so `self.chrome.picker()` reads the overlay without reaching into the rest of `App`; every field is still visible to the whole `window` module.

## Normal mode

Normal mode is what lets the vim vocabulary work against any shell without shell-side configuration: Winter owns the keyboard while it is active, so those keys never reach the PTY and nothing has to be installed on the far end of an `ssh`. It is also the largest subsystem in the application, roughly 12,000 lines across the key resolver, the mode machine, and the navigation submodules.

### Resolving a key

`resolve_with` takes the pane's mode, the key event, and a mutable `PendingPrefix`, and returns an `Action`; Insert, Normal, and Visual each have their own resolver behind it. The pending prefix is what makes multi-key sequences work without a parser. It is an explicit state machine holding what has been typed but not yet spent, so `d`, `i`, `w` arrives as three separate key events that leave `Delete`, then `DeleteObject { around: false }`, then a resolved action.

Counts live in the same enum (`Count(usize)`, accumulating digits until a motion spends them), alongside the operator-pending states, the `[` and `]` bracket prefixes, and `C-w` for window commands. Because the prefix is a value rather than hidden control flow, a half-typed sequence can be rendered: the which-key popup is a view over it.

### Motions and text objects

The motion primitives are pure functions over a `&Grid` with no `App` state involved, which is why they are cheap to test. A motion never lands on punctuation, which over code is a cursor stop nobody asks for: small-word motions (`w`, `b`, `e`) cross it the way they cross a blank, so `w` over `foo.bar` goes straight to the `b`, and big-word motions (`W`, `B`, `E`) take it into the run beside it, so `foo.bar` is one word. Text objects (`iw`, `aw`) classify the vim way instead, keyword runs and punctuation runs apart, since a selection that stops at a punctuation boundary is one worth having.

The spans themselves are pure functions over rows of characters, in the Vim foundation rather than in the grid: a row's words and quoted runs, and the bracket pairs, paragraphs, and sentences that may cross rows, reached through a small `TextRows` trait that the grid and the text editor's buffer each answer. One implementation of `iw` serves both surfaces, so an object fixed in one is fixed in the other.

A text object pairs a kind (word, big word, a quote character, or a bracket pair) with an `around` flag, which is the `i` versus `a` distinction. Visual mode carries a selection kind so characterwise, linewise, and blockwise selections share one code path.

### What persists, and where

Most of this state is per tab, keyed by `TabId`: the jumplist, the changelist, the last change that `.` replays, and marks. Registers are the deliberate exception, one `char`-keyed map shared across every tab, so a yank in one tab pastes in another.

The jumplist keeps 100 origins and forks like undo history when you jump after stepping back. The changelist reuses that structure but is append-only, because a changelist is a log of historical facts rather than a tree to navigate, and consecutive duplicate positions collapse so repeated edits at one spot do not fill it with noise.

### Editing the shell's prompt line

Normal mode never forwards keys to the PTY, which is a problem for operators aimed at the command line the shell is currently editing, because the shell owns that text and Winter cannot edit it directly. So a prompt-line delete is translated into the readline keystrokes that produce the same result: arrow keys to move the line editor's cursor under the vim cursor, then a kill.

That translation assumes the shell's default emacs-mode bindings. A shell already in vi mode has those chords bound elsewhere, which is what the `prompt-edit-bindings "none"` setting exists for: it tells Winter to decline prompt-line operators and leave the line to the shell, which provides vim editing there anyway. Undo delegates to readline's own undo rather than replaying the edit backwards, so line-editor plugins that redraw on every keystroke repaint once instead of flashing through a rebuild.

## Editors

Not every tab is a terminal. A tab can instead be an *editor*: content Winter paints itself, as rows of styled text rather than PTY output. `crates/winter-term/src/model/editor.rs` defines what an editor is, `crates/winter-term/src/model/paint.rs` the styled rows it paints (a `Frame` of `Row`s of `Span`s, each in a semantic `Style`), and one module per kind of editor lives under `crates/winter-term/src/editors/`, testable without a window. Editors do their own file and process work where they need it (the listing, the PDF and image viewers, the preview), the slow parts off the window's thread. An editor names semantic styles (accent, dim, header, normal) and never a color, so the active theme resolves them.

Nothing slow runs on the window's thread, and work leaves it one of three ways, picked by its shape:

1. **A job** (`window/runtime/jobs`): a git command, a shell command, a file read. The editor returns a `JobRequest` in its outcome, the window runs it, and the answer comes back to the editor's `on_job` under the tab that asked, even if the tab was closed and its work handed to another view. Use it for anything the job types already describe.
2. **A `Background`** (`editors/background.rs`): one piece of work the job types have no shape for, answering once, such as a directory tree read to a budget, edits written back, or a watch set up. Starting another in its place or dropping it cancels it, so a stale answer can never land, and its answer wakes the window. A program run for its text with a deadline goes through `editors/run.rs`, which also refuses to start a program again while an earlier run of it is stuck in the kernel.
3. **A thread of its own**, only for work that lives as long as its editor and answers more than once: a build streaming its output (`compile/runner.rs`), the PDF worker serving page requests (`pdf/worker.rs`), a file watcher (`crate::watch`, with the directory and git watchers). A cache every editor shares, which no one editor could own a `Background` for, loads once the same way (`editors/dictionary.rs`). Each wakes the window or is read when it has news.

An editor is a tab of its own, a peer of the terminal. `window/layout/open_tabs.rs` holds every open tab by `TabId` as one enum, a terminal or an editor, so a tab is never both and code that wants a terminal asks for one and gets `None` for an editor. What only a terminal has (its mode, its traversal cursor, its folded blocks, its prompt model) lives with the terminal in the same entry and goes when the tab does; an editor is in no mode, and keys it declines are resolved against the window chords alone. The editor sits in a pane's tab strip beside the terminals, which go on running, unseen, while it shows. The split tree, focus, and zoom are untouched, since they only ever deal in ids. A session snapshot saves each editor that can say where it was looking (a listing's root, the text editor's file and line), by its kind's name. An editor paints into a grid of its own, which then goes through the same per-pane view the renderer draws a terminal through, so an editor tab costs the renderer nothing new. It is handed its row count before it paints, so a listing longer than the pane returns the window it wants visible: scrolling belongs to the editor, which is the only side that knows where its cursor is. A pane showing an editor is one nothing else may draw into, so its rich-block tiles and images are held back for as long as the editor is up, on the same rule that hides them under a full-screen app.

Slow work follows the same shape. An editor asks for it, a detached worker thread does it, and the answer is collected on the next event-loop poll and handed back to the editor, which then asks for the next piece. The work is either a program to run or a walk of the filesystem, and a walk is Winter's own rather than a shelled-out program, so a directory total and a text search behave the same on every platform and every accumulator they fill has a cap. An editor that must stay current without being asked, such as the process monitor, names its next deadline and is woken for it: the event loop sleeps until the earliest deadline of any editor showing, and a request the runner dropped is retried when the editor's give-up deadline passes. Requests past a fixed number in flight are dropped rather than queued, since every caller is asking about what is on screen and a queued answer would be about a screen the user has left. Cancelling forgets the job rather than killing the thread: the worker notices a flag and exits, and an answer that arrives anyway is discarded because the job is no longer one anybody is waiting on. While any job runs, the loop keeps its busy poll interval so a finished one is never left sitting in the channel.

An editor may hold more than one thing at a time where that is what the thing is: the text editor keeps a list of open files and shows one of them, so a second file opened while it is up joins it rather than covering it. The host offers the file to whatever editor is up and falls back to opening an editor of its own when it is declined, which keeps the rule that an editor owns what is drawn in its pane.

Opening a file from a listing adds the text editor as a tab right after the listing, and closing a tab shows the one shown before it in the same pane, so the listing is still where it was left. An editor put back from the stash of closed tabs is told so, and re-reads whatever it was showing, since a file saved while it was gone is exactly the kind of thing that changes what it says.

An editor that needs an answer from the user asks for one rather than reading the keyboard itself: it returns a prompt request carrying a tag, the host collects the answer in a dialog in the middle of the window, and the tagged answer comes back to the editor. A question whose wrong answer cannot be undone is collected as a single key instead of a line. That way one input path serves every editor, and an editor never has to know how the question is drawn.

Reports go the other way on the same principle. An editor never draws what it has to say ("wrote 12 lines", "nothing staged to commit", a failed command) in its own rows: it queues a notice, an error or a confirmation, and the host takes it with `Editor::take_notice` after every call it makes into the editor and shows it in the window's status bar. Taking it after the call, rather than as one more outcome, lets an editor report and ask for something else at once, such as the job that re-reads the tree after a push. The shared text-editing keys queue their own complaints ("nothing to undo") the same way, so the text editor, Keys, and the edit modes of a listing and of a search all report where every other editor does. The status bar is also where an editor says where its keys stand: `Editor::status` gives the editor's mode, shown with the same letter and color as a terminal's, and short segments the bar draws at its right edge, such as the cursor's line and column, a PDF's page, or how much is marked. What an editor shows in its rows is what it is about and its state, which lasts until it changes: a file's name, a query and its counts, the toggles in force, or a body saying a repository could not be read.

Every notice, an editor's or the window's own, goes through one entry point, `App::notify`, into a `model::notify::Notifier`. The notifier is plain data on a clock the caller passes in: a `ToastStack` of what is on screen (expiring by severity, held while the pointer rests on a card, a repeat refreshing the newest card rather than stacking) and a `NotificationLog` of the session's history, whose unread errors and warnings drive the status bar's bell. The app only decides where a notice is drawn (warnings and errors as toast cards, confirmations in the status bar, everything in the bar with `notifications { toasts #false }` and everything as a card while the bar is hidden) and maps severities to the renderer's colors; the renderer keeps where it last drew each card and the bell in its `FrameHits`, and answers `hits().toast_at` and `hits().status_bell_at` from that, so a click is tested against what is on screen rather than against a second copy of the layout.

Searching is shared on the same principle. `/` is a prompt like any other, and the answer goes to one matcher the editors hand their own rows to, so `/`, `n`, and `N` mean the same thing in every editor. The match is literal text rather than a pattern, which is what keeps the editor layer free of the regex engine the scrollback search needs. Each editor says what text stands for one of its rows, usually the row as painted, so the key list is as searchable by chord as by command.

The pointer reaches an editor the same way: a press, a drag, and a turn of the wheel are offered to the editor showing in the pane before any terminal grid sees them, and an editor that answers none of them leaves the terminal's own selection untouched. The text editor is the one that takes all three, so a click lands the cursor on the character under it and a drag selects from where the button went down.

The `Editor` trait holds only what any editor may do, each method with a default so an editor implements the ones it needs. What only one kind of editor does (the text editor's language-server hooks, Git's status view, a PDF's SyncTeX mark) is that editor's own method, and the window reaches it by type, through `with_kind`, which every editor gets through a blanket `AsAny` impl. A new editor therefore reads one general trait rather than every other editor's needs, and a hook belongs on the trait only once a second editor would use it. What several editors share without being an editor sits beside them under `editors/`: `text_file` reads a file into lines and writes it back losslessly, `git_exec` builds the git commands Winter runs and `git_parse` reads what they print, `highlight` colors source by its TextMate grammar, `word_diff` finds the words that changed between two lines, `synctex` maps between a LaTeX source and its PDF, `units` formats sizes and shares, and `list`, `background`, and `refresh` serve the listings, so no editor reaches into another's insides. Settings reach every editor the same way, through one `configure(&Config)` before each paint, each editor taking what it uses.

An editor tab is in no mode, which is what keeps its keys honest. Keys reach the editor after the globally-intercepted chords and before anything else; a key the editor declines falls through to the window-layout chords alone, so splitting, closing, zooming, and moving focus all still work from an editor, while a vim motion or a mode switch resolves to nothing rather than acting on a tab that has no grid and no PTY. No key switches an editor tab into a mode, so the tab holds its editor until it closes.

## Commands

Every command Winter runs by name is one entry in `crates/winter-term/src/window/commands.rs`: its id, what the palette calls it, and what running it does. Most run on the window with the tab in front; the rest are handed to the focused editor by id, or are a key binding's action the palette offers too. The palette lists the table, a binding is checked against it, and `App::run_command` runs from it, so a command can be neither listed and dead nor live and unlisted; a test holds every entry to the kind it claims. A command an extension adds comes after them, by its own id.

## Extensions

An extension is a Rust crate written against `winter-extension`, which depends on nothing, compiled into Winter behind a Cargo feature of its own and listed in `crates/winter-term/src/extensions.rs`. Each is activated once into one registry: it adds commands, status bar segments, completion back-ends, and views (tabs it draws), and asks to hear of events: a file opened, closed, saved, edited (once the typing pauses), or brought to the front. Everything it does goes through a narrow `Host` (`crates/winter-term/src/window/extension_host.rs`), which runs on the window's thread, so slow work goes to `spawn`, which runs it on a thread of its own and hands the result back. A panic in an extension is caught, named in a notice, and stops only that extension. What the editors show is watched in one place, compared each pass of the event loop with what extensions were last told, so a file is heard of however a tab came to show it. `crates/winter-ext-example` is a working template: an on-save and as-you-type TODO linter, a status segment, a completion back-end, and a board view, built with `cargo run --features example-extension`.

## Multiplexer

`crates/winter-term/src/mux/` is a headless PTY session manager. The server owns PTY processes and outlives client disconnects; clients attach to named sessions over a Unix domain socket, or over an SSH tunnel to a remote server.

```text
   Winter (client) ──> Unix socket ──> mux server ──> PTY
   Winter (client) ──> SSH tunnel  ──> remote mux  ──> PTY
```

Frames are 4-byte big-endian length prefixes followed by UTF-8 JSON, chosen to stay debuggable and language-agnostic, with PTY bytes carried as base64 strings. A connection opens with a `Hello` naming the protocol version; the server answers with its own, or with an error and a closed connection when they differ, and a frame it cannot decode is answered with an error rather than dropped. Session names are limited to letters, digits, `-`, `.`, and `_`, so a name is safe as a word on the remote shell's command line that `ssh` builds. Two details carry design weight. On attach the server replays buffered output as a `Scrollback` message before live output resumes, so a fresh client rebuilds its screen rather than starting blank. And PTY geometry is server-arbitrated at the smallest geometry among attached clients, with a `Resized` message to every client, so a pane larger than the session letterboxes its grid instead of rendering a stream wrapped for a width it does not have.

The server does not poll. A listener thread accepts connections, each client has a reader and a writer thread, and each session a reader thread; all of them report into one channel that the event loop blocks on. The loop (`mux/server/mod.rs`) hands each event to a socket-free state machine (`mux/server/dispatch.rs`), with size arbitration in `mux/server/size.rs` and the per-client threads in `mux/server/connection.rs`. A session is identified by its name and the generation it was created in, so a late output or exit report from a killed program is never mistaken for the new session that reused its name; a session's clients are detached when it ends. A reconnecting client attaches without asking for the replay, since it already shows that history.

`crates/winter-term/src/control.rs` reuses the same framing for a separate one-shot control socket, used by `winter --reload` to ask a running instance to reload its configuration, or with a new binary on disk to replace itself with it, and by `winter --edit-wait` and `winter workspace` to open files and switch workspaces in a running window. The module doc is explicit that this is framing reuse only, not a second multiplexer.

## Configuration and persistence

Configuration is KDL, split across `settings.kdl` for appearance and behavior and `keybindings.kdl` for bindings, under `~/.config/winter-term/` (`%APPDATA%` on Windows). User themes are separate KDL files: an optional `base "dark"|"light"` plus a `colors` block layered over it.

On clean exit Winter writes a session snapshot to `$XDG_STATE_HOME/winter-term/session.json`, and with `restore-session #true` (off by default) restores the split layout, each terminal's working directory, and the editor tabs that can say where they were on the next launch. A reload that replaces the binary writes a handover of its own, `session-handover-<pid>.json`, which the new process image reads and deletes. PTY children are spawned fresh, not reattached; panes labeled with the `mux:` or `mux-remote:` prefix are re-attached to their session instead of respawning a local shell.

## Entry points

The `winter` binary dispatches on its first argument: no argument opens the window, `mux` routes to the multiplexer subcommands (`serve`, `list`, `new`, `kill`, `attach`, `attach --host`, `proxy`), `workspace` to the workspace commands, `--reload` and `--edit-wait` talk to the control socket of a running instance, and `--version` and `--help` print and exit.

## Testing

More than three thousand `#[test]` functions live beside the code, concentrated in the pure model layer. Integration tests cover the PTY (`crates/winter-core/tests/pty.rs`), shell integration, and mux end-to-end behavior. Two fuzz suites feed adversarial streams at the parsers: `crates/winter-core/tests/vt_fuzz.rs` for the VT parser and cell grid, and `crates/winter-core/tests/tbp_fuzz.rs` for the TBP block parser.
