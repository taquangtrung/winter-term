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
- A rich block escaping its trust tier: scripting, network access, filesystem access, or navigation that `restricted` is supposed to deny. See the [protocol spec](docs/terminal-block-protocol-spec.md#trust-tiers) for what each tier grants.
- A trust tier arriving on the wire being honoured above the configured ceiling (`security { block-max-trust ... }`). A tier on the wire is a request, never a grant.
- Reading the clipboard, spawning a process, or writing outside the side-channel directory without the user having opted in.
- A TBP side-channel `file=` reference resolving outside `WINTER_SIDECHANNEL_DIR`.
- Anything in the multiplexer that lets one user attach to another user's session over the local socket.

Out of scope:

- Advisories against dependencies that are already acknowledged in [`.cargo/audit.toml`](.cargo/audit.toml), each with the reason it cannot be fixed here. `cargo audit` runs in CI and fails on anything not listed there.
- A shell doing something dangerous because the user typed it. Winter runs the shell you tell it to.
- Rendering artefacts, wrong colours, or layout bugs with no capability consequence. Those are ordinary issues, and welcome as such.

## Hardening already in place

- Every crate sets `#![forbid(unsafe_code)]`.
- `restricted` is the default trust ceiling for rich blocks, so nothing arriving from a PTY reaches scripting without configuration.
- Remote asset fetching for rich blocks is opt-in (`security { block-remote-assets ... }`): rendering a block does not make a network request the user did not ask for.
- OSC 52 clipboard reads are opt-in (the top-level `clipboard-read` setting), because the query is silent on the querying side.
- Every unbounded accumulator has an explicit cap: retained block output, live-block patch count, scrollback rows, the mux client outbox, and the APC payload buffer. See the resource budgets in [`docs/architecture.md`](docs/architecture.md).
- `cargo audit` runs in CI on every push and on a schedule.
