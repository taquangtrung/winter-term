# Terminal Block Protocol (TBP) v1

TBP lets a command-line program hand the terminal typed, structured content instead of a stream of characters, while staying invisible to terminals that do not implement it.

The reference implementation is the [`winter-proto`](../crates/winter-proto) crate. Client libraries live under [`clients/`](../clients).

## Design in one paragraph

A program emits a **MIME bundle**: several representations of the same thing, and the terminal renders the richest one it can. Every bundle carries a `text/plain` fallback, so the same program stays readable under `tmux`, over `ssh`, or in CI. The whole message rides inside a single OSC escape, which any terminal that does not know the sequence discards wholesale. The model is Jupyter's `display_data`, moved to the terminal.

## Framing

One message is one OSC escape:

```text
OSC 9001 ; <verb> ; <params> ; <base64 payload> ST
```

where `OSC` is `ESC ]` (`\x1b]`) and `ST` is `ESC \` (`\x1b\\`).

- Fields are separated by `;`.
- The `<params>` field is a list of `key=value` pairs separated by `,`.
- The `<payload>` field is base64-encoded JSON.
- Both `<params>` and `<payload>` are omitted, along with their leading `;`, when a verb has none.

The payload is base64 rather than raw JSON so the escape contains no `;`, no `ESC`, and nothing a naive parser can trip over. That is what keeps the whole message opaque to terminals that ignore OSC 9001.

## Verbs

| Verb | Params | Payload | Meaning |
|---|---|---|---|
| `emit` | `v`, `id`, `trust` | MIME bundle | A complete block, rendered once |
| `emit` (file) | `v`, `id`, `trust`, `mime`, `file` | none | Payload in a side file |
| `open` | `id`, `mime` | initial spec | Open a live block for incremental updates |
| `patch` | `id` | RFC 6902 patch | Update an open live block |
| `close` | `id` | none | Close a live block |
| `caps` | none | none | Ask what the terminal can render |

`id` is chosen by the emitter. Later `patch` and `close` messages refer back to it. Ids share one namespace per session, which every program writing to that terminal uses, so an emitter counting from 1 collides with the next program to do the same: the terminal refuses an `open` for an id still open, and the later program's patches land on the earlier block. The shipped clients build ids as `pid << 20 | sequence`, the process id above a 20-bit per-process counter, which keeps programs apart and keeps every id below 2^53 so it survives a JavaScript number. A client should also close the live blocks it opens when it exits early.

Parameter values are percent-encoded wherever they hold a byte the framing gives meaning to (`;`, `,`, `=`, `%`) or a control byte, which could end the escape early: `mime=text/html%3Bcharset%3Dutf-8` carries `text/html;charset=utf-8`. Ordinary MIME types, ids, and file names contain none of these and read the same either way.

### emit

```text
ESC ] 9001 ; emit ; v=1,id=1,trust=restricted ; <base64 JSON> ESC \
```

The payload is a JSON object holding the MIME bundle under `mime`, an object mapping MIME type to value, and optionally `meta`:

```json
{
  "mime": {
    "text/markdown": "# Results\n\n| n | t |\n|---|---|\n| 1 | 4 |",
    "text/plain": "Results\n n  t\n 1  4"
  },
  "meta": { "title": "Results", "height_hint": 6 }
}
```

`meta` is optional, and each of its fields is: `title` names the block, and `height_hint` suggests how many rows it takes. A terminal may ignore either.

Include `text/plain` in every bundle. A terminal that cannot render any of the richer types falls back to it, and so does `tmux`, a pipe, or a log file.

### emit with a side-channel file

Terminals cap the length of an OSC string, so a large image inlined as base64 can be truncated and silently dropped. Instead, write the payload to a file in the directory named by the `WINTER_SIDECHANNEL_DIR` environment variable and reference it:

```text
ESC ] 9001 ; emit ; v=1,id=2,trust=restricted,mime=image/png,file=plot.png ESC \
```

Winter exports `WINTER_SIDECHANNEL_DIR` into every shell it spawns. A program that does not see it should inline the payload instead.

### open, patch, close

A live block is one that updates in place: a progress bar, a log tail, a chart that grows.

```text
ESC ] 9001 ; open ; id=3,mime=application/vnd.vega-lite+json ; <base64 JSON spec> ESC \
ESC ] 9001 ; patch ; id=3 ; <base64 JSON patch> ESC \
ESC ] 9001 ; close ; id=3 ESC \
```

Unlike `emit`, an `open` carries a single representation rather than a bundle of alternatives, named by its `mime` parameter.

Each `patch` payload is an [RFC 6902](https://datatracker.ietf.org/doc/html/rfc6902) patch document applied to the block's current spec:

```json
[{"op": "replace", "path": "/data/values/0/y", "value": 42}]
```

Patch folding is deliberately best-effort: a malformed operation is skipped rather than aborting the whole patch, because freezing a progress display at its initial state is a worse outcome for a terminal than showing a slightly stale one.

Each patch is folded into the block's state as it arrives, so a block takes updates for as long as it is open, each costing its own size. One patch applies at most 256 operations (`MAX_PATCH_OPS` in `winter-proto`) and drops the rest; an emitter with more to say sends a whole-document `replace` (`"path": ""`), as the clients do.

### caps

`caps` asks the terminal what it can render, so a tool can pick a representation before emitting anything.

```text
ESC ] 9001 ; caps ESC \
```

**Reserved in v1.** Winter accepts `caps` and does not answer it yet, so a client must never wait for a reply: send it, if at all, only alongside a fallback that works without one. A terminal that answers sends the reply not as another TBP escape but as a JSON object on the querying tool's stdin, in this form:

```json
{
  "live": true,
  "mime": ["application/vnd.vega-lite+json", "text/html", "image/svg+xml",
           "text/markdown", "text/csv", "image/png", "image/jpeg", "image/gif",
           "image/webp", "application/json",
           "text/plain"],
  "side_channel": true,
  "tiers": ["trusted", "restricted", "isolated"],
  "v": 1
}
```

| Field | Meaning |
|---|---|
| `live` | Whether `open`/`patch`/`close` are supported, or only one-shot `emit` |
| `mime` | Renderable MIME types, richest first |
| `side_channel` | Whether `file=` payload references are resolved |
| `tiers` | Trust tiers the terminal's policy will actually grant |
| `v` | The TBP version the terminal speaks |

A tier absent from `tiers` will be clamped away, so there is no point requesting it.

## Trust tiers

`trust` on an `emit` is a **request, never a grant**.

```text
isolated  <  restricted  <  trusted
```

| Tier | What a terminal that renders blocks in a web engine grants |
|---|---|
| `isolated` | The least: sandboxed, no scripts |
| `restricted` | No network, no navigation. **The default.** |
| `trusted` | The most a terminal's policy allows |

Every byte reaching a terminal is attacker-controlled. A `cat` of a downloaded file, output piped from `curl`, or a program on the far side of an `ssh` can all spell `trust=trusted`, and nothing on the wire authenticates the emitter. So a terminal that renders blocks in a web engine clamps a requested tier down to its configured ceiling and never raises it.

Winter has nothing for a tier to grant: it draws every block natively, runs nothing in one, and fetches nothing for one. It accepts any tier and draws the block the same way. A client should still request the lowest tier that renders its content correctly, for terminals where it matters.

The ordering above is load-bearing: the clamp is a minimum over it. Never compare tiers any other way.

## Versioning

`emit` carries `v=1`, and `open` and `patch` may. A message declaring a version newer than the terminal implements is refused rather than read with the older meaning; one without `v` predates the parameter and is read as version 1. Until 1.0, the wire format may change in a minor release of the crates; see [`CHANGELOG.md`](../CHANGELOG.md).

## Limits

A terminal bounds what one message may hold. Winter's:

| What | Limit |
|---|---|
| One OSC string, a whole TBP escape | 16 MiB; past it the escape is cut off and dropped |
| Operations applied from one `patch` | 256 (`MAX_PATCH_OPS`); the rest are dropped |
| Live blocks open at once | 1,024; an `open` past it is refused |
| An `id` | Any `u64`; the clients keep theirs below 2^53 |
| A `file=` name | One name inside `WINTER_SIDECHANNEL_DIR`, never a path out of it |

An unknown verb, or a message that does not parse, is dropped without a reply.

[`tbp-golden-vectors.json`](tbp-golden-vectors.json) holds the exact escape for a set of messages. The reference codec and every shipped client are tested against it, and a new client can be too.

## Writing a client

Three clients ship with Winter, and each is small enough to read end to end:

| Language | Path |
|---|---|
| Rust | [`clients/client-rs`](../clients/client-rs) |
| Python | [`clients/client-py`](../clients/client-py) |
| Shell | [`clients/client.sh`](../clients/client.sh) |

A client should:

1. Emit nothing but the `text/plain` fallback when the terminal is not Winter, and turn later `patch`/`close` calls into no-ops. `clients/client-rs` does exactly this.
2. Always include `text/plain` in a bundle.
3. Request the lowest trust tier that renders the content correctly.
4. Use a side-channel file for payloads over a few kilobytes.
