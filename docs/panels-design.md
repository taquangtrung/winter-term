# Panels design

This document is the design for the window's panels: a left sidebar, a right sidebar, and a bottom panel, set around the panes where the work happens. It replaces the dock. [architecture.md](architecture.md) describes the code as it stands.

Status: steps 1 to 4 are built as of 2026-11-20: the model, the bottom panel in place of the dock, the sidebars (`C-S-h`, `C-S-l`), and workspaces. A sidebar is at least 20 cells wide, as the launcher needs that much. Step 5, the Agents page, waits on the agent design's status step.

Changed after building, at the user's direction: the sidebars hold a fixed set of tools, not any tab. Files (Dir at the project's root), Git, Outline (a new page showing the outline of the main tab last worked in), and Todo; Agents to come. Each is opened by `Sidebar: Files` and the rest, an empty sidebar opens the tool it last showed (Files on the left, Outline on the right at first), and a sidebar's new tab key offers the tools. Moving any other tab to a sidebar is refused, and where tabs open (below) concerns the bottom panel and the main area only. The bottom panel is as designed. The launcher no longer fills an empty sidebar.

## Problem

The dock is the root split of the pane tree with a `dock` flag on it (`winter-model/src/layout.rs`). That works for one panel along the bottom, and stops working past it:

- A sidebar has to stay put while the panes beside it split, balance, zoom, and close. Inside the tree it is one more split, and every operation on the tree has to know to leave it alone, as each one now does for the dock's split.
- The dock's height is a ratio of its split, so it changes with the window. A list of agents or files wants a width in cells that stays.
- Closing the dock's last tab removes the dock, so the next build has to make it again; a panel should be somewhere things go, there or not.
- A workspace switch parks the whole layout, dock included, so a page kept in view goes with the project.

Pages and editors are arriving that want a place of their own beside the work: the Agents page ([agent-workspace-design.md](agent-workspace-design.md)), Dir and Git kept in view while editing, an outline. A second and third flag on the tree would multiply the special cases; the panels need a model of their own.

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

- **The main area** is the pane tree as it is today, without the dock's split. Splitting, balancing, zooming one pane among the others, and closing other panes act here only. It always has at least one pane.
- **A panel** has a strip of tabs like a pane's, one shown, but sits outside the split tree and never splits.
- **The sidebars run the window's full height**, and the bottom panel spans the main area between them, so the lists in the sidebars keep their length when the bottom panel opens.

Any tab can sit in a panel: a terminal, an editor, or a page. What goes where is the user's choice, with defaults (below).

## How panels behave

### Showing and hiding

- Each panel has a key that shows it and focuses it, or hides it when it already has focus: `C-S-h` (`View: Toggle Left Sidebar`), `C-S-l` (`View: Toggle Right Sidebar`), and `C-S-j` (`View: Toggle Bottom Panel`), the dock's key. On macOS the bottom panel keeps `s-j`; the sidebars keep `C-S-h` and `C-S-l`, since `s-h` is the system's Hide.
- A hidden panel keeps its tabs, and their programs keep running, as the hidden dock's do.
- Closing a panel's last tab hides the panel; the panel stays, empty, for the next thing sent to it.
- Showing an empty panel fills it: the bottom panel with a shell where the work is, as the dock does; a sidebar with the split launcher, from which the user picks what goes there.
- Hiding a panel that has focus sends focus back to the main pane it came from.

### Size

- A sidebar's width and the bottom panel's height are kept in cells, not as a share of the window, so resizing the window leaves them alone. Dragging a panel's edge sets it, and so do the resize keys while a panel has focus.
- Being in cells, a panel grows with the font when the text is zoomed, on purpose: a shell in the bottom panel keeps its rows, and a list in a sidebar keeps the width of its words.
- The main area keeps a minimum size. When the window is too small for the panels at their sizes, they shrink toward their own minimums, the right sidebar first, then the left, then the bottom panel; past that, the right sidebar is left out, then the left.
- Whether a panel is shown is the user's choice, and is what is saved. Whether a shown panel fits is worked out each time the window is laid out, and never saved, so a panel left out of a small window comes back at its size when the window grows. A panel left out while it has focus sends focus back to the main pane it came from.

### Focus

- Focus is one place in the window: a panel tab with focus takes keys exactly as it would in a pane.
- `M-h/j/k/l` moves across the edge of the main area into a shown panel, and back out. A panel that is hidden or left out is skipped.
- Moving out of a panel goes to the main pane along that edge that last had focus, not to whichever pane happens to sit across from the panel's middle; only when none along the edge has had focus does the geometry choose.

### Zoom

Zooming the focused tab fills the window with it, wherever it sits: a pane, or a panel tab such as a shell in the bottom panel. The panels and the other panes come back as they were when the zoom ends.

### Keys inside a panel

The window's keys act on the panel where they make sense there, and on the main area where they do not:

| Key or command | With a panel focused |
| --- | --- |
| New tab, reopen closed tab | Adds the tab to the panel, so a second shell sits beside the first |
| Close tab | Closes it in the panel; the last one hides the panel |
| Split | Splits the main pane that last had focus, since a panel does not split |
| Close pane, go to pane N (`close_pane_N`, `C-M-N`) | Counts main panes only; panels are reached by their own keys |
| Next and previous tab | Moves within the panel's strip, stopping at the ends |

### Opening from a panel

A panel holds what serves the work, and the work goes to the main area. Opening a file from Dir in a sidebar, an agent from the Agents page, or a line from a saved list in the bottom panel shows it in the main pane that last had focus. Focus follows only when the action would move it in a pane today.

