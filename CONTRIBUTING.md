# Contributing to Winter

Thanks for taking a look. Winter is early software, so bug reports from real use are worth more than almost anything else right now.

## Building from source

- **Rust** stable, 1.96 or newer. This is the floor CI enforces, and the dependency graph alone requires 1.95.
- **uv**, for the Python client tests. Optional.

```bash
make build        # build all Rust crates
make rust-test    # the Rust test suite
make test         # everything: Rust and Python
make lint         # clippy (deny warnings) and rustfmt check
make help         # all targets
```

`make package` produces the right installer for whatever OS you are on, and `make install` installs it. Use `make install` rather than `cargo install` from a checkout: on Linux it also removes any `winter` that `cargo install` put in `~/.cargo/bin`, so only one copy is on your PATH.

## Trying the pipeline without a display

The `winter` binary runs a command under a PTY and prints both views of its output: the live screen grid and the parsed block list, both from `winter-emulator`.

```bash
make demo CMD='ls -la'
# or directly:
cargo run -p winter-term -- bash -c 'echo hi; printf "\033[1;32mgreen\033[0m\n"'
```

To watch a rich block go through the parser, let the command emit one itself so it rides the PTY the example reads. `WINTER=1` enables emission:

```bash
# Shell client, emitting an SVG block:
printf '<svg width=10/>' > /tmp/plot.svg
cargo run -p winter-term --example dump_session -- \
  bash -c "WINTER=1 $PWD/clients/client.sh svg /tmp/plot.svg"

# Python client, the same:
cargo run -p winter-term --example dump_session -- \
  bash -c "WINTER=1 PYTHONPATH=$PWD/clients/client-py/src python3 -c \
    'import winter; winter.display_svg(\"<svg width=10/>\", text=\"fallback\")'"
```

The SVG appears as a `Content` block with its MIME bundle and a `text/plain` fallback. Without `WINTER=1` the clients print the plain-text fallback instead, which is the safe degradation path.

To record a demo GIF or screenshot of the real GUI, see [`scripts/demo/README.md`](scripts/demo/README.md).

## Before you open a pull request

Run the same checks CI runs:

```bash
make lint    # rustfmt and clippy
make test    # the workspace test suite
```

Both must be clean. Clippy runs with warnings denied, so a warning fails the build.

Add a note to [`CHANGELOG.md`](CHANGELOG.md) under an `## [Unreleased]` heading for anything a user would notice. Internal refactors do not need one.

## How CI is set up

CI runs on pull requests only, never on a push to `main`. The Linux jobs (clippy, tests, MSRV, the Python client, `cargo audit`, and a crates.io publish dry run) run on every pull request. The Windows and macOS test jobs run only when the workflow is started by hand (`gh workflow run CI`), because a macOS runner bills at ten times a Linux one. **If you touch anything platform-specific, start a manual run.** The source is not rustfmt-formatted, and CI does not check formatting.

Doc-only changes skip CI entirely.

## What the project cares about

- **Every byte from a PTY is attacker-controlled.** Parsers reachable from terminal output are fuzzed for a reason. If you touch the VT parser, the OSC 133 state machine, or the TBP codec, think about what a crafted stream does to your change. See [`SECURITY.md`](SECURITY.md).
- **Tests should be able to fail.** A test that passes no matter what the code does is worse than no test. Name it after the behavior it pins, not the function it calls.
- **Comments explain why, not what.** The most valuable comments in this codebase record a debugging session: which version of which dependency behaved how, and what broke before the current approach.
- `#![forbid(unsafe_code)]` is set in the protocol, core, render, spell, and client crates and should stay that way. The app crate denies `unsafe` and allows it only around a few system calls, each with a `SAFETY` comment saying why it holds.

## Adding a key

Every key a surface answers is declared in a key table, so it is listed in the Keys editor, rebindable in `keybindings.kdl`, and shown in the help, all from one place. To add one:

1. Add a `Binding` to the surface's `KeyTable` (`winter_model::input::table`): the action, an `id` unique within the table (`toggle_archive`, the name users rebind it by), the default keys in `keybindings.kdl` notation, and the label the help shows.
2. A new table gets a `block` named after the editor and its part (`dir`, `dir.edit`) and goes in the file's `KEY_TABLES` list, which `editors::KEY_TABLE_GROUPS` reads.
3. Dispatch through `table::find` or `table::lookup` (for sequences), and build menus with `EditorMenuItem::bound`, never by matching `KeyCode` and `Mods` by hand.

`test_every_key_is_declared_in_a_key_table` fails on a key matched as written outside a table, and `test_every_key_table_is_listed` on a table no list names. Text being typed, such as a hex digit while editing, is no command: mark the line, or the one above it, `// key-table:` with the reason. The Vim and Emacs engine and the key plumbing are exempt for good (`KEY_ENGINE_FILES`); the editors' keys not yet moved onto tables are listed in `KEY_TABLE_DEBT`, which may only shrink.

## Getting oriented

- [`docs/architecture.md`](docs/architecture.md): what each crate owns and how bytes become pixels.
- [`docs/usage-guide.md`](docs/usage-guide.md): the modes, the keymap, and every settings key.
- [`docs/terminal-block-protocol-spec.md`](docs/terminal-block-protocol-spec.md): the wire protocol.

## Reporting bugs

Open an issue with the version, your platform, and the smallest reproduction you can manage. For rendering or parsing bugs, the byte stream that triggers it is the most useful thing you can include.

Security issues go through the private channel in [`SECURITY.md`](SECURITY.md) instead, not a public issue.

## Releasing

Maintainer task, documented in [`RELEASING.md`](RELEASING.md).

## Licensing

Winter is MIT. Unless you state otherwise, any contribution you submit for inclusion is licensed the same way, without additional terms.
