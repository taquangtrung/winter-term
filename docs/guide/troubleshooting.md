# When Winter misbehaves

What to do when Winter freezes, lags, or crashes, and what to send with an issue. The `Profiling:` commands are in the command palette; the `winter` commands run in a terminal, outside Winter if the window is frozen.

## Profiling

Profiling is off by default. Turn it on to have Winter record what goes wrong, then leave it on until the problem shows again.

| Command | What it does |
|---|---|
| `Profiling: Profile the Window (Toggle)` | Turns profiling on or off for this window. |
| `winter --profile` | Opens a window with profiling on. |

While profiling is on:

- A moment the window took longer than a tenth of a second is kept, with what it was doing and the tab in front.
- When the window stops answering for five seconds, every thread's stack goes to `crash.log`, and how long it was stuck once it answers again. You do not need to catch it in the act.
- Warnings and errors go to `winter.log`.
- The report says how many frames the window drew in the last ten seconds, how long they took, and which tabs' output asked for them. If Winter uses a lot of CPU while you are not typing, write a report then.
- It stays on through `Window: Reload`.

## Looking into a window

These work whether profiling is on or not; the slow moments are there only once it is on.

| Command | What it does |
|---|---|
| `Profiling: Show What the Window Is Doing` | Opens the recent slow moments and every thread's stack in a tab. |
| `Profiling: Write a Report` | Opens a report to attach to an issue: versions, system, GPU, the programs running in your tabs, stacks, `settings.kdl`, and the last lines of the logs. |
| `winter profile stacks` | Prints what `Profiling: Show What the Window Is Doing` shows. Works on a frozen window. |
| `winter profile report [file]` | Writes the report to a file. Works on a frozen window. |

- The window pauses for a few seconds while its stacks are read.
- Look a report over before you share it: it holds your `settings.kdl` whole.
- Stacks need `gdb` installed. Without it everything else is still there.

## Where Winter writes

All of these are in Winter's state directory, `~/.local/state/winter-term` on Linux.

| File | Holds |
|---|---|
| `crash.log` | Every crash and error Winter recovered from, and while profiling is on, each freeze with stacks. |
| `winter.log` | While profiling is on, warnings and errors with the time each was said. The one before it is kept as `winter.log.old`. |
| `reports/` | What the `Profiling:` commands wrote. |
