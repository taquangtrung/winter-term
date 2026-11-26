# Monitors

## Process Monitor

Process Monitor (`C-S-,`, or `Tool: Process Monitor`) lists every process on the machine, refreshed every two seconds, as a tree of which process started which. A summary at the top says how many there are and how they are listed. The footer describes the selected process: its ids, owner, shares, paths, and full command line.

| Key | Action |
|---|---|
| `j` `k` | Move |
| Click a heading | Sort by that column; click it again to reverse |
| Click a process | Select it |
| `s` / `S` | Sort by the next column, reverse |
| `=` | Switch between the tree and a flat list |
| `Enter` `Space` `Tab`, `h` `l` | Fold, unfold the children |
| `/`, `n` `N` | Search by name, command, pid, or user |
| `f` / `Esc` | Filter, clear the filter |
| `x` / `X` | Terminate, kill (asks first) |
| `t` / `T` | Terminate, kill with its children (asks first) |
| `z` / `Z` | Suspend, resume |
| `R` | Change the priority |
| `r` / `p` | Refresh now, pause or resume |
| `y` / `Y` | Copy the pid, the command line |

CPU is a share of one core, so a busy process can pass 100. A VRAM column appears when `nvidia-smi` answers. Signals go through the system's own `kill`, so another user's process needs `sudo` from a terminal.

## System Monitor

System Monitor (`Tool: System Monitor`) is a dashboard of the whole machine, refreshed every two seconds. Its cards stack in one column:

- **CPU:** model, core count, load averages, total use with its recent history, and a vertical bar chart of each core.
- **Memory:** RAM use with its recent history, and swap use when present.
- **Disks:** each mounted volume's path, file system, size, and capacity bar.
- **GPU:** each NVIDIA card's name, video memory, load, and temperature, when `nvidia-smi` answers.

Usage bars and percentages turn amber at 75 percent and red at 90.

| Key | Action |
|---|---|
| `j` `k`, `C-e` `C-y` | Scroll |
| `gg` / `G` | Top, bottom |
| `]` / `[` | Next, previous card |
| `r` / `p` | Refresh now, pause or resume |