### The last main tab

The main area is never left without a pane. Closing its last tab, or moving it to a panel, leaves the split launcher in its place. The window ends only when the main area's last tab closes with every panel empty, as closing the last tab ends it today.

### Moving tabs

- `View: Move Tab to Left Sidebar`, `…to Right Sidebar`, `…to Bottom Panel`, and `…to Main Area` move the focused tab.
- Dragging a tab works as it does between panes today, with panels as targets: let go on a panel's strip or body and the tab joins the panel. A panel's edges are not split targets, since a panel does not split.

### Where tabs open

Each page, and each kind of editor other than the text editor (Dir, Git, a build, a saved list), remembers the panel it was last moved to, per window, and opens there next time; moving it back to the main area makes it open there again. That one rule covers the defaults below and every later kind of tab, with no setting per kind.

Terminals and text editors remember nothing: they open in the main area, or in the focused panel by the new tab key, and moving one into a panel moves that tab alone. Moving a terminal into the right sidebar does not send every later terminal there.

Where a tab opens before the user has moved its kind:

| Tab | Opens in |
| --- | --- |
| The shell from `C-S-j` | Bottom panel |
| A build | Bottom panel |
| Agents page | Left sidebar |
| Every other tab | Main area, as today |

The `compile { dock }` setting retires with the dock: a build opens where builds were last moved, the bottom panel until the user moves one.

## Workspaces

A workspace switch changes the project, so what belongs to the project goes with it, and what belongs to Winter stays:

- **Terminals and editors in a panel belong to the workspace.** A shell in the bottom panel was started in the project; Dir in the left sidebar shows its folder. They are parked with the main area, and the workspace switched to brings back its own.
- **Pages stay with the window.** A window has one of each page already, so the Agents page, Notifications, or System stay in their panel across the switch.
- **Each panel's shown state and size belong to the window**, so the frame of the window does not jump when the project changes.

So one strip can hold the window's pages and the workspace's tabs. Three rules keep it steady across a switch:

1. The window's pages sit at the start of the strip, in their own order, and the workspace's tabs follow.
2. Each workspace remembers which tab its panels showed, and shows it again when switched back to; a workspace that showed a page shows that page.
3. When the focused tab is parked by the switch, focus goes to the main area of the workspace switched to.

## Persistence

The session snapshot and the reload handover carry each panel's tabs, its shown tab, whether it is shown, and its size, beside the main area's tree. A workspace's saved layout carries its panel tabs and the tab each panel showed. The panel each kind of tab remembers is kept with the window's state.

A session or handover written before panels has the dock's split at its root; the reader takes that split's lower side as the bottom panel, shown, and the rest as the main area, so a reload across the change keeps the dock where it was. (Loading it as an ordinary split instead, tried first, left the dock stacked in the main area under a new bottom panel.)

## How it is built

- `winter-model`'s layout becomes a module of three files: `layout/tree.rs`, the pane tree as it is now without the dock's split; `layout/panels.rs`, the three panels, each a `Pane` with whether it is shown and its size in cells; and `layout/mod.rs`, where `Layout` holds both. `layout.rs` is over two thousand lines today, and the panels would otherwise grow it further.
- A `Side` (`Left`, `Right`, `Bottom`) names a panel, and a `Region` (`Main`, `Panel(Side)`) says where a tab is. Focus stays one `TabId` for the whole layout, beside the main tab that last had focus, kept so focus has somewhere to go back to, and the last focused main tab along each edge, for leaving a panel.
- `Layout::panes` works out which shown panels fit, lays them out, then lays the tree in what is left, so every caller drawing or hit-testing panes sees panels as panes with a region.
- Directional focus, zoom, closing, the last main tab, and moving tabs learn the panels in the model, where they are tested without a window. The dock's flag, `DockState`, `open_dock`, `hide_dock`, `show_dock`, and `dock_tab` go; the tree's operations no longer step around a special split.
- In the app, `toggle_dock`, `show_editor_in_dock`, `show_buried_in_dock`, and the send-to-terminal path's use of the dock become operations on the bottom panel. The command `toggle_dock` becomes `toggle_bottom_panel`, with every keymap updated, beside `toggle_left_sidebar` and `toggle_right_sidebar`.
- The tab strip's drop targets (`window/strip`) gain the panels, without their edge targets.
- Workspace parking moves the main area's tree, the panels' workspace tabs, and each panel's shown tab, and leaves the pages.

This replaces the dock in one pass; nothing keeps the old split working beside the panels.

## Steps

1. The model: the layout module split into tree and panels, the dock's split gone, fitting and the shrink order, focus across edges and back, zoom, closing, the last main tab, moving tabs, and tests for each.
2. The bottom panel in place of the dock: the toggle, the keys inside a panel, builds, sending to a terminal, the session and handover, and the `compile { dock }` setting retired.
3. The sidebars: their toggles, the move commands and dropping tabs on panels, resizing, and where tabs open.
4. Workspaces: parking the workspace's panel tabs and each panel's shown tab, and keeping the pages at the start of the strip.
5. The Agents page in the left sidebar, as the agent design's status step.

Each step ships on its own. The guide's [Basics](guide/basics.md) page gains the panels beside panes and tabs, and the Terminal page's dock section becomes a link to it.

## Open questions

1. **A narrow strip.** A sidebar's strip has little room for titles. Titles cut short with their full wording as a tooltip come first; a strip of icons can follow if that proves cramped.

## Not in this design

- Splitting a panel into panes.
- Floating panels, or panels torn off into windows of their own.
- A top panel.
- A bottom panel running the window's full width, under the sidebars.
