# Agent workspace design

This document is the design for running AI coding agents (Claude Code, Codex, Gemini, Antigravity, and any other command-line agent) inside Winter: several at once, each on its own branch, out of sight while they work and back in front when they need you. Burying terminals (2026-11-20), managed agents, agent status, review and land, and the lifecycle (2026-11-24) are built and described as they stand; what is still open is listed under each part's open questions. [architecture.md](../internals/architecture.md) describes the code.

## Problem

An agent is a program in a terminal tab, and working with several means watching several tabs. Today that breaks in three places:

- Closing a terminal tab ends its shell, so putting an agent out of sight kills it mid-task. The only way to keep it is to leave its tab open, and a strip of agent tabs crowds out the work.
- Two agents in one checkout edit the same files and step on each other's changes.
- Nothing says which agent is working, which is waiting for an answer, and which is done, short of looking at each tab.

Winter already has the parts: terminals, panes and tabs, `winter-mux`, git, Diff, a picker framework, and notifications. The design wires them together rather than building an agent product; Winter stays a workspace where agents are one kind of program.

## Principles

- **The agent keeps its own interface.** An agent's command line is its interface, and its authors keep improving it. Winter runs it in a terminal and builds the frame around it: where it runs, whether it needs you, and what it changed.
- **Any agent, however started.** People start an agent by typing its name in a terminal, so status, notifications, and burying work in any terminal, not only for agents Winter started.
- **An agent outlives the window.** A long task should not end because a tab closed or Winter restarted. `winter-mux` already keeps terminals alive apart from the window, so a managed agent runs there.
- **Nothing runs unseen and unstoppable.** What outlives the window can be listed, stopped, and cleaned up from Winter, and a server and a window that disagree on their version say so rather than misbehave.
- **Each agent on its own branch.** An agent edits a checkout of its own, and its work lands only when the user reviews and merges it.

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

Built as of 2026-11-24. `Agent: New…` asks which agent to run, from a list in `settings.kdl`, and in which repository: the focused tab's by default. Then it:

1. Creates a git worktree beside the repository (`<repo>/../<repo>.worktrees/<branch>`, configurable), on a new branch named after the agent and a number (`agent/claude-1`) that no existing branch uses, from the current branch.
2. Starts a `winter-mux` session named after the branch, whose shell starts in the worktree, and types the agent's command into it, so the shell and the agent's last words outlive the agent.
3. Opens a terminal tab attached to that session, named after the agent and its branch.

Each agent edits its own checkout, so agents never conflict until their branches are merged.

### Where an agent lives

The mux server owns the agent's terminal, so the agent keeps running when its tab closes, when Winter reloads, and when Winter quits. Closing the tab buries it; the next launch reattaches it, as a restored session already reattaches `mux:` panes. Only the server's own end, such as a reboot, stops the agent, and even then not for long: the server keeps each session's recipe on disk and brings it back when it next starts. Winter gives the server the agent's resume line with the session (`SetResume`), kept in the recipe, and the server types it into the shell it brings back, so the agent comes back with its conversation rather than as a bare shell.

The record is not kept in a file of its own. Winter rebuilds it from `git worktree list`, taking the branches under the agent prefix, and joins each with its mux session by name. A worktree with a live session is a running agent; one without is a stopped agent, and its tab, if any, is a link with nothing behind it. So an agent is never lost because Winter forgot about it, and a worktree made in an earlier run still shows up.

`Agent: List` opens a picker of every agent, running or stopped, with its branch and state at the right. `Enter` shows a running agent's terminal, or opens a stopped one again: a new session in its worktree, starting the agent with its resume option (`claude --continue` for Claude Code) so the conversation comes back, though a task cut off mid-way does not.

### Settings

