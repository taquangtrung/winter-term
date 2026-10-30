# Security policy

## Reporting a vulnerability

Please report security issues privately, not as a public GitHub issue.

Use [GitHub's private vulnerability reporting](https://github.com/taquangtrung/winter-term/security/advisories/new) for the repository. If that is unavailable to you, email <taquangtrungvn@gmail.com> with `winter-term security` in the subject.

Please include the version or commit, your platform, and the smallest reproduction you can manage. A byte stream that triggers the problem is ideal, since most of the attack surface here is exactly that.

Expect an acknowledgement within a week. Winter is maintained by one person in their spare time, so please allow reasonable time for a fix before disclosing publicly.

## Supported versions

While the major version is `0`, only the latest release receives security fixes. There are no maintained release branches.

## What counts as a vulnerability

Winter's threat model starts from one assumption: **every byte arriving from a PTY is attacker-controlled.** A `cat` of a downloaded file, output piped from `curl`, or a program on the far side of an `ssh` can all write arbitrary escape sequences into the terminal. Anything that turns those bytes into more capability than the user granted is in scope.

In scope, and treated seriously:

- A crash, hang, panic, or unbounded memory growth reachable from PTY output. The VT parser, the OSC 133 block state machine, and the TBP codec all parse untrusted input by definition. Both fuzz suites under `crates/*/tests/` exist for this class.
- A rich block causing anything but its own drawing: running code, making a network request, reading a file, or crashing the renderer. Winter draws blocks natively and runs nothing in them; see the [protocol spec](docs/terminal-block-protocol-spec.md#trust-tiers).
- Reading the clipboard, spawning a process, or writing outside the side-channel directory without the user having opted in.
- A TBP side-channel `file=` reference resolving outside `WINTER_SIDECHANNEL_DIR`.
- Anything in the multiplexer that lets one user attach to another user's session over the local socket.

Out of scope:

- Advisories against dependencies that are already acknowledged in [`.cargo/audit.toml`](.cargo/audit.toml), each with the reason it cannot be fixed here. `cargo audit` runs in CI and fails on anything not listed there.
- A shell doing something dangerous because the user typed it. Winter runs the shell you tell it to.
- Rendering artefacts, wrong colours, or layout bugs with no capability consequence. Those are ordinary issues, and welcome as such.

## Hardening already in place

- The protocol, core, render, spell, and client crates set `#![forbid(unsafe_code)]`; the app crate denies it and allows it only around a few system calls, each with a `SAFETY` comment.
- Rich blocks are drawn natively, with no web engine: an image is decoded, an SVG and a chart drawn, and HTML turned into text with its scripts dropped, so nothing arriving from a PTY is ever run, and drawing a block never makes a network request. The Markdown preview editor lays pages out with Blitz, with no scripting, and reads only small regular files a page links to, never the network.
- A project's `.winter/tasks.kdl` runs commands on save only once the user has trusted that file, and again after it changes.
- PDFs are read by a renderer written in Rust (`hayro`), which forbids `unsafe` code.
- OSC 52 clipboard reads are opt-in (the top-level `clipboard-read` setting), because the query is silent on the querying side.
- Every unbounded accumulator has an explicit cap: retained block output, a command's text, the block count, live-block patch size, OSC strings, scrollback rows, the mux client outbox, and the APC payload buffer. See the resource budgets in [`docs/architecture.md`](docs/architecture.md).
- `cargo audit` runs in CI on every pull request.
