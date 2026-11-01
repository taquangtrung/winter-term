# Crate structure design

This document is the agreed design for regrouping Winter's crates into layers: a small app on top, tools beside one another, and the crates they stand on beneath. It describes a plan, decided on 2026-11-04, not the code as it stands; [architecture.md](architecture.md) describes the code.

## Problem

`winter-term` holds about 134,000 lines: the window, and most of the tools shown in it. Every edit to any tool rebuilds all of it, and nothing stops one tool reaching into another or into the window. The crates beside it sit in one flat `crates/` folder, so the tree does not show which crates are the base, which are tools, and which are the kit extension authors build with.

## Approach

Winter is split into layers, and each crate depends only on the layers below its own. A tool is a crate of its own, built on the `Editor` trait in `winter-model`, as `winter-git` and `winter-pdf` already are; it asks the window for anything through that trait's outcomes, never by reaching into `winter-term`. The crates are grouped into folders by layer.

```text
winter-term          the window: panes, tabs, terminal, chrome, config
     │
     │ depends on
     ▼
tool crates          winter-dir, winter-git, winter-text, winter-pdf,
                     winter-monitor, winter-docker
     │
     │ depends on
     ▼
base crates          winter-model (the Editor trait), winter-render,
                     winter-base, winter-picker, winter-syntax, …
```

`winter-term` may also depend on a base crate directly. No arrow points up: a tool never depends on `winter-term` or on another tool, and a base crate never depends on a tool. The `sdk` crates sit beside the layers rather than in them: any crate may depend on them (`winter-model` and `winter-wasm` use `winter-ext-api`), and they depend on nothing but one another, so the kit an extension author builds with carries nothing of Winter's own.

## Layout

```text
crates/
  app/    winter-term
  tools/  winter-dir, winter-git, winter-pdf, winter-text,
          winter-monitor, winter-docker
  base/   winter-base, winter-core, winter-lsp, winter-model,
          winter-mux, winter-picker, winter-proto, winter-render,
          winter-spell, winter-syntax, winter-wasm
  sdk/    winter-ext-api, winter-ext-guest, winter-ext-api-example
```

- `sdk/` holds what an extension author adds to their own `Cargo.toml`: the API, the macro that builds a component, and the template. `winter-wasm`, which loads and sandboxes components, is Winter's own side and sits in `base/`.
- Crate names do not change with their folders: `use winter_model::…` and every dependency key stay as they are. The workspace's `members` and `[workspace.dependencies]` take the new paths.

## Renames

- `winter-extension` becomes `winter-ext-api`, and `winter-ext-example` becomes `winter-ext-api-example`. The WIT package keeps its name, `winter:extension`, and "extension" stays the word for the concept.
- `winter-highlight` becomes `winter-syntax`: Winter highlights search matches, occurrences, picker matches, and Scout's matches too, and this crate colors only syntax. Its doc says what it reads: TextMate's scopes, in Sublime's `.sublime-syntax` files.
- `winter-model`'s description stops naming the palette, which lives in `winter-picker`.

## Tools moved out

| Crate | From `crates/winter-term/src/editors/` | Lines | What it needs first |
|---|---|---|---|
| `winter-dir` | `dir/` | 9,500 | Nothing: it uses only `winter-model`, `winter-base`, `winter-git`, and `winter-render`. |
| `winter-monitor` | `proc/`, `sys/`, with `run.rs`, `units.rs`, `gpu.rs` | 6,200 | `refresh.rs` moved down to `winter-model`. |
| `winter-docker` | `docker/` | 1,600 | `refresh.rs` moved down to `winter-model`. |
| `winter-text` | `text/`, with `diff/` | 29,000 | Completion handed in, not looked up (see below). |

- `run.rs`, `units.rs`, and `gpu.rs` serve only the two monitors, so they go with them. `refresh.rs`, which times a self-refreshing editor's next snapshot, serves the monitors and Docker, so it moves down into `winter-model`'s editor module. All four use the standard library alone.
- The text editor looks up completion back-ends in the extension registry itself (`crate::extensions::registry`). `winter-term` hands them in instead, as a list of completers given to the editor, so the text editor knows nothing of extensions.
- The diff view is built on the text editor's pieces and moves with it.

What stays in `winter-term`: what is tied to the window (the launcher, notifications, the Keys editor, which lists the window's commands), and the small editors not worth a crate yet (Grep, Compile, Preview, Image, Hex). Any of these moves out later the same way, when it is large enough or self-contained enough to be worth it.

## Keeping the layers

Folders do not enforce layers; dependencies do. A test (`crates/app/winter-term/tests/layers.rs`) reads `cargo metadata` and fails when a crate depends on one in a higher layer, a tool on another tool, or an `sdk` crate on anything outside `sdk` (none is allowed: what two tools share moves down into `base`, as git's command lines did into `winter-vcs` and SyncTeX into `winter-synctex`). A crate's layer is the folder it sits in, and a folder under `crates/` that is not a layer fails it too.

## Outside this repository

`magic-winter` depends on `winter-ext-api` (now `winter-extension`), `winter-ext-guest`, and `winter-wasm` by relative path, and `winter-term` depends on it by path. Its paths change once, to the final layout, after the last move here.

## Order of work

On a branch, `crates`, each step building and passing the workspace's tests before the next:

1. The renames: `winter-ext-api`, `winter-ext-api-example`, `winter-syntax`, and `winter-model`'s description.
2. The layer folders, every crate moved into its own, and the layer test.
3. `winter-dir`.
4. `refresh.rs` into `winter-model`, then `winter-monitor` and `winter-docker`.
5. Completion handed in to the text editor, then `winter-text` with the diff view.
6. `magic-winter`'s paths, docs (`architecture.md`, `RELEASING.md`, the crate table in `crates/winter-term/README.md`), and CI.

## Non-goals

- A single `winter-tools` crate: it would hold no boundaries between tools, and an edit to any tool would rebuild them all.
- Splitting the window itself (panes, tabs, the terminal) out of `winter-term`.
- Changing any tool's behaviour while it moves.