The agents offered come from an `agents { }` block in `settings.kdl` (Winter's users keep only `settings.kdl` and `keybindings.kdl`), with built-in entries for Claude Code, Codex, Gemini, Antigravity (`agy`), GitHub Copilot, Kiro (`kiro-cli chat`), opencode, and pi. An entry gives a name, the command, its arguments, and its resume arguments. A plain terminal is known to run an agent when its foreground program is an entry's name or its command's first word.

### Moving a detected agent

Built as of 2026-11-24. An agent started by hand in a plain terminal runs in a local PTY, and a running process cannot move into the mux server. `Agent: Manage` offers the next best thing: it stops the agent, creates a worktree and session as `Agent: New…` does, and starts the agent there with its resume option, so the conversation carries over. It makes the worktree before it ends the agent, so an agent that cannot move keeps running, and it leaves an agent at work alone. Uncommitted edits stay in the original checkout; bringing them along is left for later. An agent that keeps conversations by directory, as Claude Code does, finds none in the new worktree and starts afresh.

### Open questions

1. **A fresh worktree is a bare checkout.** Ignored files, such as `.env`, `node_modules`, and `target/`, are not in it, and a Rust repository builds from cold in every worktree. A setup command per repository, and an option to share the build directory, would help; where a per-repository setting lives, given the two user files, is open.
2. **Resume options differ by agent.** Claude Code's (`--continue`), Codex's (`resume --last`), Antigravity's, Copilot's, opencode's, and pi's (`--continue`), and Kiro's (`chat --resume`) are set, each from its own `--help`; Gemini has none, so it opens again with a fresh conversation. An agent's last conversation is that of its folder for Claude Code, and by their `--help` for pi and Kiro too, so `Agent: Manage` cannot carry theirs over; the others are not checked.

## Agent status

Built as of 2026-11-24. Each agent shows a small state in its tab strip entry and in `Agent: List`:

| State | Meaning |
| --- | --- |
| Working | The agent is acting on a request |
| Waiting | The agent asks for an answer or a permission, or has finished its turn |
| Stopped | The agent's program has exited, or a managed agent has no live session |

An agent stays at its own prompt when it finishes a turn, so a finished turn is Waiting, not Stopped: it is the moment the agent needs you.

Winter learns the state from three sources, the first available winning:

1. **The agent's hooks.** Claude Code and Codex can run a command when a turn ends or a question comes up. The guide gives the hook for each, which runs `winter agent report <state>`; Winter does not write it into their settings. Inside a mux session the report goes to the mux server, which passes it to whichever window is attached and keeps it while none is, since the window that started the agent may be gone; in a local terminal it goes to the window's own socket.
2. **The terminal title and screen.** Many agents set their title to show they are busy, which Winter already reads: a spinner glyph leading it means Working, any other title Waiting. An agent that sets no title, as Antigravity does, draws its spinner on the screen instead (`⣽  Generating...`), so each time an agent's terminal prints, a visible line led by a braille spinner, a space, and a word also means Working. A desktop notice owed for Waiting is dropped if the agent is back at work by the time it would go out, since a screen read between two frames can miss the spinner.
3. **The bell.** A bell, or an OSC 9 or 777 notification, from a terminal already known to run an agent means Waiting. Quiet after output is not read as Waiting, since an agent's screen redraws spinners and timers while it waits.

The same sources tell Winter that a plain terminal runs an agent at all, together with the foreground process name where the system gives one. A terminal known to run an agent is buried when its tab closes, whatever `Terminal::has_foreground_process` says. An agent waiting at its prompt sets no new title, and after a reload the window knows of no agent, so the Agents view also looks over the terminals each time it fills or changes and takes in any running a known agent, as its title says.

Two sidebar views ([Panels](../guide/basics.md#panels)) keep it in sight, in the left sidebar, 300 pixels wide until the user drags it. In both, each row's subtitle is its folder, dimmed and in the cell grid's font, shortened only as far as the panel's width needs (the cells `Page::content` is given), its folders from the left each to its first letter as fish's prompt writes them (`~/W/apps/winter-term`, then `~/W/a/winter-term`), the last never; the row's tooltip says it in full. Each state is an icon (`sync` at work, `bell-dot` waiting, `circle-filled` running, `circle-outline` stopped, `check` done, `error` failed), its words on hover.

- **The Agents view** manages agents: the managed ones, each with its branch and state and the row actions Review, Merge, Discard, and Stop, then the detected ones by the agent's name. Each folds open on what it is doing (its terminal's title, past the glyph that shows how it stands) and, for a managed one, the branch it started from. `Agent: Manage` is not among a detected agent's row actions yet; it runs from the agent's tab. Starting an agent from the view, rather than with `Agent: New…`, is left for later.
- **The Terminals view** is the terminal manager: every terminal open in the window, agents' among them, so it gives one look at everything running. The window's workspace's come first, buried ones among them, then each workspace put aside's under its name. Each is titled by its running command, else its last one, and marked as its command went, or as the agent in it stands. Winter never injects anything into the user's shell: it reads the foreground program's command line from `/proc` (`Terminal::foreground_command_line`, Linux only) and keeps the last it saw (`Terminal::last_command`), noted on every look, on output or the view's own. That says nothing of how a command went, and misses builtins and commands that end between two looks. Where the user sources shell integration (OSC 133), a finished command comes from it, marked done or with the code it failed with; its line is read off the screen when the `C` mark says it was run (from where the `B` mark left the cursor), not taken from what the shell printed while it was edited, which a shell drawing suggestions as you type (zsh's autosuggestions) fills with every redraw. A running command's line comes from the processes first.

Both are kept current while shown: as agents change state, as terminals retitle themselves or their programs finish, and as tabs open, close, or move. `Enter`, or a click on anything that runs, on its row or its lines, shows its terminal in the main area, going first to the workspace put aside it is in; a stopped agent takes a double click, since that starts it again; an agent's fold mark is a target of its own.

A desktop notification (`misty::Notification`) goes out when an agent moves to Waiting or Stopped while its terminal is buried or the window is out of focus, and clicking it shows the agent's terminal.

## Review and land

Built as of 2026-11-24. On a managed agent's tab, its row in `Agent: List`, or its row in the Agents view:

- `Agent: Review` opens Diff on the agent's branch against the branch it started from, with the worktree's uncommitted changes included.
- `Agent: Merge` lands the branch. It rebases the branch onto the base inside the agent's worktree, so a conflict is resolved there, by the user or by the agent, and never leaves the main checkout half-merged. Then it fast-forwards the base branch, asking first when the main checkout has uncommitted changes, and removes the worktree, the branch, and the session.
- `Agent: Discard` removes the worktree, the branch, and the session, after asking when they hold changes not on the base branch.

`Agent: Merge` and `Agent: Discard` both end the agent's terminal, asking first if the agent is still working.

## Lifecycle

Built as of 2026-11-24. Agents that outlive the window need ways to end them, and a server that outlives the window needs to stay in step with it:

- **Quitting** says how many agents keep running, and a setting stops them on quit instead.
- **`Agent: Stop All`** ends every managed agent's session, keeping the worktrees.
- **Cleanup**: a session whose worktree has been removed outside Winter is offered for ending in `Agent: List`, and a stopped agent's worktree can be discarded from there.
- **Upgrades**: the mux server and a window of another version refuse to pair. When `Agent: New…` or `Agent: List` meets such a server, the window asks whether to restart it, ending it by the process id it writes beside its socket (`winter-mux.sock.pid`), and opens the repository's agents again, typing the resume line only where the old server's recipe did not keep one. The window cannot ask that server whether an agent is at work, so the question says a working agent ends too and leaves the choice to the user. A server from before the process id file is ended by hand.

## Steps

1. Buried terminals: done, with tests in `window/editor/tests/questions.rs` and `window/workspace.rs`, and the guide's Basics page updated.
2. Managed agents: done, with the settings block, `Agent: New…` on a mux session (starting a mux server when none answers), the record rebuilt from worktrees and sessions, `Agent: List`, and opening a stopped agent again; tests in `window/agents/` and `config/agents.rs`, and the guide's Agents page.
3. Agent status: done. `winter agent report`, which finds its terminal by the process ids above it, or, inside a mux session (`WINTER_MUX_SESSION`), reports to the mux server, which keeps it per session and passes it to the windows attached that ask (`Attach`'s `agents`, absent from older clients, so no protocol version change); the hooks for Claude Code and Codex, given in the guide's Agents page rather than written into their settings; the title fallback (a spinner glyph leading the title means Working), which also detects an agent in a plain terminal by its foreground program; a bell and OSC 9 and 777 marking an agent Waiting; the tab strip's spinner and dot; a desktop notification for an agent waiting out of sight, which shows its terminal when clicked; and the Agents view, which lists detected agents under the managed ones, folds each open on its task, and takes in agents already running after a reload.
4. Review and land: done. `Agent: Review`, `Agent: Merge`, and `Agent: Discard`, on the focused tab's agent, a row of `Agent: List`, or a row of the Agents view. The branch an agent started from is kept in git (`branch.<agent>.winterBase`), so the record is still rebuilt from git alone.
5. Lifecycle: done. `stop-on-quit`, and a notification on quitting saying how many agents keep running; `Agent: Stop` and `Agent: Stop All`; sessions whose worktrees are gone, offered for ending in `Agent: List`; restarting a mux server of another version, which a new server no longer displaces either (`MuxClient::probe` tells it from none); and `Agent: Manage` for agents started by hand.

Each step ships on its own and is usable without the next. Each new command gets its row in the guide's pages, and a new page, `docs/guide/agents.md`, opens with what agents in Winter are and how to start one: any agent you run gets status and notifications, and one started with `Agent: New…` also gets its own branch and keeps running after you quit.

## Later

- **An agent page.** A Page showing an agent's conversation in Winter's own interface, for people who prefer it to the terminal. It would speak ACP (Agent Client Protocol), which Gemini CLI supports and Claude Code and Codex support through adapters, rather than one agent's own output format. It reuses the record, so worktrees, status, and review are the same for both views.
- **An input editor over the agent's prompt**: Winter's editor for writing a long request to a terminal agent.

## Not in this design

- A browser inside Winter for pointing agents at a web page.
- A mobile app for watching agents.
- Agents starting other agents, and orchestration between them.
- An agent of Winter's own; Winter runs the agents people already use.
- Sharing agents between machines; `winter-mux` reaches remote servers over SSH, but agents on them are left for later.
