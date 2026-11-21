# Agent workspace design

This document is the design for running AI coding agents (Claude Code, Codex, Gemini, and any other command-line agent) inside Winter: several at once, each on its own branch, out of sight while they work and back in front when they need you. Its first part, burying terminals, is built as of 2026-11-20 and described as it stands; the rest is a plan for review. [architecture.md](architecture.md) describes the code.

## Problem

An agent is a program in a terminal tab, and working with several means watching several tabs. Today that breaks in three places:

- Closing a terminal tab ends its shell, so putting an agent out of sight kills it mid-task. The only way to keep it is to leave its tab open, and a strip of agent tabs crowds out the work.
- Two agents in one checkout edit the same files and step on each other's changes.
- Nothing says which agent is working, which is waiting for an answer, and which is done, short of looking at each tab.

Orca, Conductor, and Warp now offer parallel agents as a matter of course, so a developer comparing tools expects it. Winter already has the parts: terminals, panes and tabs, `winter-mux`, git, Diff, a picker framework, and notifications. The design wires them together rather than building an agent product; Winter stays a workspace where agents are one kind of program.

## How others do it

Neither Orca nor Warp draws its own interface for Claude Code or Codex. Both run the agent's own command line in a terminal and build the frame around it.

- **Orca** gives each agent a git worktree and a terminal in it, with a status dashboard, diff review, and merging. Status comes from the terminal title the agent sets. A session daemon owns every terminal, so quitting Orca leaves the agents running and the next launch reattaches to them. Its issue tracker shows the daemon's cost: requests to stop agents on quit, processes left behind, a daemon drifting versions behind the app, leftover services blocking new terminals.
- **Warp** recognizes known agents in any terminal and adds notifications, inline review comments, and an input editor of its own over the agent's prompt. Notifications come from the agent's hooks, through a plugin Warp ships for Claude Code. Quitting Warp ends the agents, after a warning; restoring them after a restart is an open request.

Winter follows Orca on survival, since `winter-mux` is already the daemon, and Warp on detection, since people start agents by typing their name in a terminal.

## Approach

An agent is two things: a **record** (a worktree, a branch, the branch it started from, which agent it is, and its state) and the **terminal** it runs in. The record is Winter's; the terminal is an ordinary terminal tab with a link to its record. There is no Agent tab kind: the agent's interface is its own, and drawing it would be the agent product this design leaves out.

The features come in two layers:

1. **Detected agents.** Any terminal running a known agent, however it was started, gets status, notifications, and burying when its tab closes.
2. **Managed agents.** An agent started with `Agent: New…` also gets a worktree and branch of its own, keeps running after Winter quits, and can be reviewed, merged, or discarded.

The pieces, in the order they are built:

1. **Buried terminals** (built): closing a terminal with a program running in it keeps the program running out of sight, as closing an editor keeps its buffer.
2. **Managed agents**: `Agent: New…` starts an agent in a worktree of its own, in a `winter-mux` session, and `Agent: List` shows every agent, running or stopped.
3. **Agent status**: every detected or managed agent says whether it is working, waiting for you, or stopped, and a desktop notification says when one needs you.
4. **Review and land**: Diff shows an agent's branch against the one it started from, and one command merges or discards it.
5. **Lifecycle**: stopping every agent at once, cleaning up after removed worktrees, and keeping the mux server and the window in step across upgrades.

What is deliberately left out, and what may come later, is listed at the end.

## Buried terminals

Built. Burying already existed for editors: closing an editor's tab buries its buffer, which a picker brings back. Terminals now share the same mechanism.

### What the user sees

- Closing a terminal's tab while a program runs in it (an agent, a build, a server) buries the terminal. The program keeps running, and a notice says so: "claude runs on, buried: back from Buffer: Switch to…".
- A terminal sitting at its shell prompt has nothing to keep, so closing it ends the shell as before. Idle shells never pile up out of sight.
- `Buffer: Switch to…` lists buried terminals with the buried editors; `Enter` brings one back into the focused pane. `C-S-t` (reopen closed tab) brings back the same terminal, not a new shell.
- `Buffer: Kill` ends a terminal, shown or buried, asking first when a program is running.
- Closing a pane, or switching workspaces from the window, buries the running terminals among its tabs in the same way, and asks nothing about them.
- Quitting asks first when buried terminals still have programs running, as it does for unsaved edits. So does reloading Winter, since the reload's handover does not carry buried terminals.
- A buried terminal whose shell exits on its own goes, with nothing left to come back to.

### How it is built

