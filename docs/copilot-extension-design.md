# Copilot extension design

This document is the agreed design for turning GitHub Copilot into an extension built on `winter-ext-api` alone, compiled to WebAssembly and shipped inside Winter's packages. It describes a plan, decided on 2026-11-05, not the code as it stands; [architecture.md](architecture.md) describes the code.

## Problem

Copilot is built into two crates: the suggestions in `winter-text` (`src/inline/copilot.rs`, 270 lines) and the server, its install, and the sign-in in `winter-term` (`src/window/editor/copilot.rs`, 510 lines), with its settings in `winter-term`'s config and its document versions in the text editor's documents. Nothing of it is open to anyone else: a company adding its own assistant, or a paid completion of Winter's own, would have to be built into Winter the same way, and closed code cannot be compiled into the open binary.

## Approach

Copilot becomes a bundled extension: a crate that uses only `winter-ext-api` and `winter-ext-guest`, built as a component, and installed with Winter beside the binary, where it loads like any extension a user installs. Everything it needs that the SDK lacks is added to the SDK for every extension, compiled in or WebAssembly alike, so the API a third party or a paid extension builds on is the one Copilot proves.

- Bundled, not compiled in: the closed and third-party assistants this is for can only ship as components, so the component path is the one that has to work, and Copilot, used every day, is what keeps it working.
- The sandbox stays shut: an extension never starts a process, opens a socket, or reads the user's files itself. It asks the host, which does each of these within what the extension is granted.
- Documents never cross into the sandbox on every keystroke. The host keeps a server the extension runs in step with the open files itself, as the language-server hub already does, so the boundary carries only the requests and their answers.

## Layout

```text
crates/
  app/         winter-term
  tools/       winter-dir, winter-docker, winter-git, winter-monitor,
               winter-pdf, winter-text
  base/        winter-base, winter-core, winter-lsp, winter-model, …
  sdk/         winter-ext-api, winter-ext-guest
  extensions/  winter-copilot, winter-ext-api-example
```

- `extensions/` is a new layer whose crates may depend only on `sdk`: the same kit an extension author outside Winter has. The layer test enforces it, and `winter-ext-api-example` moves there as the template it already is.
- `winter-copilot` is a `cdylib` built for `wasm32-wasip2`. It also builds for the host, so `cargo test --workspace` runs its tests of the protocol without a component.

## What the SDK gains

Each addition goes into `winter-ext-api`'s Rust API, the WIT package (`winter:extension`), `winter-ext-guest`, and `winter-wasm`'s host side in one pass, with a test through a component for each.

### Inline suggestions

