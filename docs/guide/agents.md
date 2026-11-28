# Agents

Winter runs AI coding agents (Claude Code, Codex, Gemini, or any other command-line agent) in its own terminals. An agent started with `Agent: New…` gets a branch and a checkout of its own, so several can work at once without touching each other's files, and it keeps running when you close its tab or quit Winter.

| Command | Action |
|---|---|
| `Agent: New…` | Start an agent in the repository you are in |
| `Agent: List` | Every agent of the repository, running or stopped; `Enter` shows one |
| `Agent: Review` | Show the agent's work against the branch it started from |
| `Agent: Merge` | Put the agent's work on the branch it started from, then remove the agent |
| `Agent: Discard` | Remove the agent and its work |
| `Agent: Stop` | End the agent, keeping its folder to open again |
| `Agent: Stop All` | End every agent, keeping their folders |
| `Agent: Manage` | Move an agent you started by hand onto a branch of its own |
| `Sidebar: Sessions` | Keep the project's agents and open terminals in view, with how each stands |

`Review`, `Merge`, `Discard`, and `Stop` act on the agent in the tab you are in, or on its row in `Agent: List`.

## Starting an agent

`Agent: New…` asks which agent to run, then:

- Makes a branch for it from the branch you are on, named after the agent and a number: `agent/claude-1`.
- Checks that branch out in a folder beside the repository: `myapp.worktrees/agent/claude-1` next to `myapp`.
- Opens a terminal there and starts the agent in it.

The agent works only in its own folder, so your checkout stays as it was. Its work reaches your branch only when you merge it, with git as usual.

## Moving an agent you started by hand

An agent you started yourself, by typing `claude` in a terminal, runs in your checkout. `Agent: Manage`, in its tab, gives it a branch and a folder of its own, as `Agent: New…` would:

- It makes the branch and folder first, so if that fails, the agent keeps running where it was.
- It ends the agent, which leaves your terminal at its shell, and starts it again in the new folder with its resume line.
- Edits you have not committed stay in your checkout.
- An agent at work is left alone: move it once it waits for you.

Claude Code keeps its conversations by folder, so after the move it starts a fresh conversation.

## Coming back to an agent

The agent runs in a [mux session](terminal.md), named after its branch (`agent-claude-1`). Closing its tab buries it, and it goes on working; quitting Winter leaves it running too.

`Agent: List` shows every agent of the repository with `running` or `stopped` beside it. `Enter` on a running one shows its terminal. On a stopped one, it opens a terminal in the agent's folder again and starts the agent with its resume line, so its last conversation comes back.

After a restart of your computer, the mux server starts the agents again by itself, each with its resume line, as soon as it starts. A task cut off mid-way does not carry on by itself.

If no mux server is running, Winter starts one. If the one running is from another version of Winter, after an upgrade say, Winter cannot use it and asks whether to restart it. Restarting ends every mux session, an agent at work included, then opens the agents of your repository again with their resume lines.

## Seeing how an agent stands

An agent's tab says how it stands:

| Tab | Meaning |
|---|---|
| A spinner | Working on a request |
| A dot in the accent color | Started waiting for you out of sight, and you have not looked since: a question, a permission, or a finished turn |

An agent starts waiting out of sight when its tab is behind another tab, buried, or Winter is in the background. The dot goes once the tab is in front with Winter focused, so an idle agent you have seen shows nothing.

Without hooks, Winter tells from the agent's terminal: a spinner in its title or on its screen, as Claude Code, Antigravity, and pi draw while they work, means working, and anything else waiting. An agent that shows neither reads as waiting all the time.

The agent says how it stands best itself, through its hooks, which run `winter agent report working`, `waiting`, or `stopped`. Add them once, for every agent you run in Winter's terminals. For Claude Code, in `~/.claude/settings.json`:

```json
{
  "hooks": {
    "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": "winter agent report working" }] }],
    "Notification": [{ "hooks": [{ "type": "command", "command": "winter agent report waiting" }] }],
    "Stop": [{ "hooks": [{ "type": "command", "command": "winter agent report waiting" }] }],
    "SessionEnd": [{ "hooks": [{ "type": "command", "command": "winter agent report stopped" }] }]
  }
}
```

For Codex, in `~/.codex/config.toml`: `notify = ["winter", "agent", "report", "waiting"]`.

This works for an agent in any of Winter's terminals, one `Agent: New…` started included: its report goes to the mux server, which keeps it, so a window that opens the agent later still shows how it stands. Outside Winter the command does nothing, so the hooks can stay. Once an agent has reported, a bell or a desktop notification it prints also marks it waiting.

## Landing an agent's work

- `Agent: Review` opens what the agent changed since its branch left yours, what it has not committed included.
- `Agent: Merge` first replays the agent's commits on top of your branch, in the agent's own folder, so a conflict stops there, for you or the agent to sort out, and never in your checkout. Then it moves your branch to them and removes the agent's folder, branch, and session. It asks first if your checkout has changes of its own, and refuses while the agent has changes it has not committed.
- `Agent: Discard` removes the agent's folder, branch, and session, after asking when they hold work on no other branch.

Both ask first if the agent is still working.

## The Sessions sidebar

`Sidebar: Sessions` groups sessions into two sections: **Agents** (managed agents and agents you started by hand in a terminal) and **Terminals** (ordinary open terminals, including those in parked workspaces), each with an icon for how it stands: working, waiting for you, running, or stopped (pointing at the icon says which). An agent is found even when it runs under other programs, as `make dev` runs one under `make` and `cargo`, on Linux, macOS, and Windows; a running terminal's spinner means only that a program runs in it. Under its name is the folder it works in, shortened from the left a folder at a time when the sidebar is narrow (`~/W/apps/winter-term`, then `~/W/a/winter-term`); pointing at the row shows it in full.

Each managed agent opens on a few lines about it: what it is doing, as its terminal's title says, and the branch it started from. Click the arrow beside an agent, or press `TAB` on any of its lines, to fold it shut or open it again; `h` and `l` fold it and open it. Click a running agent or terminal to show it; a stopped agent takes a double click, since that starts it again.

| Key | Action |
|---|---|
| `Enter` | Show the agent's terminal, or open a stopped one again |
| `TAB` | Fold the agent shut, or open it again |
| `h` / `l` | Fold the agent shut, or open |
| `r` | Review its work |
| `m` | Merge it |
| `x` | Discard it |
| `s` | Stop it |

## When Winter quits

Agents keep running when Winter quits, and a notification says how many. Set `stop-on-quit #true` in the `agents` block to end them instead. A session left after its folder was removed outside Winter shows in `Agent: List`, where `Enter` ends it.

## Which agents are offered

The `agents` block of `settings.kdl` lists them. Each entry gives the line typed to start the agent and, if it has one, the line that opens its last conversation again:

```kdl
agents {
    agent "claude" {
        command "claude"
        resume "claude --continue"
    }
    agent "aider" {
        command "aider --no-auto-commits"
    }
}
```

Claude Code, Codex, Gemini, Antigravity, GitHub Copilot, Kiro, opencode, pi, and Summer are built in; an entry with the same name changes one, and a new name adds an agent. `worktrees "../{repo}.worktrees"` in the same block says where the agents' folders go, relative to the repository, `{repo}` standing for its folder's name.

A new folder holds only what git tracks: files git ignores, such as `.env`, installed packages, or build output, are not in it, so the first build there starts from nothing.