- `App::buries(tab, closing)` (`window/editor/closing.rs`) decides, for every way a tab can close, whether it is buried or dropped. A terminal is buried when it closes as `Closing::Keep` and `Terminal::has_foreground_process` says a program runs beyond the shell. The question asked before closing (`let_go`) leaves out tabs that will be buried, so nothing is asked about them.
- `OpenTabs::bury` (`window/layout/open_tabs.rs`) puts the tab on the one buried list editors already use; for a terminal it only records the tab, since the terminal keeps running. `OpenTabs::remove` takes any tab off the list, so a killed or dead terminal does not linger there.
- The terminal stays in `OpenTabs` with its PTY, its grid, and its title. The pump (`drain_all_panes`) already reads every open terminal, not only the ones a pane shows, so a buried agent's output is read and its screen kept current.
- `ClosedTab::Editor` became `ClosedTab::Buried`, recorded for any buried tab, so reopening a closed tab brings back a buried terminal as it runs now.
- `show_buffer` (`window/picker/sources/lists.rs`) unburies a terminal into the focused pane; the buffer list gives each buried terminal a row with show and kill actions.
- `reap_dead_panes` drops a buried terminal whose shell exited. It used to take any dead tab no pane shows for one in a workspace put aside, find none, and leave it to be reaped again on every pump.

### Open questions

1. **Surviving a restart.** Buried terminals live in Winter's process: quitting or reloading ends them, after asking. Managed agents survive by running under `winter-mux` instead (below), so this stays open only for plain terminals, such as a build or a server, where asking first is enough.
2. **Detection off Linux.** Without shell integration, `Terminal::has_foreground_process` falls back to the alternate screen and `/proc`, and `/proc` exists only on Linux. An agent drawing on the primary screen on macOS or Windows is then not seen, and closing its tab ends it. Agent detection (below) closes the gap for agents; other programs still need shell integration.
3. **The notice's wording** names the command (`Buffer: Switch to…`) rather than a key, since keys can be rebound. A key hint drawn from the current keymap would be friendlier.

## Managed agents

Planned. `Agent: New…` asks which agent to run, from a list in `settings.kdl`, and in which repository: the focused tab's by default. Then it:

1. Creates a git worktree beside the repository (`<repo>/../<repo>.worktrees/<branch>`, configurable), on a new branch named after the agent and a number (`agent/claude-1`) that no existing branch uses, from the current branch.
2. Starts a `winter-mux` session named after the branch, whose shell starts in the worktree, and types the agent's command into it, so the shell and the agent's last words outlive the agent.
3. Opens a terminal tab attached to that session, named after the agent and its branch.

Each agent edits its own checkout, so agents never conflict until their branches are merged.

### Where an agent lives

The mux server owns the agent's terminal, so the agent keeps running when its tab closes, when Winter reloads, and when Winter quits. Closing the tab buries it; the next launch reattaches it, as a restored session already reattaches `mux:` panes. Only the server's own end, such as a reboot, stops the agent.

The record is not kept in a file of its own. Winter rebuilds it from `git worktree list`, taking the branches under the agent prefix, and joins each with its mux session by name. A worktree with a live session is a running agent; one without is a stopped agent, and its tab, if any, is a link with nothing behind it. So an agent is never lost because Winter forgot about it, and a worktree made in an earlier run still shows up.

`Agent: List` opens a picker of every agent, running or stopped, with its branch and state at the right. `Enter` shows a running agent's terminal, or opens a stopped one again: a new session in its worktree, starting the agent with its resume option (`claude --continue` for Claude Code) so the conversation comes back, though a task cut off mid-way does not.

### Settings

