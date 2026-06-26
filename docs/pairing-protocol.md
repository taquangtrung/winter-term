# Snow and Winter pairing protocol

Snow (the editor) and Winter (the terminal) talk over two local sockets so an agent running in one Winter pane and Snow running in another work as one tool. Winter routes file references to Snow, reports agent turns, and delivers text to the agent; Snow reviews what the agent changed and sends context back. Each side keeps its own wire format; the client of a socket speaks that socket's format. This document is the single source of truth: a copy lives in both repositories and the two must stay identical.

## Environment

Winter sets these in every pane it spawns, alongside the existing `TERM_PROGRAM=winter` and `WINTER=1`:

| Variable | Meaning |
|---|---|
| `WINTER_SOCKET` | Absolute path of the window's own control socket (`winter-control-<pid>.sock`) |
| `WINTER_PANE_ID` | Decimal id of the terminal tab the process runs in, unique within the window and stable for its lifetime |

Snow sets `SNOW_SOCKET` to its own control socket for its children (`:!cmd`, jobs, the git editor handoff). A process that sees neither variable is not paired and every feature below is off.

## Snow control socket

Snow listens on `$XDG_RUNTIME_DIR/snow-<pid>.sock` (temp-dir fallback, mode 0600; loopback TCP plus a `snow-<pid>.port` file on Windows). The framing is one line of JSON per request and one line of JSON per response on the same connection. A connection may carry several requests in sequence.

Requests are objects tagged by `cmd`. Every response carries `"ok": true` plus fields, or `"ok": false, "error": "<message>"`. Unknown commands answer with an error; unknown fields are ignored.

| `cmd` | Fields | Response fields | Effect |
|---|---|---|---|
| `hello` | | `name`, `version`, `pid`, `cwd`, `commands` | Capability probe |
| `open` | `path` (absolute), `line?`, `col?` (1-based), `focus?` (default true) | | Open or switch to the file, move the cursor |
| `buffers` | | `buffers: [{path, modified, active}]` | List open file buffers |
| `selection` | | `path`, `start: {line,col}`, `end: {line,col}`, `text` | The active selection, or the cursor line when nothing is selected |
| `diagnostics` | `path?` | `diagnostics: [{path, line, col, severity, message, source}]` | Current diagnostics, for all files or one |
| `turn` | `phase` (`start` or `end`), `agent_pane`, `cwd` | | Agent turn boundary: `start` snapshots a baseline, `end` opens the review buffer when anything changed |
| `review` | `cwd?` | | Open the review buffer against the last baseline, or `HEAD` when none |

The legacy git-editor request `{"open": "<path>", "kind": "commit"}` (no `cmd` field) is still accepted and answered with `{"status": "saved"}` or `{"status": "aborted"}` once the buffer closes, so `snow git-editor` keeps working.

## Winter control socket additions

Winter's control socket uses its existing framing: the client writes one length-prefixed JSON message (4-byte big-endian length, then a serde externally tagged `ControlMessage`), shuts down its write half, and reads the answer. Messages that answer with data reply with one length-prefixed JSON frame; the others reply with the single accept byte `1` or reject byte `0`, matching `Edit`.

| Message | Answer | Effect |
|---|---|---|
| `{"RegisterEditor": {"pane": 7, "socket": "/run/.../snow-123.sock"}}` | byte | Record pane 7 as the window's editor; Winter routes file opens there until the pane closes or another editor registers |
| `{"ListPanes": null}` | `[{"id", "title", "program", "cwd", "busy", "editor"}]` | Every terminal tab in the window; `program` is the foreground process name, `busy` the busy-spinner state |
| `{"Focus": {"pane": 3}}` | byte | Focus that tab, revealing its pane |
| `{"SendText": {"pane": 3, "text": "...", "submit": false}}` | byte | Write the text to the tab's PTY as a bracketed paste when the program enabled it; `submit` appends a carriage return |
| `{"Notify": {"title": "...", "body": "...", "pane": 3}}` | byte | Toast in Winter plus a desktop notification when the window is unfocused |
| `{"Turn": {"phase": "start", "pane": 3, "cwd": "/repo"}}` | byte | Agent turn boundary reported by a hook; Winter forwards it to the registered editor as a Snow `turn` request and, on `end`, notifies |

## Command-line helpers

Hooks and scripts use small CLI entry points instead of speaking the sockets:

```bash
winter notify --title "Claude" --body "Waiting for input"
winter turn start      # reads WINTER_SOCKET, WINTER_PANE_ID, $PWD
winter turn end
snow --remote path/to/file.rs:42:5   # open in the paired Snow, else start one
```

A Claude Code setup pairs them with hooks in `.claude/settings.json`:

```json
{
  "hooks": {
    "UserPromptSubmit": [{"hooks": [{"type": "command", "command": "winter turn start"}]}],
    "Stop": [{"hooks": [{"type": "command", "command": "winter turn end"}]}],
    "Notification": [{"hooks": [{"type": "command", "command": "winter notify --title Claude --body 'Needs attention'"}]}]
  }
}
```

## Flows

Opening a reference from agent output:

```text
gx / Ctrl+click / F8 on "src/foo.rs:42" in the agent pane
  │
  ▼
Winter resolves the path against the pane's OSC 7 directory
  │
  ▼
registered editor? ── no ──> built-in editor (unchanged behaviour)
  │ yes
  ▼
Snow `open` {path, line, col}, then Winter `Focus` the editor pane
```

Reviewing an agent turn:

```text
UserPromptSubmit hook: winter turn start
  │
  ▼
Winter forwards `turn start` to Snow: Snow records a baseline tree
  │
  ▼
agent edits files: Snow reloads clean buffers, flags conflicts
  │
  ▼
Stop hook: winter turn end
  │
  ▼
Snow opens review:// (baseline vs worktree); Winter notifies
```

Sending context to the agent: Snow's `agent.send-selection` command formats the selection as `@path#L10-24`, picks the agent pane (the pane of the last `turn`, else the single busy or agent-named pane from `ListPanes`, else a picker), and sends `SendText` followed by `Focus`.

## Security

Both sockets are owned by the user and mode 0600. Every request is size-capped (64 KiB for Winter, 1 MiB per line for Snow) and read with a timeout. Paths in `open` must be absolute; Snow opens them like a command-line argument and never executes anything a request names. `SendText` writes to a PTY, so Winter only accepts it from its own socket, never from the mux protocol.
