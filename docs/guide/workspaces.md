# Workspaces

A workspace is a project the window can switch to and back from, coming back as you left it: its panes, its tabs, and each terminal's directory. Switching away keeps its programs running out of sight.

| Command | What it does |
|---|---|
| `Workspace: Mark Folder as Workspace…` | Make the project in front a workspace |
| `Workspace: Switch…` | Go to a workspace |
| `Workspace: Open Recent…` | Go to a recent workspace (bind `workspace_recent` for a key) |
| `Workspace: Add Folder` | Add the project in front to this workspace |
| `Workspace: Rename…` | Rename this workspace |
| `Workspace: Remove…` | Forget a workspace; its folders are left alone |

Leaving a layout that is in no workspace closes it, asking first about running programs and unsaved edits. Quitting saves every open workspace.

The same commands work from a shell:

```bash
winter workspace add [dir] [--name N]
winter workspace add-folder [dir]
winter workspace switch <name>
winter workspace rename <name>
winter workspace remove <name>
winter workspace list
```

Workspaces are kept in `~/.local/state/winter-term/workspaces/`, never in the project's folder.

## Settings

A workspace has its own settings, layered over `settings.kdl`. On the Settings page, `[` and `]` choose where changes go: `User` (your `settings.kdl`) or the workspace. A setting the workspace overrides says so; in the workspace's scope its **Reset** button drops the workspace's value.