The agents offered come from an `agents { }` block in `settings.kdl` (Winter's users keep only `settings.kdl` and `keybindings.kdl`), with built-in entries for the common command-line agents. An entry gives a name, the command, its arguments, and its resume arguments.

### Moving a detected agent

An agent started by hand in a plain terminal runs in a local PTY, and a running process cannot move into the mux server. `Agent: Manage` offers the next best thing: it stops the agent, creates a worktree and session as `Agent: New…` does, and starts the agent there with its resume option, so the conversation carries over. Uncommitted edits stay in the original checkout unless the user chooses to bring them.

### Open questions

1. **A fresh worktree is a bare checkout.** Ignored files, such as `.env`, `node_modules`, and `target/`, are not in it, and a Rust repository builds from cold in every worktree. A setup command per repository, and an option to share the build directory, would help; where a per-repository setting lives, given the two user files, is open.
2. **Resume options differ by agent.** Each built-in entry needs its agent's resume arguments checked; an agent without one opens again with a fresh conversation.

## Agent status

Planned. Each agent shows a small state in its tab strip entry and in `Agent: List`:

| State | Meaning |
| --- | --- |
| Working | The agent is acting on a request |
| Waiting | The agent asks for an answer or a permission, or has finished its turn |
| Stopped | The agent's program has exited, or a managed agent has no live session |

An agent stays at its own prompt when it finishes a turn, so a finished turn is Waiting, not Stopped: it is the moment the agent needs you.

Winter learns the state from three sources, the first available winning:

1. **The agent's hooks.** Claude Code, Codex, and OpenCode can run a command when a turn ends or a question comes up. Winter ships the hook for each, which runs `winter agent report <state>`. Inside a mux session the report goes to the mux server, which passes it to whichever window is attached and keeps it while none is, since the window that started the agent may be gone; in a local terminal it goes to the window's own socket.
2. **The terminal title.** Many agents set their title to show they are busy, which Winter already reads.
3. **Output and the bell.** A bell, or an OSC 9 or 777 notification, means Waiting; quiet after output means Waiting. This is the fallback only, since an agent's screen redraws spinners and timers while it waits. Winter parses neither OSC 9 nor 777 today.

The same sources tell Winter that a plain terminal runs an agent at all, together with the foreground process name where the system gives one. A terminal known to run an agent is buried when its tab closes, whatever `Terminal::has_foreground_process` says.

An **Agents page** keeps the same list in view while several agents run, with each agent's branch and state, its row actions Review, Merge, Discard, and Stop. It opens in the left sidebar ([panels-design.md](panels-design.md)), and `Enter` on a row shows the agent's terminal in the main area.

A desktop notification (`misty::Notification`) goes out when an agent moves to Waiting or Stopped while its terminal is buried or the window is out of focus, and clicking it shows the agent's terminal.

## Review and land

Planned. On a managed agent's tab, or its row in `Agent: List`:

- `Agent: Review` opens Diff on the agent's branch against the branch it started from, with the worktree's uncommitted changes included.
- `Agent: Merge` lands the branch. It rebases the branch onto the base inside the agent's worktree, so a conflict is resolved there, by the user or by the agent, and never leaves the main checkout half-merged. Then it fast-forwards the base branch, asking first when the main checkout has uncommitted changes, and removes the worktree, the branch, and the session.
- `Agent: Discard` removes the worktree, the branch, and the session, after asking when they hold changes not on the base branch.

`Agent: Merge` and `Agent: Discard` both end the agent's terminal, asking first if the agent is still working.

## Lifecycle

Planned. Agents that outlive the window need ways to end them, and a server that outlives the window needs to stay in step with it. Each answers a problem Orca users have reported:

- **Quitting** says how many agents keep running, and a setting stops them on quit instead.
- **`Agent: Stop All`** ends every managed agent's session, keeping the worktrees.
- **Cleanup**: a session whose worktree has been removed outside Winter is offered for ending in `Agent: List`, and a stopped agent's worktree can be discarded from there.
- **Upgrades**: the mux server and a window of another version refuse to pair. The window says so and offers to restart the server once no agent is working, carrying the stopped agents back through their resume option.

## Steps

1. Buried terminals: done, with tests in `window/editor/tests/questions.rs` and `window/workspace.rs`, and the guide's Basics page updated.
2. Managed agents: the settings block, `Agent: New…` on a mux session, the record rebuilt from worktrees and sessions, `Agent: List`, and opening a stopped agent again.
3. Agent status: the hooks and `winter agent report`, reports through the mux server, the title and output fallbacks, detection in plain terminals, the tab strip state, notifications, and the Agents page, which waits on the panels design's sidebars.
4. Review and land: the three commands on top of Diff and git.
5. Lifecycle: the quit setting, `Agent: Stop All`, cleanup, and the version check. `Agent: Manage` for agents started by hand.

Each step ships on its own and is usable without the next. Each new command gets its row in the guide's pages, and a new page, `docs/guide/agents.md`, opens with what agents in Winter are and how to start one: any agent you run gets status and notifications, and one started with `Agent: New…` also gets its own branch and keeps running after you quit.

## Later

- **An agent page.** A Page showing an agent's conversation in Winter's own interface, for people who prefer it to the terminal, as Zed's agent panel does. It would speak ACP (Agent Client Protocol), which Gemini CLI supports and Claude Code and Codex support through adapters, rather than one agent's own output format. It reuses the record, so worktrees, status, and review are the same for both views.
- **An input editor over the agent's prompt**, as Warp offers: Winter's editor for writing a long request to a terminal agent.

## Not in this design

- A browser inside Winter for pointing agents at a web page (Orca has one).
- A mobile app for watching agents.
- Agents starting other agents, and orchestration between them.
- An agent of Winter's own; Winter runs the agents people already use.
- Sharing agents between machines; `winter-mux` reaches remote servers over SSH, but agents on them are left for later.
