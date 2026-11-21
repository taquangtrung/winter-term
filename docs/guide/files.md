# Files

## The file browser

The file browser (`C-S-f`, or `File: Browse Files`) walks directories in a picker to open a file quickly. It starts on the path under the terminal's cursor when there is one, else in the working directory.

| Key | Action |
|---|---|
| Typing | Filter the directory's entries |
| `/` after a name | Go into that directory and keep typing: `src/`, `../`, `~/` |
| `Up` `Down` | Move |
| `Tab` or `Right` | Go into a directory |
| `C-h` or `Left` | Go up |
| `Enter` | Open a file, or list a directory in Dir |
| `Esc` or `C-S-f` | Close |

## Files as root, in containers, and over SSH

A remote file is named by a URI, which works wherever a path does: the browser, the text editor, the recent list, and a saved session.

| URI | Reaches |
|---|---|
| `sudo:///etc/hosts` | A file as root, through `sudo` |
| `docker://web/etc/nginx.conf` | Inside the running container `web` |
| `docker://www-data@web/var/www` | The same, as user `www-data` |
| `ssh://me@example.com:2222/srv` | On an SSH host |
| `docker://` or `ssh://` | The list of containers, or of hosts in `~/.ssh/config` |

`File: Browse Files as Root`, `File: Browse Docker Containers`, and `File: Browse SSH Hosts` open the browser on them.

- The status bar shows where the file lives: `[sudo]`, `[docker://web]`, `[ssh://me@host]`.
- Saving a file you can read but not write asks whether to save it as root. When `sudo` wants a password, a dialog asks; the password is not kept.
- SSH needs key authentication, since Winter does not prompt for passwords.
- Dir, Git, and the PDF viewer work on local files only.
