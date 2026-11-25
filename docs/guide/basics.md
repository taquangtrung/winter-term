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

Around the panes sit three panels: a left sidebar, a right sidebar, and a bottom panel. A panel shows views, one at a time, chosen from the switcher along its top. A view is a list built for its panel; it is no tab, and tabs stay among the panes.

| Key | Panel |
|---|---|
| `C-S-h` | Left sidebar |
| `C-S-l` | Right sidebar |
| `C-S-j` | Bottom panel |

- A panel's key shows it and moves the keyboard there; pressed again while the panel shows, it hides it, wherever the keyboard is. It shows the view it last showed.
- `M-h/j/k/l` moves into a panel and back out, to the pane you came from.
- In a view, `j`/`k` move, `RET` opens, `l`/`h` open and close a folder or group, `r` looks again, and `t` opens the view's tool among the panes.
- What a view opens, a file or a line, opens in the pane you last worked in.
- Every view is ready in its panel from the start, listed on the switcher along its top: click one to show it. A sidebar's switcher shows each view as an icon; point at it for its name and key. Next and previous tab move between them; closing one hides the panel.
- Drag a panel's edge to resize it. When the window is too small, the right sidebar gives way first, then the left, then the bottom panel; they come back as the window grows.
- A [workspace](workspaces.md) switch points the views at the new project; the panels stay as they are. Each workspace keeps the folders Files has open and its latest search, through a restart too.

### Sidebars

| Command | View |
|---|---|
| `Sidebar: Files` | The project's folders as a tree, each changed file and its folders marked with git's state; what git ignores is hidden until `.` shows it. See [Dir](dir.md) for the full listing |
| `Sidebar: Git` | The branch and the changed files; `s` stages and `u` unstages the file under the cursor ([Git](git.md)) |
| `Sidebar: Outline` | The headings and symbols of the file you work on, nested as they are, each line number at the right; `l` and `h` open and close one holding others |
| `Sidebar: Todo` | The project's TODO comments, by file ([Todo](todo.md)) |
| `Sidebar: Agents` | The project's agents and how each stands ([Agents](agents.md)) |
| `Sidebar: Terminals` | Every terminal open in the window, in every workspace, by what it runs or last ran; a click shows it |

- A view sits in one sidebar at a time: Outline starts on the right, the rest on the left. `View: Move View to the Other Sidebar` moves the focused one across.
- An empty sidebar opens with Files on the left and Outline on the right.

### The bottom panel

| Command | View |
|---|---|
| `Bottom Panel: Output` | The builds run, newest first, and the end of the one under the cursor; `l` reads the whole of it there, `h` goes back, and `]` and `[` read the next and previous build ([Compile](compile.md)) |
| `Bottom Panel: Problems` | The errors and warnings the builds, the language servers, and extensions found in the project, by file; the row along its top narrows them to one of those, and `]` and `[` step through it |
| `Bottom Panel: Search` | The latest project search or picker rows saved; a new one replaces it, and `t` keeps it as a saved list |

- Empty, the bottom panel opens with Output.
- A shell has no place here: open one among the panes.

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
- At its right: where the cursor is (`12:5  340 lines`, `page 3 / 18`), what extensions show (such as AI quotas), and the notification bell.

`status-bar { show #false }` hides it, `status-bar { show-mode #false }` hides the mode, and `status-bar { progress-details #false }` leaves out what a running program last said; rest the pointer on the progress to read it.

The AI quotas come from extensions Winter ships: Claude Code, Antigravity, and Z.ai usage, each read the way its own tools read it. Click one to refresh it. Z.ai needs a key: `Z.ai: Set API Key` stores it in the system's secret store. A provider that cannot be reached is hidden. In `status-bar { segments … }` they are `claude.quota`, `antigravity.quota`, and `zai.quota`.

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
