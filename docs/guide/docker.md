# Docker

Docker (`Docker: Containers, Images, Volumes, Networks`) lists what the Docker daemon knows, one kind at a time, refreshed every three seconds. It runs the `docker` command, so it can reach what your shell can.

| Key | Action |
|---|---|
| `Tab` / `S-Tab` | Containers, images, volumes, networks |
| `Enter` | Inspect |
| `r` | Refresh now |
| `/`, `n` `N` | Search |
| `y` / `Y` | Copy the name, the id |
| `D` | Remove (asks first) |
| `C` | Prune what is unused (asks first) |
| `s` / `x` / `R` | Start, stop, restart a container |
| `p` / `K` | Pause or unpause, kill a container |
| `e` / `a` / `L` | Open a shell in it, attach, follow its logs, in a new terminal tab |
| `o` / `t` / `S` | Its last 500 log lines, its processes, its CPU and memory |
| `P` / `H` | Pull an image again, read its layers |

Output such as an inspect or a page of logs replaces the list; `q` goes back.
