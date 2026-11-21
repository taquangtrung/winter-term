# Basics

## Windows, panes, and tabs

A window is split into panes, and each pane holds a strip of tabs. A tab is one of three things:

- A **terminal** runs a shell or a program.
- A **page** shows Winter's own state: Settings, Keyboard Shortcuts, Notifications, Extensions, Processes, System, Docker, and extensions' views. A window has one of each page; opening it again goes to it.
- An **editor** shows a file, a directory, a repository, or a build.

An editor showing a file or a folder runs its path along the top as breadcrumbs; click a folder to list it in [Dir](dir.md).

Splitting a pane (`M-S--` or `M-S-\`) opens a launcher in the new pane: `RET` or `t` for a terminal, `f` for a file, `d` for a directory, `g` for git, or click a choice.

All window keys work from any tab: split, zoom, close, and `M-h/j/k/l` to move between panes. See [Keys](keys.md#window-pane-and-tab-keys).

## Panels

Around the panes sit three panels: a left sidebar, a right sidebar, and a bottom panel. Each holds a strip of tabs, like a pane but without their numbers, and never splits, and stays put while the panes beside it split, zoom, and close.

| Key | Panel |
|---|---|
| `C-S-h` | Left sidebar |
| `C-S-l` | Right sidebar |
| `C-S-j` | Bottom panel |

- A panel's key shows it and moves the keyboard there; pressed again in the panel, it hides it. Hidden, its tabs keep running.
- `M-h/j/k/l` moves into a panel and back out, to the pane you came from.
- Drag a panel's edge to resize it. When the window is too small, the right sidebar gives way first, then the left, then the bottom panel; they come back as the window grows.
- Closing the last tab among the panes leaves a launcher there while a panel still holds tabs.
- A [workspace](workspaces.md) switch changes the terminals and files in the panels; pages stay where they are.

### Sidebars

A sidebar holds tools, one shown at a time:

| Command | Tool |
|---|---|
| `Sidebar: Files` | The project's folder as a tree ([Dir](dir.md)) |
| `Sidebar: Git` | The project's [git](git.md) status |
| `Sidebar: Outline` | The headings and symbols of the file you work on; `RET` or a click goes there |
| `Sidebar: Todo` | The project's TODO comments ([Todo](todo.md)) |

- An empty sidebar opens with the tool it last showed: Files on the left and Outline on the right at first.
- The new tab key or button in a sidebar offers the tools. Other tabs, terminals and files, stay among the panes.
- A file opened from a tool, from Files say, opens in the pane you last worked in.
- `View: Move Tab to Left Sidebar` and `…to Right Sidebar` move a tool from one sidebar to the other.

### The bottom panel

The bottom panel holds any tab. Empty, it opens with a shell where you were working; builds open there too.

- `View: Move Tab to Bottom Panel` and `…to Main Area` move a tab; dragging a tab onto the panel does too. A page or a tool's tab, such as a build, then opens where it was moved next time.

## Buffers

An editor's contents are a buffer, kept apart from the tab showing it.

- Closing an editor's tab buries its buffer instead of dropping it. A build keeps running and a file keeps its edits.
- `Buffer: Switch to…` lists every buffer, buried ones too; `Enter` brings one back.
- `C-M-S-q` (`Buffer: Kill`) lets a buffer go, asking first about unsaved edits or running work.
- A file is one buffer however many tabs show it: an edit in one tab shows in the others, and each tab keeps its own cursor.
- Opening a file that a tab in the pane already shows goes to that tab.

Closing a terminal's tab while a program runs in it (an agent, a build, a server) buries the terminal: the program keeps running, and `Buffer: Switch to…` or `C-S-t` brings it back. A shell at its prompt just closes. `Buffer: Kill` ends a terminal, buried or shown, asking first when a program is running. Quitting or reloading Winter ends buried programs too, so it asks first. To keep a shell alive after Winter quits, run it in a [mux session](terminal.md#multiplexer).

## The status bar

The status bar runs along the bottom of the window:

- At its left: where the focused terminal's commands run (this machine, an SSH host, or a container), and the mode.
- In the middle: what the focused tab reports, and short messages such as `pushed`. Long work running, such as a `git push`, follows it: what runs, how far it has come, how many more tasks run, then what the program last said (`git push origin main 50% · Writing objects: 50% (1/2)`). Click it, or run `Tasks: Show Running`, to see every task as a card.
- At its right: where the cursor is (`12:5  340 lines`, `page 3 / 18`), AI quotas, and the notification bell.

`status-bar { show #false }` hides it, `status-bar { show-mode #false }` hides the mode, and `status-bar { progress-details #false }` leaves out what a running program last said; rest the pointer on the progress to read it.

The AI quotas show Claude Code, Antigravity, and Z.ai usage, each read the way its own tools read it. Z.ai needs a key: `Quota: Set Z.ai API Key` stores it in the system's secret store. `Quota: Refresh` refreshes them all. A provider that cannot be reached is hidden.

## Notifications

- A confirmation shows in the status bar for three seconds.
- A warning or error shows as a card at the window's top right: six seconds for a warning, eight for an error. Hover to keep it; click it to open the history.
- `notifications { toasts #false }` shows warnings and errors in the status bar instead.

Unread warnings and errors light the bell. Click it, or run `Notifications: Open`, to open the Notifications tab:

| Key | Action |
|---|---|
| `Enter` | Unfold or fold a long message |
| `Tab` / `S-Tab` | Filter to errors, warnings, or the rest |
| `y` | Copy the message |
| `/` | Find a message |
| `C` | Clear all |

`Notifications: Clear` forgets all notices from anywhere.

## Common keys in every editor

- `gg` and `G` (or `M-S-,` and `M-S-.`) go to the top and bottom; `H` `M` `L` and `C-d` `C-u` `C-f` `C-b` work as in Vim.
- `C-r` reads or runs again what the editor shows.
- `/` (or `?`) searches with a find box at the pane's top right; `n` and `N` step through matches. A lower-case query matches any case; a capital makes it exact.
- `Help: Keys of This Tab` lists the keys of the tab in front.
- A right click opens the editor's own menu, each entry showing its key.

In a listing (Dir, Git, a saved list, the monitors, Notifications), `h` `l` `w` `b` `e` `0` `$` and `f` `t` move a column cursor along the row, and `v` or `V` selects from it, so you can copy part of a listing.

## Questions

An editor asks in a panel in the middle of the window. A question with known answers shows a filtered list: type to narrow it, `Enter` to take one, `Esc` to cancel. In any answer field, `Up` and `Down` bring back earlier answers.

## Closing

Closing a tab, a pane, or the window asks first when a terminal has a program running or an editor has unsaved edits. Press `y` to close; any other key cancels.
