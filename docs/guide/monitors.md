# Monitors

Four pages show what the machine is doing, each from a `Monitor:` command in the command palette:

| Command | Shows |
|---|---|
| `Monitor: Processes` (`C-S-,`) | Every process, with the actions of a task manager |
| `Monitor: System` | The whole machine: CPU, memory, network, disks, GPUs |
| `Monitor: Ports` | Every socket and the process holding it |
| `Monitor: Startup` | What starts on its own at boot or login |

Each reads only while it is shown, and catches up when you come back to it.

## Process Monitor

The Process Monitor lists every process on the machine, refreshed every two seconds, as a tree of which process started which. A summary at the top says how many there are and how they are listed. The footer describes the selected process: its ids, owner, shares, paths, and full command line.

| Key | Action |
|---|---|
| `j` `k` | Move |
| Click a heading | Sort by that column; click it again to reverse |
| Click a process | Select it |
| `Enter` | Look into the process (see below) |
| `s` / `S` | Sort by the next column, reverse |
| `=` | Switch between the tree and a flat list |
| `Tab`, `h` `l` | Fold, unfold the children |
| `/`, `n` `N` | Search by name, command, pid, or user |
| `f` / `Esc` | Filter, clear the filter |
| `x` / `X` | Terminate, kill (asks first) |
| `t` / `T` | Terminate, kill with its children (asks first) |
| `z` / `Z` | Suspend, resume |
| `R` | Change the priority |
| `r` / `p` | Refresh now, pause or resume |
| `y` / `Y` | Copy the pid, the command line |

CPU is a share of one core, so a busy process can pass 100. A VRAM column appears when `nvidia-smi` answers. Signals go through the system's own `kill`, so another user's process needs `sudo` from a terminal.

### Looking into a process

`Enter` shows the selected process in the same tab, in sections:

- **Open Files:** each descriptor, and whether it is a file, a pipe, or a socket.
- **Sockets:** its network connections.
- **Libraries:** the shared libraries it loaded.
- **Memory:** what it maps, the most in memory first: each file, the heap, the stack, anonymous memory.
- **Environment:** the variables it was started with.
- **Threads:** each thread's id, state, and name.

It is read once when opened; `r` reads it again. Another user's process keeps most of this to itself, and each section says so. On Linux every section is filled; on macOS the files, sockets, and libraries; on Windows the sockets.

| Key | Action |
|---|---|
| `q` / `Esc` | Back to the list |
| `j` `k`, `C-e` `C-y` | Scroll |
| `]` / `[` | Next, previous section |
| `r` | Read again |
| `y` / `Y` | Copy the pid, the command line |

## System Monitor

The System Monitor is a dashboard of the whole machine, refreshed every two seconds. Its cards stack in one column:

- **CPU:** model, core count, load averages, total use with its recent history, and a vertical bar chart of each core.
- **Memory:** RAM use with its recent history, and swap use when present.
- **Network:** each interface's receive (`↓`) and send (`↑`) rates, with the recent history of both together.
- **Disks:** each mounted volume's path, file system, size, and capacity bar.
- **Disk Activity:** each disk's read and write rates, with their recent history.
- **GPU:** each NVIDIA card's name, video memory, load, and temperature, when `nvidia-smi` answers.

Usage bars and percentages turn amber at 75 percent and red at 90. The rates need two readings, so they show after the first refresh. Disk Activity is shown on Linux only.

| Key | Action |
|---|---|
| `j` `k`, `C-e` `C-y` | Scroll |
| `gg` / `G` | Top, bottom |
| `]` / `[` | Next, previous card |
| `r` / `p` | Refresh now, pause or resume |

## Ports

The Ports page lists the sockets waiting for connections and the process holding each, refreshed every two seconds: what answers "what is on port 3000?". `a` lists every socket, connections too. The footer describes the selected socket.

| Key | Action |
|---|---|
| `j` `k` | Move |
| Click a heading | Sort by that column; click it again to reverse |
| `a` | Listening sockets only, or every socket |
| `s` / `S` | Sort by the next column, reverse |
| `/`, `n` `N` | Search |
| `f` / `Esc` | Filter, clear the filter |
| `x` / `X` | Terminate, kill the process holding it (asks first) |
| `r` / `p` | Refresh now, pause or resume |
| `y` / `Y` | Copy the address, the whole line |

A socket held by another user's process shows no program, and cannot be ended from here.

## Startup

The Startup page lists what starts on its own, and whether each is on:

- **systemd:** your services, on and off, and the machine's that are on (Linux).
- **autostart:** the desktop's login entries (Linux), or Windows's startup commands.
- **launchd:** launch agents and daemons (macOS).
- **cron:** the lines of your crontab.
- **shell:** the files every new shell reads, such as `.bashrc` and `.zshrc`.

It is read when first shown; `r` reads it again.

| Key | Action |
|---|---|
| `j` `k` | Move |
| `Enter` | Open the file that sets it up |
| `t` | Turn one of your own services on or off |
| `/`, `n` `N` | Search |
| `f` / `Esc` | Filter, clear the filter |
| `r` | Read again |
| `y` / `Y` | Copy the name, the command |

Anything else is changed in its file. For a service of the whole machine, `t` says the `sudo systemctl` command to run in a terminal.
