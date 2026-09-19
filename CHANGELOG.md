# Changelog

All notable changes to Winter are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

While the major version is `0`, the TBP wire format, the KDL config schema, and every crate's public API may change in a minor release.

## [Unreleased]

### Added

- **Tool panes.** A pane can hold a *tool page*: content Winter paints itself as rows of styled text, drawn over the pane while the shell underneath keeps running. A page is a fifth per-pane mode, and keys it declines still reach the window chords, so splitting, zooming, and moving focus all work from inside one.
- **Dir** (`Ctrl-Shift-d`): a keyboard-driven directory listing rooted at the pane's working directory, with tree folding, visit history, marks, copy, move, rename, delete, permission bits, sorting by name, time, or size, and directory totals walked in the background.
- **Git** (`Ctrl-Shift-g`): the working tree as foldable sections with hunk-by-hunk diffs. Hunks stage, unstage, and discard one at a time through `git apply`, and menus drive branch, commit, merge, rebase, tag, stash, cherry-pick, bisect, remote, reset, revert, worktree, and log. `Alt-b` blames, `Alt-y` lists refs, `Alt-g` opens the remote, and `Enter` on a commit reads it as a list of what it changed. Anything wanting a terminal of its own goes to a new tab.
- **Grep** (`Ctrl-Shift-s`): the lines under the working directory holding some text, grouped by file, with `n`/`N` between matches and `Alt-n`/`Alt-p` over whole files. The walk is Winter's own rather than a shelled-out `grep`, runs off the event loop with `Esc` to abandon it, skips version-control internals, `node_modules`, `target`, files over a megabyte, non-text files and symlinks, and caps at 500 matches.
- **Editor**: the file a tool was pointing at, as editable text, opened over the listing so closing it puts the listing back where it was left. Vim Normal, Insert, and Visual modes, `Ctrl-s` to write, `ZZ` to write and close, `ZQ` to discard. It edits text and nothing else: no completion, no language server. `Ctrl-o` hands the file to `$EDITOR` in a tab of its own.
- **Syntax coloring in the editor** for Rust, Python, Go, TypeScript, JavaScript, C, C++, shell, JSON, and the config formats, chosen by the file's extension. A hand-written lexer rather than a grammar, so there is no new dependency and an unknown language is painted as plain text rather than guessed at.
- **A file browser in the command palette** (`Ctrl-Shift-f`): walks the working directory a step at a time, typing to filter, `Enter` or `Right` to descend, `Backspace` or `Left` to go up, and `Enter` on a file to open it in the editor. One directory read per step, so it opens at once however large the tree is.
- **Keys** (`Keys: Show Every Command`): every built-in command beside the chord bound to it, read from the keymap in force so it cannot disagree with what the keys actually do.
- **Every question is asked in the middle of the window.** A tool asking for a name, a destination, or a yes-or-no opens a floating dialog where the command palette opens, rather than writing into the status bar.
- **A picker every tool can ask with.** A page that knows the answers to its own question hands them back and the host shows them as a filtering list. Checking out, merging, and rebasing onto a branch, deleting a branch or a tag, and removing or pruning a remote all pick from a list now instead of typing the name.
- **Search in every tool**: `/` asks for text, `n` and `N` step through the matches and wrap. One matcher serves every page, matching literal text without regard to case, which keeps the page layer free of a pattern engine. Grep keeps `/` for a new search, since its rows already are one.
- **File-type icons in Dir and Git**, from a bundled set of 1481 icons keyed by 833 extensions, 317 exact filenames, and 374 directory names. The `icons` setting picks `"svg"` (the default), `"font"` for Nerd Font glyphs, or `"none"`; a row leaves the columns blank whichever is chosen, so switching never reflows a listing.
- **A text cursor in the tool panes.** Dir and Git draw a block cursor on the active row's first non-blank column, and `v` starts selecting from there with the Vim motions, `V` for whole lines, `y` or `Enter` to copy.
- **Key hints for every multi-key Git command**, on the same centred card the terminal's own prefixes use, shown at once rather than after a pause. Any page with a multi-key command gets one, including the `g` jump leader. The menus used to be rows drawn under the view, so in a repository taller than the pane they fell off the bottom.
- **`.` opens a file menu** in Git: stage, unstage, discard, diff, log, or blame the file under the cursor, instead of six keys spread across the page.
- **The status view says what git is part-way through.** A `State:` line names an unfinished merge, rebase, cherry-pick, revert, or bisect, the step it is on, and the keys that continue, skip, or abort it. Read from the repository's own state files, since a porcelain status reports none of it.
- **A stashes section, and a section for what the upstream is holding.** Stashes list in the status view, each opening as a diff with `Enter`; the commits the upstream has and this branch does not get their own section under the remote's name.
- **`Enter` opens the file a row belongs to**, everywhere a row has one: a path in a Git section at the top, a hunk at the line it changes, a blame row at its own line, and a `src/foo.rs:42` reference under `gx` in the editor rather than in a new tab. A row standing for a commit, a stash, or a section heading belongs to no one file and keeps the key.
- **`_` in Dir creates a file and opens it** in the editor, with the listing left on it for when the editor closes.
- **Every setting is editable in the settings page.** Ten rows only the config file could reach before: dimmed inactive panes, the pane divider's width, session restore, the prompt-line binding set, ligatures, cursor blink and hiding, clipboard reads, and the two block-trust settings.
- **Slow work off the event loop.** A page asks for work, a detached worker does it, and the answer is collected on the next poll. Requests past a fixed number in flight are dropped rather than queued, since every caller is asking about what is on screen.
- **`Alt-Shift-,` and `Alt-Shift-.` reach the top and bottom of any tool page**, answering Emacs' `M-<` and `M->` beside `gg` and `G`.
- **The sentence text object (`is`/`as`).** Up to a `.`, `!`, or `?` and the quotes or brackets that close after it, read within its own row. A stop only ends a sentence when a blank or the row's end follows, so `v1.2.3` and `main.rs` stay whole.
- **The paragraph text object (`ip`/`ap`).** The run of lines around the cursor that are all blank or all not, which over command output is one block of it: `yap` copies a command's output, `dip` clears a paragraph off the prompt.
- **The marks Vim keeps on its own**: `` ` ``/`''` to where the last jump started, `` `. `` to the last change, `` `^ `` to where Insert was left, and `` `[ ``/`` `] `` to the ends of the last yank.
- **An edit costs what it changed, not what the file holds.** Undo keeps the lines a command replaced rather than a copy of the whole file, so a keystroke on a 2.3 MB file went from 1.9ms and 2.3 MB of memory to under a microsecond and a few bytes. Scrolling to the end of 50,000 lines went from 17.8ms to 0.5ms.
- **Files the editor will not open say so** rather than opening wrongly: anything holding a NUL byte, anything that is not UTF-8, and anything over four megabytes. Saving goes through a temporary file and a rename, so an interrupted write leaves the previous contents whole, and permission bits, CRLF endings, and a missing final newline all survive the trip.

### Changed

- **`y` is an operator.** `yy`, `Y`, `y{motion}`, and `yi{object}`/`ya{object}` copy straight out of the scrollback, where a yank used to mean going through Visual first (`viwy`). `"{reg}` still chooses where the text lands, and the block yank moves to `gy`.
- **A yank lights what it took.** The span a `y` copied stays highlighted for a moment, the way vim-highlightedyank does it, so what landed in the register is visible rather than only reported.
- **The editor answers to the Vim the rest of Winter does.** Visual mode by character and by line, text objects under an operator, the motions that were parsed and then dropped (`{`, `}`, `%`, `ge`, `gE`, `g_`, `zz`/`zt`/`zb`), the char searches with `;` and `,`, literal search with `/`, `?`, `n`, `N`, `*`, `#`, marks, `.` to repeat the last change, `D`, `C`, `s`, `S`, `X`, `R`, `>>`, `<<`, `Ctrl-a`, `Ctrl-x`, and a count either side of an operator.
- **One editor, many files.** Opening a file while the editor is up adds a buffer instead of covering it, and opening one already open goes to it. `]b` and `[b` step through them, `gb` lists them, and `q`/`ZZ` close one at a time. Buffers share the registers, the searches, and the last change; each file keeps its own marks, undo history, and cursor.
- **Yanks in the editor can leave it.** Named registers `"a` through `"z`, with `"+`/`"*` the system clipboard, where a yank used to go to one register that died with the page.
- **No word motion stops on punctuation.** `w`, `b`, `e`, and `ge` now cross punctuation the way they cross a blank, so `w` over `foo.bar` lands on the `b` instead of the `.`. `W`, `B`, `E`, and `gE` take the punctuation into the run beside it, so `foo.bar()` is one word to step over or delete whole. `iw` and `aw` are unchanged.
- **One implementation of the text objects, for every surface.** `iw`, `a"`, `ip`, `is`, the bracket pairs, and the `%` match are now pure functions over a small `TextRows` trait that the grid and the editor's buffer each answer, so an object fixed in one place is fixed everywhere.
- **Every wrap folds at word boundaries.** Tool pages split words mid-token at the pane's edge; they now fold before the last word that fits, the same fold the terminal's own `wrap-words` makes. The height a page measures a row by and the lines it paints come from one algorithm now, so a wrapped row reserves exactly the rows it draws.
- **The tool pages start at the pane's left edge.** Git and Grep nested their rows under one another, up to six columns in. Every row now opens at column zero, what it belongs to is read off the band above it, and the columns go to the text instead.
- **A new line keeps the indent of the one it came from.** `Enter` in the middle of an indented line used to drop the cursor back to column zero, which is the indentation retyped on every split.
- **The editor takes the mouse.** A click puts the cursor on the character under the pointer, dragging from it selects, and the wheel scrolls the file rather than the shell hidden behind it.
- **A file changed on disk is read again** when an editor with nothing unsaved in it comes back to the front. With unsaved edits it leaves your work alone and still asks at save time.

### Fixed

- **Winter no longer burns CPU sitting idle.** A repaint counted as activity for the event loop's idle back-off, and the cursor blink asked for one roughly twice a second, so the loop never left its 16ms poll and a window that was not even focused kept repainting a cursor it draws steady anyway. Blinking now stops with focus, and an idle window went from 66 wakeups a second to 4. The repaint flag every state change sets is read now too, where before those repaints only landed because the blink happened to ask for a frame.
- **`ys` reaches the surround prompt.** `y` resolved straight to a block yank, so the state behind `ys{object}{char}` was unreachable and the keys after it fell through as ordinary commands. With `y` an operator it opens the way `ds` and `cs` always have.
- **Selecting text in a tool pane copies what is on screen.** A selection was resolved against the hidden terminal grid rather than the page's, so dragging across a Dir listing highlighted nothing and copied whatever had scrolled past before the tool opened.
- **Selecting text with the mouse works again inside a full-screen app.** A drag starting within a couple of rows of the pane's edge collapsed to the cell it began on: the edge auto-scroll pinned the drag's live end to the edge row about sixty times a second whether or not the view had actually moved. The alternate screen has no history at all, so every drag over vim or htop hit this on its first row.
- **A selection drag carried past the pane's edge keeps selecting.** The drag now belongs to the pane it started in: off the bottom it runs to the end of the last row, off the top back to the start of the first, and beside the pane it keeps the row and pins the column.
- **Scrolling a full-screen app drops the selection** instead of leaving a stale highlight over rows the app has since repainted, as other terminals do. A scroll driven from the app's own keys is not covered, since nothing on the wire distinguishes it from any other repaint.
- **The alternate screen no longer scrolls into the shell's scrollback.** The retained rows belong to the primary buffer, so scrolling a full-screen app's viewport back through them painted unrelated shell output over the app.
- **One wheel report per notch is forwarded to an app that tracks the mouse.** Each notch was sent once per line Winter would have scrolled its own view, so an app received three per notch and jumped three times too far.
- **Saving settings no longer drops the keys the writer did not know about.** `dim-inactive`, `restore-session`, and the cursor's `hide-in-inactive` were never written back, so any of them set by hand was lost the moment anything else was saved from the settings page.

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

[Unreleased]: https://github.com/taquangtrung/winter-term/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/taquangtrung/winter-term/releases/tag/v0.1.0
