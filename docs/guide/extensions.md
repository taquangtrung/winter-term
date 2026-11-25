# Extensions

An extension adds commands, status bar segments, completion, pickers, and tabs to Winter. Most come as a single WebAssembly (`.wasm`) file, and each runs in a sandbox: it can reach only its own data directory and what you grant it, reaches the network only to make web requests of the sites you grant it, and is stopped if it misbehaves.

## Installing

Run `Extensions: Install From File…` in the command picker, or `winter extension install <file.wasm>`. Winter shows what the extension asks to reach and installs it only once you approve. Restart Winter for it to run.

Winter runs only the exact file you approved. A file changed since, or copied in by hand, is not run until you review it again.

## The Extensions page

`Extensions: Manage Extensions` lists each installed extension, whether it runs, and what it may reach. Extensions compiled into Winter are listed under Built in; they are always on.

| Key | Action |
|---|---|
| `j` / `k` | Next, previous row |
| `RET` or a double click | Do what the row offers: install, grant, or add a directory |
| `d` | Take a grant back |
| `D` | Remove the extension |

Each row also has a button for its action, and the selected permission, or one the pointer rests on, says what it allows. Changes take effect when Winter next starts.

## From a shell

| Command | What it does |
|---|---|
| `winter extension install <file.wasm>` | Review and install |
| `winter extension list` | List installed extensions and what they may reach |
| `winter extension permissions <id>` | Show one's grants |
| `winter extension grant <id> read\|write\|run\|net <what>` | Let it read or write a directory, run a program, or make web requests of a site |
| `winter extension revoke <id> read\|write\|run\|net <what>` | Take a grant back |
| `winter extension remove <id>` | Uninstall it |

## Shipped with Winter

Winter ships Copilot, [Docker](docker.md), and three AI quota extensions: `claude`, `antigravity`, and `zai`, each a status bar segment (see [the status bar](basics.md#the-status-bar)). Docker and the quotas run only while their tab or segment is shown.

## Settings

An extension's settings go in a block of `settings.kdl` named for its id, such as `copilot { enable #false }`, and show on the Settings page.

## Writing one

Start from `crates/ext/winter-simple-ext`, a working template. The crate can be built into Winter or as a `.wasm` file:

```bash
cargo rustc --release --target wasm32-wasip2 --crate-type cdylib --features wasm
```

Everything it adds is named under its id: a command it adds as `word_count` is `<id>.word_count` in `keybindings.kdl`. To name them under another word, give the package `namespace="<word>"`; no other extension may have the same one. Its tests run its commands against `winter_ext_api::testing::Recorder`, which keeps what it asks of Winter.

Its `package.kdl` says what it is, what it asks to reach, and what its commands are called:

```kdl
package id="<id>" name="<Name>" version="0.1.0" {
    description "What it does, shown when it is installed"

    permissions {
        read "~/Notes" "~/Documents"
        write "~/Notes/Data"
        net "api.example.com"
    }

    commands {
        "<id>.word_count" title="Count Words in File"
    }
}
```

A segment, a view's tabs, a command, and a picker row can each have an icon (`winter_ext_api::Icon`): the icon Winter draws for a file of some name (`Icon::File("Dockerfile")`), one of Winter's bundled icons by name, or the extension's own SVG. Only two bundled sets keep their names from one Winter to the next, so only they may be named: `symbols/<codicon>` (`symbols/rocket`) and `apps/<tool>` (`apps/claude`). Any other name draws Winter's fallback icon.

An extension built against version 0.1 of the API runs on every later Winter speaking 0.1; [extension-api.md](../reference/extension-api.md) says what is promised. The [architecture notes](../internals/architecture.md#extensions) describe how extensions run.