- An extension registers an inline source with `Registry::inline(name, title, provider)`, the provider an `InlineProvider` answering `ask`, `shown`, and `taken`. What it offers it says with `Host::offer_inline`, as its settings and its server change, so Winter asks nothing of it on a keystroke to learn that.
- `InlineRequest` names the ticket, the file and its URI, the cursor (line and UTF-16 column, as language servers count), the kind (ghost text or next edit), whether `C-\` asked for it, the indentation in use, and the version of the document the extension's servers were handed.
- An answer comes later through `Host::answer_inline(ticket, answer)`, since every assistant asks a server. `InlineAnswer` is ghost text (each suggestion's text, the range it replaces, its receipt) or one next edit (a range and its text, with a receipt), or nothing.
- A receipt is a string the extension makes and reads back; the host only carries it, so what an assistant is told of a suggestion seen or taken is its own business.
- `winter-text` keeps its `InlineSource` slot, the ghost text, the next-edit card, and every key that works them (`Tab`, `M-f`, `C-.`, `M-n`, `M-p`, `C-g`, `C-\`). An `ExtensionSource` fills that slot for each source the window hands in with the settings (`EditorSettings::inline`), so the text editor never looks up the registry for them.

### Servers run for an extension

- An extension declares a server in its manifest: its name, the program and the places to look for it (the `PATH`, the extension's own data directory, and directories named in the manifest), its arguments, its initialization options, and whether it reads every file or one language's.
- `Host::start_server(name)` starts it through the language-server hub; `Host::server_request(server, method, params)` and `Host::server_notify(server, method, params)` speak to it in JSON; its answers, notifications, and requests come back as a new event, `server-message`. A server's request the extension does not answer is answered with an error, as the hub does now.
- The text editor hands the extension's servers every open local file, and each change to it, before its source is asked, as it handed Copilot before. Files on another machine are never sent, whatever the extension asks. This stands in for document events: an extension needs no copy of the text, only the version and URI each request carries.
- Running a program is a grant: the package names each server it asks to run (`run "copilot-language-server"`), and a shipped extension is granted what its package asks. A server it was not granted is not started, and the notice says which grant is missing.
- Installing a server is a download the host makes on the user's command: the spec names a `.tar.gz` archive by URL per platform, and the file within it to keep, and the host unpacks it into `extension-servers/<id>/<name>` in the state directory, in a Compile tab so its progress shows. That directory is none the extension can write to, and no program it names in one it can write to is run: a sandboxed extension could otherwise put a program there and have Winter run it. As built it is the same `curl | tar` Copilot ran before, from the extension's URL; checking a checksum is left for later.

### A page and the clipboard

- `Host::open_url(url)` opens an `https` page in the browser, and `Host::copy(text)` puts text on the clipboard. Both are what the sign-in needs; both are in the notice log, so a user sees what an extension did.

### Settings of its own

- An extension declares its settings with `Registry::setting`: each one's name, kind (switch, number, or text, the kind its default is), default, and a line saying what it does. The settings page gives each a row under its extension's name, and `settings.kdl`'s checking reports a value of the wrong kind or a name none declares.
- Its block in `settings.kdl` is named for its id, so a user's `copilot { … }` reads as it does now. The extension is handed its settings at activation and again with each change, through a new `configured` event.

## The Copilot extension

`winter-copilot` holds what the two Copilot files hold now, rewritten against the SDK:

- The server: `copilot-language-server --stdio`, looked for on the `PATH` and in its data directory; the install archive for each platform; the editor and plugin it says Winter is. The servers VS Code downloads are not looked among: the built-in Copilot never did, though its settings comment said so.
- The suggestions: `textDocument/inlineCompletion` for ghost text and `textDocument/copilotInlineEdit` for next edits, with `didShowCompletion`, `didShowInlineEdit`, and the accept command told through receipts, and `textDocument/didFocus` sent on the `focused` event.
- The sign-in: `Copilot: Sign In`, `Copilot: Sign Out`, `Copilot: Status`, and `Copilot: Install Language Server`, as commands of the extension under the titles they have now, with the device code copied and GitHub's page opened through the host.
- Its settings, as now: `enable`, `completions`, `next-edit`, and `server-path`.

## Bundling

- Winter looks for extensions in two places: its bundled directory and the user's `extensions` directory. One the user installs with the same id takes the bundled one's place, which is how a user runs a newer Copilot before Winter ships it.
- The bundled directory is `<prefix>/lib/winter/extensions` on Linux and `extensions` beside `winter.exe` on Windows, each extension in a directory of its own, its package embedded in its component. `make dev` builds `winter-copilot` and copies it beside the debug binary, so a development build loads it too.
- The Makefile gains one target that builds every extension crate for `wasm32-wasip2`; `make build`, the Debian package, the AUR `PKGBUILD`, the Windows installer, and the release workflow call it and install what it built.
- A build without `wasm-extensions` has no Copilot. That is the one cost of bundling over compiling in, and the reason `wasm-extensions` stays a default feature.

## What goes from Winter

- `winter-text`: `inline/copilot.rs`, `CopilotConfig` and its `Default`, the Copilot port, and each document's Copilot version (the host's server sync keeps its own).
- `winter-term`: `window/editor/copilot.rs`, the `copilot` part of the config schema, reading, writing, and the settings page, its commands, and the test that the text editor's defaults match `settings.kdl` for Copilot.
- `builtin/settings.kdl` keeps its `copilot { … }` block, now documenting the extension's settings.

## Steps

1. The `extensions` layer: the folder, the layer test's rule, and `winter-ext-api-example` moved into it.
2. Settings of its own, then a page and the clipboard: the two small additions, each through a component in `winter-wasm`'s tests.
3. Servers run for an extension: the manifest's server, the `run` grant, the download, and `server-message`, tested against a fake server as the hub's tests are.
4. Inline suggestions: the SDK side, `ExtensionSource` in `winter-text`, and a test that a component's ghost text shows and `Tab` takes it.
5. `winter-copilot` built on all of it, with the bundled directory and the Makefile target. Both Copilots are kept for this one step, the built-in one switched off, so they can be compared.
6. The time from the last key to ghost text is measured on Xvfb for both. The extension may add no more than 5 ms; past that, the boundary is made cheaper before going on.
7. The built-in Copilot removed, the packaging updated, and the usage guide's Copilot section told where Copilot now comes from.

`make lint` and `make test` pass at the end of every step, and each step is a commit of its own.

## Not in this design

- An extension marketplace, or updating extensions from inside Winter: a user installs a newer one by hand, as now.
- Chat, or any assistant view beyond inline suggestions: a view built on the SDK's views when it is wanted.
- Signing or verifying the bundled component: it ships in the same package as the binary and is trusted as the binary is.
