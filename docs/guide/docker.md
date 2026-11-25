# Docker

Docker (`Docker: Containers, Images, Volumes, Networks`) lists what the Docker daemon knows, one kind at a time. It is an extension Winter ships, allowed to run only the `docker` command, so it can reach what your shell can. The list is read again as its tab comes into sight, after each action, on `r`, and every 30 seconds while it stays in sight.

| Key | Action |
|---|---|
| `Tab` / `S-Tab` | Containers, images, volumes, networks |
| `j` / `k`, `g g` / `G` | Next, previous, first, last row |
| `Enter` | Inspect |
| `r` | Refresh now |
| `/` | Find a row in the picker |
| `y` / `i` | Copy the name, the id |
| `D` | Remove (asks first) |
| `C` | Prune what is unused (asks first) |
| `s` / `x` / `R` | Start, stop, restart a container |
| `p` / `K` | Pause or unpause, kill a container (kill asks first) |
| `e` / `a` / `L` | Open a shell in it, attach, follow its logs, in a new terminal tab |
| `o` / `t` / `S` | Its last 500 log lines, its processes, its CPU and memory |
| `P` / `H` | Pull an image again, read its layers |

Output such as an inspect or a page of logs replaces the list: `Y` copies the line under the cursor, and `q` goes back.

The keys are rebound in the `docker.list` block of `keybindings.kdl`.
