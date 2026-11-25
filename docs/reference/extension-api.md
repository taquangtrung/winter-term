# Extension API

This document is the promise Winter makes to extension authors, and the rules a change to the extension API follows to keep it. It covers four things released together: the `winter-ext-api` crate (an extension's Rust API), the `winter-ext-guest` crate (what builds one as a WebAssembly component), the WIT package `winter:extension` in `crates/sdk/winter-ext-api/wit/winter.wit` (the contract a component is held to), and the `package.kdl` format. The crates are at 0.1.0, the WIT at `winter:extension@0.1.0`, and the package format at `api=1`.

Versions follow semver, which names a release line by its first number that is not 0: 0.1.x is one line, 0.2.x the next, and from 1.0, 1.x. Within a line the API only grows; a change that breaks starts the next line. [architecture.md](../internals/architecture.md#extensions) describes how extensions run.

## What is promised

- An extension built against any 0.1.x runs on every later 0.1.x Winter, unchanged. Compiled in, it builds against a later 0.1.x crate; as a component, the `.wasm` file built against 0.1.x loads as it is.
- A component built against a later version, or another line, is refused as it loads, with a notice saying which version it was built for, never run to fail later.
- A `package.kdl` written for a later format is refused by its `api` number, saying to update Winter.

Two things are outside the promise:

- Items hidden from the crate's docs (`#[doc(hidden)]`) are what Winter and `winter-ext-guest` read back out of a `Registry`. An extension never needs them, and they change as Winter does.
- `Host` is Winter's to implement. A method added within a line comes with a default body, so other implementations keep building, but one that relies on a method's default doing something will not. An extension's tests use `winter_ext_api::testing::Recorder`, which is kept up with `Host`.

## The contract

These hold for every extension, compiled in or a component:

- **One call at a time, and quick.** Everything an extension registers runs on Winter's thread, between one key and the next. A component's call on that thread is stopped after a second, and its slow work (`spawn`) after a minute; either stops only that extension, named in a notice. A component runs one call at a time, its slow work included.
- **What it asks is done after.** What an extension asks of the `Host` is done once Winter's call into it returns. An answer comes back later as an event, under the tag the extension chose: `Event::Fetched` for `fetch`, `Event::Ran` for `run`, `ServerSaid::Answer` for `server_request`.
- **A refusal is an answer.** A request the extension's grants do not allow comes back as `NotAllowed` (`FetchError`, `RunError`) where it has an answer, and as a notice to the user where it has none: a file it may not watch, a program it may not open a terminal on.
- **Lazy.** Its timers ring, and its files are watched, only while one of its status bar segments or views is shown. Shown again, it hears `Event::Shown`, an overdue timer rings, and each watched file is heard of once.
- **Ids.** An extension's id is lower case, with no spaces and no `.`. Everything it adds is filed under its namespace, which is its id unless its `package.kdl` names another with `namespace="…"`: the command it names `greet` is `<namespace>.greet` to a key binding, to `Host::run_command`, and to its `package.kdl`, and its views, segments, completion back-ends, and inline sources are named the same way. A namespace is one word, with no `.`, and no two extensions share one: Winter refuses to install, or to load, an extension whose namespace another already has.
- **Views.** A view is drawn lines, or a page of HTML, and declares the keys it answers (`ViewKey`); it never sees a key itself. A component's view is a WIT resource, dropped as its tab closes.
- **Values grow.** An enum marked `#[non_exhaustive]` (`Event`, `Icon`, `Tone`, `Severity`, `FetchError`, `RunError`, and the rest) may gain a case within a line: match it with a `_` arm.

## How it changes

Within a release line, the API only grows. Each change below keeps every extension of the line running:

- **A new host function** (an import of the component) is added to the `host` interface, marked `@since(version = 0.1.x)`, with a `Host` method that has a default body. wasmtime links a component's import of `winter:extension/host@0.1.0` against Winter's later 0.1.x by semver, so older components link as they did.
- **A new guest export** is added to the `guest` interface, marked `@since`. Winter looks it up by name as a component loads, outside the generated bindings, and asks only a component that has it, so one built before it existed loads as it did.
- **A new resource method** is added the same way, under `@since`, and called only on a component built against a version that has it.
- **A new type** is added freely: a record, variant, or enum no released function names.
- **A new case of a released variant or enum** cannot be sent to a component built before it, which would fail to read it. It is added to a new type, sent through a new function or export, so only components that know it hear it. The Rust enum gains the case at once, as it is `#[non_exhaustive]`.

What never changes within a line: a released record's fields, a variant's or enum's cases, a function's parameters or result, and a resource's existing methods. To give a record more fields, add a new record and a new function taking it (`set-status-v2`), and keep the old one.

An item to go is marked `@deprecated` in the WIT and `#[deprecated]` in Rust, and kept until the next line. A change that cannot be made by adding waits for the next line: a new WIT package `winter:extension@0.2.0`, new crates at 0.2.0, and a Winter that refuses 0.1.x components unless it keeps their bindings.

## What holds it

- `crates/libs/winter-wasm/tests/released.rs` loads the example extension as each release built it (`tests/released/example-<version>.wasm`), runs a command, sends an event, and draws a view. A release adds its build; none is ever rebuilt.
- `crates/libs/winter-wasm/tests/wit.rs` checks, with `wit-component`'s semver check, that a component built against each released WIT (`tests/released/winter-<version>.wit`) still links against the WIT now.
- The `semver` CI job runs `cargo semver-checks` on both crates against the pull request's base, so a Rust API break that stays in its line fails.

## Releasing

1. Raise the crates' versions in their `Cargo.toml` files and in the workspace's dependencies: the patch for an addition, and the next line (0.1 to 0.2; from 1.0, the major) for a break.
2. Raise the WIT package's version in `winter.wit`, and `WIT_VERSION` in `crates/libs/winter-wasm/src/lib.rs` with it.
3. Build the example: `cargo rustc -p winter-simple-ext --features wasm --release --target wasm32-wasip2 --crate-type cdylib`. Copy `target/wasm32-wasip2/release/winter_simple_ext.wasm` to `crates/libs/winter-wasm/tests/released/example-<version>.wasm`, and list it in `RELEASED` in `tests/released.rs`.
4. Copy `winter.wit` to `crates/libs/winter-wasm/tests/released/winter-<version>.wit`, and list it in `RELEASED` in `tests/wit.rs`.

## The package format

A component carries its `package.kdl`, embedded by `winter_ext_guest::export!` as it is built:

```kdl
package id="example" name="Example Extension" version="0.1.0" api=1 {
    description "Counts words and flags TODO lines as problems."

    permissions {
        read "~/Notes"
        write "~/Notes/Data"
        run "some-language-server"
        net "api.example.com"
    }

    commands {
        "example.word_count" title="Example: Count Words in File"
    }
}
```

`api` is the version of this format, 1 where it is left out. Winter reads only the fields it knows and refuses a package naming any other, so a field added later comes with a new `api`, and a Winter too old for it says so by number. Each command is named by its full id, under the package's namespace: its id, unless `namespace` names another, so an extension can keep the command names its users already bind when its id changes.
