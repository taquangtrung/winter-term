# Panels design

This document is the design for the window's panels: a left sidebar, a right sidebar, and a bottom panel, set around the main area where the work happens. [architecture.md](architecture.md) describes the code as it stands.

Status: redesigned on 2026-11-21 and built through step 4 the same week. The first design (built 2026-11-20) made each panel a strip of tabs like a pane's; this one gives each panel views of its own, drawn by misty, and keeps every tab in the main area.

## Problem

The first design treated the three panels alike: each was one pane outside the split tree, holding any tab. Use showed that neither panel wants what a pane holds:

- The tools a sidebar showed (Dir, Git, Outline, Todo) are laid out for the main area: columns, diffs, menus, a footer. Squeezed to 30 cells they lose most of what they show, and the cell grid makes a narrow list harder to read than the UI font would.
- A sidebar then grew rules the shape could not hold: only its tools may go in, at most one of each, remembered per side. The app enforced them with guards on every path that moves a tab.
- The bottom panel held a shell most of the time. Winter is terminal-first: the main area always holds terminals, so a shell in a strip 14 rows high repeats it in less room. Orca, terminal-first too, has no bottom panel and keeps every terminal in its pane tree.
- The text editor in the bottom panel, or any document, has the same problem as a tool in a sidebar: too little room for what it is.

## The model

A window holds a **main area** and three **panels**:

```text
┌──────────┬──────────────────────┬──────────┐
│          │                      │          │
│   left   │      main area       │  right   │
│ sidebar  │   (the pane tree)    │ sidebar  │
│          │                      │          │
│          ├──────────────────────┤          │
│          │    bottom panel      │          │
└──────────┴──────────────────────┴──────────┘
```

- **The main area** is the pane tree. Every tab lives here: terminals, editors, and pages. It always has at least one pane.
- **A panel shows views, one at a time,** chosen from a switcher along its top. A view is drawn by misty in the UI font, never on the cell grid, and is built for the panel's shape. A view is not a tab: it has no strip, never closes, never moves to the main area, and nothing moves into a panel.
- **A sidebar shows sidebar views,** narrow and tall: Files, Git, Outline, and Todo.
- **The bottom panel shows bottom panel views,** wide and short: Problems, Output, and Search.
- **The sidebars run the window's full height**, and the bottom panel spans the main area between them.

A view belongs to a tool and shares its data, not its layout: the Files view reads the folder the way Dir does, the Output view shows a build Compile runs. Each view has `Open in Main Area`, which opens its tool there at the same place.

| View | Panel | Shows | Its tool |
| --- | --- | --- | --- |
| Files | Sidebar | The project's folders as a tree, with git's marks | [Dir](guide/dir.md) |
| Git | Sidebar | The branch; staged, unstaged, and untracked files; stage and unstage | [Git](guide/git.md) |
| Outline | Sidebar | The headings and symbols of the main tab last worked in | (none) |
| Todo | Sidebar | The project's TODO comments, by file | [Todo](guide/todo.md) |
| Problems | Bottom | Errors and warnings from the builds, one row each, by file | [Compile](guide/compile.md) |
| Output | Bottom | The builds run, newest first, one shown, following its end | [Compile](guide/compile.md) |
| Search | Bottom | Saved lists and project searches, one shown, a row per match | [Pickers](guide/pickers.md) |

A view in a sidebar sits in one sidebar at a time. `Sidebar: Files` and the rest show it where it is, else in the sidebar it starts in (Outline on the right, the rest on the left); the switcher's menu moves it to the other side.

## How panels behave

### Showing and hiding

- Each panel has a key that shows it and focuses it, or hides it when it already has focus: `C-S-h` (`View: Toggle Left Sidebar`), `C-S-l` (`View: Toggle Right Sidebar`), and `C-S-j` (`View: Toggle Bottom Panel`). On macOS the bottom panel keeps `s-j`.
- A panel shows the view it last showed: at first Files on the left, Outline on the right, and Output at the bottom.
- Hiding a panel that has focus sends focus back to the main pane it came from. A hidden view keeps its state.

### Size

- A sidebar's width and the bottom panel's height are kept in logical pixels, so resizing the window leaves them alone, and they follow the UI scale as the rest of the chrome does, not the terminal font. Dragging a panel's edge sets the size, and so do the resize keys while a panel has focus.
- The main area keeps a minimum size. When the window is too small for the panels at their sizes, they shrink toward their own minimums, the right sidebar first, then the left, then the bottom panel; past that, the right sidebar is left out, then the left.
- Whether a panel is shown is the user's choice, and is what is saved. Whether a shown panel fits is worked out each time the window is laid out, and never saved.

