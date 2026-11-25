# Settings

Winter's settings live in two files in `~/.config/winter-term/` (`%APPDATA%\winter-term` on Windows):

- `settings.kdl` for appearance and behavior;
- `keybindings.kdl` for keys (see [Keys](keys.md#rebinding-keys)).

Both are [KDL](https://kdl.dev/). Anything you leave out keeps its default. Changes apply when you save the file, with no restart.

## The Settings page

`C-,` (or `Preferences: Settings`) opens every setting as a page, which writes the same file. Each change applies at once.

| Key | Action |
|---|---|
| `j` / `k` | Next, previous control |
| `h` / `l` | Change the setting |
| `SPC` | Turn a switch on or off |
| `i` or `RET` | Type into the control, until `Esc` |
| `/` | Find a setting in any section by its name or what it does |
| `Tab` | Move between the sections and the settings |
| `[` / `]` | Choose where changes go: your settings or the [workspace](workspaces.md#settings) |

The page is in Normal mode until `i`, `RET`, or `/`, so letters never type into a setting by accident. Each setting shows its description, or its note on the workspace, under its name. The page also edits each platform's shell, the status bar's segments, completion back-ends, language servers, and colors over the theme.

## Common settings

| Setting | Meaning |
|---|---|
| `font` / `font-size` | Font family and size |
| `font-weight` / `font-weight-bold` | Weights for normal and bold text |
| `ligatures` | Draw `->` and `=>` as arrows |
| `theme` | `"dark"`, `"light"`, `"auto"`, or a theme file's name |
| `opacity` | Window opacity, `0.1` to `1.0` |
| `menu-style` | `"modern"` (a menu button) or `"classic"` (a menu bar) |
| `title-bar-style` | The system's title bar or Winter's |
| `gutter-side` | `"left"` or `"right"`: where an editor's gutter goes |
| `terminal-gutter` | A column of exit statuses beside terminals |
| `pane-border-width` | The line between panes |
| `scrollbar` | `"auto"` shows scrollbars while you scroll or point at them, then fades them out; `"always"` or `"never"` |
| `dim-inactive` | Dim panes without focus |
| `restore-session` | Reopen the last layout on launch |
| `scroll-margin` | Rows kept above and below the cursor (5) |
| `screen-row-motions` | `j` and `k` step through a wrapped line's screen rows in every editor, as Vim's `gj` and `gk` do (on) |
| `scrollback-lines` | How much scrollback to keep |
| `shell`, `shell-linux`, `shell-macos`, `shell-windows` | The shell to run |
| `window-title-template` | The window title, such as `"Winter Term - {{ name }}"` |
| `rainbow-parens` | Color brackets by depth |
| `editor-indent-guides` | Faint lines at each indent level |
| `editor-wrap-words` / `editor-wrap-indent` | How the text editor wraps lines |
| `editor-format-on-save` | Format files on save with the language server |
| `prompt-edit-bindings` | `"emacs"` or `"none"` (see [Modes](modes.md)) |
| `cursor { … }` | Blinking and the cursor's shape in each mode |
| `status-bar { … }` | `show`, `show-mode`, `progress-details`, and which `segments` show |
| `clipboard-read` | Let programs read the clipboard |
| `option-as-alt` | On macOS, which Option key is Alt |

## Themes

A theme is a KDL file under `themes/`, with an optional `base "dark"` or `base "light"` and a `colors` block over it. A `colors` block in `settings.kdl` works too. Recolor syntax by TextMate scope, such as `syntax { entity.name.function "#bf73df" }`; `Developer: Inspect Tokens and Scopes` shows the scopes under the cursor.

## Updating without losing your session

After installing a new Winter, run `winter --reload` (or `reload` in the command picker). On Linux and other Unix systems the window restarts in place: programs in terminals keep running, and the layout, screens, and scrollback come back. On Windows, reload starts fresh shells.
