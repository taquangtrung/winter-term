# Extensions

An extension adds commands, status bar segments, completion, pickers, and tabs to Winter. Most come as a single WebAssembly (`.wasm`) file, and each runs in a sandbox: it can reach only its own data directory and what you grant it, has no network access, and is stopped if it misbehaves.

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
| `winter extension grant <id> read\|write\|run <what>` | Let it read or write a directory, or run a program as its server |
| `winter extension revoke <id> read\|write\|run <what>` | Take a grant back |
| `winter extension remove <id>` | Uninstall it |

## Settings

An extension's settings go in a block of `settings.kdl` named for its id, such as `copilot { enable #false }`, and show on the Settings page.

## Writing one

Start from `crates/ext/winter-simple-ext`, a working template. The crate can be built into Winter or as a `.wasm` file:

```bash
cargo rustc --release --target wasm32-wasip2 --crate-type cdylib --features wasm
```

Its `package.kdl` says what it is, what it asks to reach, and what its commands are called:

```kdl
package id="<id>" name="<Name>" version="0.1.0" {
    description "What it does, shown when it is installed"

    permissions {
        read "~/Notes" "~/Documents"
        write "~/Notes/Data"
    }

    commands {
        "<id>.word_count" title="Count Words in File"
    }
}
```

The [architecture notes](../architecture.md#extensions) describe how extensions run.