### Focus

- Focus is one place in the window: a pane's tab or a panel's view. A view with focus takes keys in Normal mode: `j`/`k` move, `RET` opens, `l`/`h` open and close a folder or group.
- `M-h/j/k/l` moves across the edge of the main area into a shown panel, and back out to the main pane along that edge that last had focus.
- Zoom fills the window with the focused tab; a panel's view does not zoom.

### Keys with a panel focused

| Key or command | With a panel focused |
| --- | --- |
| New tab, split, reopen closed tab | Acts on the main pane that last had focus |
| Close tab | Hides the panel |
| Next and previous tab | Moves between the panel's views, stopping at the ends |
| Close pane, go to pane N | Counts main panes only |

### Opening from a panel

What a view opens goes to the main area, in the main pane that last had focus: a file from Files, a problem's line, a match from Search, a build from Output by `Open in Main Area`. Focus follows.

### Builds

A build runs as a Compile buffer, as before, but opens no tab: the Output view shows it, newest first, and shows the bottom panel when a build starts from a key or a command. When it finishes with errors, Problems shows them, replacing that build's earlier ones. `Open in Main Area` shows the build in a Compile tab, the same buffer. `F8` walks the newest build's places as it does now.

Sending text to a terminal goes to the main-area terminal last worked in.

## Workspaces

Each view keeps its state per workspace (the folders Files has open, the results Search holds), and a workspace switch brings back that workspace's. Which view each panel shows, whether it is shown, and its size belong to the window, so the frame does not jump when the project changes. Nothing in a panel is parked, as a panel holds no tabs.

## Persistence

The session snapshot and the reload handover carry, for each panel, the view it shows, whether it is shown, its size, and for a sidebar which views it holds. A view's light state goes with it (the folders Files has open); a build's output does not.

A session or handover written before this design is read as follows: a panel's terminals and other tabs go to the main area, beside the tab that had focus, so nothing running is lost; a sidebar's tool tabs become its views; sizes kept in cells are dropped for the defaults. One written before panels has the dock's split at its root, whose lower side's tabs also go to the main area.

## How it is built

- `winter-model`'s `layout/panels.rs` holds a `Dock<K>`: the views it holds, keyed by kind, the one shown, whether it is shown, and its size in logical pixels. `Sidebar` is a `Dock<SidebarKind>`, one per `Edge` (`Left`, `Right`); `BottomPanel` a `Dock<BottomPanelKind>`. The kinds are plain enums in the model. A free function fits the shown panels around the main area.
- A view is identified, as a tab is, by a `TabId` the app allocates, so focus stays one `TabId` and a view's keys, jobs, and outcomes go the way a page's do. Its object is a page drawn as a misty view (`Page::is_view`), held in the app's open tabs but in no pane. `Panel` (`Sidebar(Edge)`, `Bottom`) names a panel, and `Region` (`Main`, `Panel(Panel)`) says where an id is.
- `Layout::panes` lays out the docks beside the tree, so drawing and hit-testing see each dock as a strip with a region; the strip of a dock is drawn as a switcher (misty's tab strip without close buttons, dragging, or the new-tab button).
- The app's `window/layout/panels.rs` becomes `window/panels/`: `mod.rs` for toggling, the switcher's picker, feeding the views, and sessions; `views/mod.rs` for the list every view is built on; one module per view beside it. Files and Git read `winter-dir`'s and `winter-vcs`'s data; Outline reads the main tab's `outline_targets`; Todo uses the window's search job; Problems and Output read the Compile buffers; Search reads saved lists.
- Moving tabs to a panel, the drop targets on panels, where each kind of tab opens (`placement`), and the sidebar tool guards go.

This replaces the first panels design in one pass.

## Steps

1. The model: `Dock<K>`, the kinds, sizes in pixels, fitting, focus across edges, and their tests.
2. The app on the model: views as page slots in no pane, the switcher, toggles, tabs only in the main area, sessions read from both older formats.
3. The sidebar views: Outline, Files, Git, Todo.
4. The bottom panel views: Output and Problems with builds; Search with saved lists.
5. Workspaces, the guide's [Basics](guide/basics.md) page, and each tool's page.

## Open questions

1. **Language servers in Problems.** Problems starts with builds; a language server's diagnostics for open files could join it, with a filter by source.
2. **Editing in Files.** Files opens; new file, rename, and delete stay in Dir until they are missed.
3. **A quick command.** A prompt that runs a command into the Output view, for the quick tasks a bottom shell served.

## Not in this design

- A terminal in the bottom panel; one could come back as a Terminal view if missed.
- Splitting a panel, floating panels, or a top panel.
- Extensions adding views.
