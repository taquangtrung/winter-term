//! `Winter`: a terminal-first IDE.
//!
//! **Window mode** (no args): opens a GPU-accelerated terminal window (winit +
//! wgpu + cosmic-text) with an interactive shell.
//!
//! **Headless demo** (with args): runs the given command under a PTY and prints
//! the live cell grid and parsed block list.

// `deny` rather than `forbid` so `attach_parent_console` alone can opt out.
#![deny(unsafe_code)]
// No-op on non-Windows targets. On Windows it links the GUI subsystem instead
// of the default console subsystem, so launching `winter.exe` without an
// inherited console (e.g. via ShellExecute, as RightKeys does) no longer
// spawns a visible conhost.exe window alongside the actual terminal window.
#![windows_subsystem = "windows"]

use std::env;
use std::process::ExitCode;

use portable_pty::CommandBuilder;
use winter_app::control::WorkspaceCommand;
use winter_app::mux::transport::MuxTransport;
use winter_app::terminal::headless::run_to_completion;
use winter_core::Screen;

// ============================================================================
// Constants
// ============================================================================

/// Name printed by `--help` and `--version`. The crate is `winter` and the
/// distro package is `winter-term`; the user-facing name is neither.
const PROGRAM_NAME: &str = "winter";

/// The release version, suffixed with the commit id (`0.1.0+7744bde`) when
/// built from a commit that is not a release tag. Stamped by `build.rs`.
const VERSION: &str = env!("WINTER_BUILD_VERSION");

/// The exit status for a command line that does not parse, as `sysexits.h`
/// and most command-line tools use it.
const USAGE_ERROR: u8 = 2;

// ============================================================================
// Entry point
// ============================================================================

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();

    if args.is_empty() {
        return run_window();
    }
    #[cfg(windows)]
    attach_parent_console();
    // `--help`/`--version` are matched before the headless fallthrough, which
    // would otherwise hand them to a PTY and try to execute `--version` as a
    // command. Packagers and `command -v`-style probes expect both.
    match args[0].as_str() {
        "-h" | "--help" | "help" => {
            println!("{}", help_text());
            ExitCode::SUCCESS
        }
        "-V" | "--version" | "version" => {
            println!("{PROGRAM_NAME} {VERSION}");
            ExitCode::SUCCESS
        }
        "mux" => run_mux(&args[1..]),
        "--reload" => run_reload(),
        "--edit-wait" => run_edit_wait(&args[1..]),
        "workspace" => run_workspace(&args[1..]),
        // A flag nothing above knows is a mistake, not a program to run:
        // `winter --relaod` started a shell to run `--relaod` and dumped it.
        flag if flag.starts_with('-') => {
            eprintln!("{PROGRAM_NAME}: unknown option {flag}\n\n{}", help_text());
            ExitCode::from(USAGE_ERROR)
        }
        _ => run_headless(&args),
    }
}

/// Borrow the console of the shell that launched us, so command-line output
/// reaches the terminal it was typed in.
///
/// The GUI subsystem (see `windows_subsystem` above) starts the process with
/// no console, and anything printed is dropped. Standard handles that are
/// already set, a pipe or a redirect, are left alone. Launched from Explorer
/// there is no parent console to attach to, and nothing is lost by failing.
#[cfg(windows)]
#[allow(unsafe_code)]
fn attach_parent_console() {
    use windows_sys::Win32::System::Console::{
        AttachConsole, GetStdHandle, ATTACH_PARENT_PROCESS, STD_OUTPUT_HANDLE,
    };

    // SAFETY: both calls take plain integer arguments and touch no memory of
    // ours; a null handle means "no stdout", and a failed attach is ignored.
    unsafe {
        if GetStdHandle(STD_OUTPUT_HANDLE).is_null() {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

/// Usage text for `winter --help`.
///
/// Written out rather than derived from an argument parser: the CLI is four
/// verbs wide and pulling in a parser crate to print one page would be the
/// larger cost.
fn help_text() -> String {
    format!(
        "\
{PROGRAM_NAME} {VERSION}: a terminal-first IDE

USAGE:
    winter                        Open a terminal window
    winter <command> [args...]    Run a command headless and dump the
                                  parsed grid and block list
    winter mux <subcommand>       Manage multiplexer sessions
    winter workspace <subcommand> Manage the workspaces of the window it
                                  is run in (or the first one launched)
    winter --reload               Reload config in a running instance
    winter --edit-wait [--socket <path>] <file>
                                  Edit a file in a running instance and wait
                                  until it is closed (set as GIT_EDITOR in
                                  Winter's own panes, aimed at their window)

OPTIONS:
    -h, --help                    Print this help
    -V, --version                 Print the version

MUX SUBCOMMANDS:
    serve                         Run the session server
    new <name> [-w dir] [-c cmd]  Create a session
    attach [name] [--host H]      Attach to a local or remote session
    list                          List sessions with geometry and uptime
    kill <name>                   Terminate a session
    proxy <name>                  Bridge stdio to the socket (used over ssh)

WORKSPACE SUBCOMMANDS:
    add [dir] [--name N]          Make dir's project a workspace (the
                                  current directory's by default) and put
                                  the window in it
    add-folder [dir]              Add dir's project to the window's workspace
    switch <name>                 Switch the window to a workspace
    rename <name>                 Rename the window's workspace
    remove <name>                 Forget a workspace (its folders stay)
    list                          List workspaces, the window's marked *

ENVIRONMENT:
    WINTER_CWD                    Directory for the initial pane
    WINTER_SOCKET                 The control socket of the window a pane
                                  is in (set in Winter's own panes)
    WINTER_SIDECHANNEL_DIR        Directory TBP file-referenced payloads
                                  are read from

Config lives in ~/.config/winter-term/ (settings.kdl, keybindings.kdl).
Docs: {repository}",
        repository = env!("CARGO_PKG_REPOSITORY"),
    )
}

// ============================================================================
// Window mode
// ============================================================================

fn run_window() -> ExitCode {
    use winit::event_loop::EventLoop;

    let event_loop = match EventLoop::new() {
        Ok(el) => el,
        Err(error) => {
            eprintln!("winter: {error}");
            return ExitCode::FAILURE;
        }
    };

    let mut app = winter_app::window::App::new();
    app.connect_event_loop(&event_loop);
    match event_loop.run_app(&mut app) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("winter: {error}");
            ExitCode::FAILURE
        }
    }
}

// ============================================================================
// Workspaces
// ============================================================================

/// `winter workspace <subcommand>`: have the window this runs in (see
/// [`winter_app::control::reachable_socket_path`]) act on its workspaces, and
/// print what it answers.
fn run_workspace(args: &[String]) -> ExitCode {
    let command = match parse_workspace_args(args) {
        Ok(command) => command,
        Err(usage) => {
            eprintln!("winter: {usage}");
            return ExitCode::FAILURE;
        }
    };
    let path = winter_app::control::reachable_socket_path();
    match winter_app::control::request_workspace(&path, command) {
        Ok(answer) if answer.ok => {
            println!("{}", answer.text);
            ExitCode::SUCCESS
        }
        Ok(answer) => {
            eprintln!("winter: {}", answer.text);
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("winter: no running window to ask: {error}");
            ExitCode::FAILURE
        }
    }
}

/// The workspace command `args` spell, or what is wrong with them.
fn parse_workspace_args(args: &[String]) -> Result<WorkspaceCommand, String> {
    let here = || std::path::PathBuf::from(".");
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["add", rest @ ..] => {
            let (dir, name) = match rest {
                [] => (here(), None),
                ["--name", name] => (here(), Some(name.to_string())),
                [dir] => (dir.into(), None),
                [dir, "--name", name] | ["--name", name, dir] => {
                    (dir.into(), Some(name.to_string()))
                }
                _ => return Err("usage: winter workspace add [dir] [--name N]".to_string()),
            };
            Ok(WorkspaceCommand::Add { dir, name })
        }
        ["add-folder"] => Ok(WorkspaceCommand::AddFolder { dir: here() }),
        ["add-folder", dir] => Ok(WorkspaceCommand::AddFolder { dir: dir.into() }),
        ["list"] => Ok(WorkspaceCommand::List),
        ["remove", name] => Ok(WorkspaceCommand::Remove {
            name: name.to_string(),
        }),
        ["rename", name] => Ok(WorkspaceCommand::Rename {
            name: name.to_string(),
        }),
        ["switch", name] => Ok(WorkspaceCommand::Switch {
            name: name.to_string(),
        }),
        _ => Err(
            "usage: winter workspace add [dir] [--name N] | add-folder [dir] | \
             switch <name> | rename <name> | remove <name> | list"
                .to_string(),
        ),
    }
}

// ============================================================================
// Reload a running instance
// ============================================================================

/// Ask an already-running windowed instance (over the control socket) to
/// save its session, relaunch itself, and exit. Used by install scripts that
/// need to replace the running binary out from under an open window.
fn run_reload() -> ExitCode {
    let path = winter_app::control::socket_path();
    match winter_app::control::request_reload(&path) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("winter: no running instance to reload: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Stand in for `$EDITOR` under a program that blocks on it, such as
/// `git commit`: have the running instance edit `file` and return once that
/// editor closes, failing when the edit was discarded. The instance is the one
/// listening on `--socket <path>`, or the first launched without it. With no
/// instance to ask, run the user's own editor so the program is never left
/// without one.
fn run_edit_wait(args: &[String]) -> ExitCode {
    let (path, args) = match args {
        [flag, path, rest @ ..] if flag == "--socket" => (path.clone(), rest),
        _ => (winter_app::control::socket_path(), args),
    };
    let Some(file) = args.first() else {
        eprintln!("winter: --edit-wait needs a file");
        return ExitCode::FAILURE;
    };
    match winter_app::control::request_edit(&path, file.into()) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(_) => run_user_editor(file),
    }
}

/// Edit `file` in `$VISUAL` / `$EDITOR` (falling back to `vi`) and report how
/// it exited.
fn run_user_editor(file: &str) -> ExitCode {
    let editor = env::var("VISUAL")
        .or_else(|_| env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string());
    match std::process::Command::new(&editor).arg(file).status() {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("winter: could not run {editor}: {error}");
            ExitCode::FAILURE
        }
    }
}

// ============================================================================
// Headless demo
// ============================================================================

fn run_headless(args: &[String]) -> ExitCode {
    let mut command = CommandBuilder::new(&args[0]);
    for arg in &args[1..] {
        command.arg(arg);
    }

    let terminal = match run_to_completion(command) {
        Ok(t) => t,
        Err(error) => {
            eprintln!("winter: {error}");
            return ExitCode::FAILURE;
        }
    };

    println!("=== Blocks ===");
    for (index, block) in terminal.scrollback().blocks().iter().enumerate() {
        println!("--- block {index} ---");
        println!("{block:#?}");
    }

    println!("\n=== Screen Grid ===");
    let mut screen = Screen::new(80, 24);
    screen.feed(terminal.scrollback().plain_text().as_bytes());
    print_grid(screen.grid());

    ExitCode::SUCCESS
}

fn print_grid(grid: &winter_core::Grid) {
    for row in 0..grid.rows() {
        let mut line = String::with_capacity(grid.cols());
        for col in 0..grid.cols() {
            let ch = grid.cell(row, col).map(|c| c.ch).unwrap_or(' ');
            line.push(ch);
        }
        println!("{}", line.trim_end());
    }
}

// ============================================================================
// Mux subcommands
// ============================================================================

/// How long a mux subcommand waits for the server to answer: a session
/// confirmation, a listing, or the replay an attach starts with.
const MUX_REPLY_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);

/// Idle poll step while waiting for the first buffered message.
const MUX_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);

/// Pause after each line typed into `winter mux attach`, so the output it
/// causes is printed before the next line is read.
const MUX_ECHO_WAIT: std::time::Duration = std::time::Duration::from_millis(50);

/// The session a mux subcommand acts on when none is named.
const DEFAULT_SESSION: &str = "default";

/// Geometry `winter mux new` creates a session with.
const NEW_SESSION_COLS: u16 = 80;
const NEW_SESSION_ROWS: u16 = 24;

const MUX_USAGE: &str = "usage: winter mux <serve|new|attach [--host H]|list|kill|proxy> [args]";

fn run_mux(args: &[String]) -> ExitCode {
    let Some(subcommand) = args.first() else {
        eprintln!("{MUX_USAGE}");
        return ExitCode::FAILURE;
    };
    let rest = &args[1..];
    let result = match subcommand.as_str() {
        "serve" => run_mux_serve(),
        "list" => run_mux_list(),
        "new" => run_mux_new(rest),
        "kill" => run_mux_kill(rest),
        "attach" => run_mux_attach(rest),
        "proxy" => run_mux_proxy(rest),
        _ => {
            eprintln!("{MUX_USAGE}");
            return ExitCode::FAILURE;
        }
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("winter mux: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `winter mux serve`: run the session server in the foreground.
fn run_mux_serve() -> anyhow::Result<ExitCode> {
    let path = winter_app::mux::server::default_socket_path();
    eprintln!("winter mux: listening on {path}");
    winter_app::mux::server::MuxServer::new(&path).run()?;
    Ok(ExitCode::SUCCESS)
}

/// `winter mux list`: print every session with its geometry and uptime.
fn run_mux_list() -> anyhow::Result<ExitCode> {
    let sessions = connect_local()?.query_sessions(MUX_REPLY_TIMEOUT)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    for s in &sessions {
        let uptime = winter_app::mux::protocol::format_uptime(s.created, now);
        println!(
            "{} ({}x{}, up {uptime}, {} attached) - {}",
            s.name, s.cols, s.rows, s.attach_count, s.command
        );
    }
    if sessions.is_empty() {
        println!("(no sessions)");
    }
    Ok(ExitCode::SUCCESS)
}

/// `winter mux new <name> [-w <dir>] [-c <command>]`: create a session on
/// the server running `command` (or the default shell) in `dir`. The
/// command runs through the user's shell, so it can carry arguments,
/// pipes, and environment expansions.
fn run_mux_new(args: &[String]) -> anyhow::Result<ExitCode> {
    let mut name: Option<&str> = None;
    let mut cwd: Option<&str> = None;
    let mut command: Option<&str> = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            flag @ ("-w" | "-c") => {
                let Some(value) = args.get(index + 1) else {
                    anyhow::bail!("new: {flag} requires a value");
                };
                if flag == "-w" {
                    cwd = Some(value);
                } else {
                    command = Some(value);
                }
                index += 2;
            }
            other if name.is_none() => {
                name = Some(other);
                index += 1;
            }
            other => anyhow::bail!("new: unexpected argument: {other}"),
        }
    }
    let name = session_name(name)?;
    // The server answers on its own schedule; wait for the confirmation
    // with a deadline rather than a single nonblocking read.
    let (cols, rows) = connect_local()?
        .spawn_confirmed(
            name,
            NEW_SESSION_COLS,
            NEW_SESSION_ROWS,
            cwd,
            command,
            MUX_REPLY_TIMEOUT,
        )
        .map_err(anyhow::Error::msg)?;
    let command = command.unwrap_or("(default shell)");
    println!("started session '{name}' ({cols}x{rows}): {command}");
    Ok(ExitCode::SUCCESS)
}

/// `winter mux kill [name]`: end a session and the program it runs.
fn run_mux_kill(args: &[String]) -> anyhow::Result<ExitCode> {
    let name = session_name(args.first().map(String::as_str))?;
    let mut client = connect_local()?;
    let sessions = client.query_sessions(MUX_REPLY_TIMEOUT)?;
    if !sessions.iter().any(|s| s.name == name) {
        anyhow::bail!("no session named '{name}'");
    }
    client.kill(name)?;
    // Wait for the server to act on it (its reply to the next request comes
    // after), so the kill is not lost when this process exits at once.
    client.query_sessions(MUX_REPLY_TIMEOUT)?;
    println!("killed session: {name}");
    Ok(ExitCode::SUCCESS)
}

/// `winter mux attach [--host <host>] [session]`: attach to a local
/// session (default), or one on a remote mux server reached over ssh at
/// `--host`, and stream it until the session ends or stdin does.
fn run_mux_attach(args: &[String]) -> anyhow::Result<ExitCode> {
    let mut host: Option<&str> = None;
    let mut name: Option<&str> = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "-h" | "--host" => {
                let Some(value) = args.get(index + 1) else {
                    anyhow::bail!("attach: --host requires a hostname");
                };
                host = Some(value);
                index += 2;
            }
            other if name.is_none() => {
                name = Some(other);
                index += 1;
            }
            other => anyhow::bail!("attach: unexpected argument: {other}"),
        }
    }
    let name = session_name(name)?;
    match host {
        Some(host) => {
            // The proxy on the far side attaches on this client's behalf.
            let mut client = winter_app::mux::remote::RemoteClient::connect(host, name, true)
                .map_err(|e| anyhow::anyhow!("could not reach '{host}' over ssh: {e}"))?;
            eprintln!("attached to '{host}:{name}'");
            stream_session(&mut client, name)
        }
        None => {
            let mut client = connect_local()?;
            client.attach(name, true)?;
            eprintln!("attached to session: {name}");
            stream_session(&mut client, name)
        }
    }
}

/// `winter mux proxy [name] [--no-replay]`: bridge stdio to the local
/// server's socket, for a client on the other end of an ssh connection.
fn run_mux_proxy(args: &[String]) -> anyhow::Result<ExitCode> {
    let replay = !args
        .iter()
        .any(|arg| arg == winter_app::mux::remote::NO_REPLAY_FLAG);
    let name = args
        .iter()
        .map(String::as_str)
        .find(|arg| *arg != winter_app::mux::remote::NO_REPLAY_FLAG);
    winter_app::mux::proxy::run(session_name(name)?, replay)?;
    Ok(ExitCode::SUCCESS)
}

/// The session a subcommand names, or the default one, refused when it is
/// not a valid name.
fn session_name(name: Option<&str>) -> anyhow::Result<&str> {
    let name = name.unwrap_or(DEFAULT_SESSION);
    winter_app::mux::protocol::validate_session_name(name).map_err(anyhow::Error::msg)?;
    Ok(name)
}

/// A connection to the local mux server, or an error saying how to start one.
fn connect_local() -> anyhow::Result<winter_app::mux::client::MuxClient> {
    let path = winter_app::mux::server::default_socket_path();
    winter_app::mux::client::MuxClient::connect(&path).map_err(|e| {
        anyhow::anyhow!(
            "cannot connect to the server at {path}: {e}; start one with 'winter mux serve'"
        )
    })
}

/// Print `session`'s output and send it each line read from stdin, until
/// the session ends, the connection does, or stdin is exhausted (then
/// detach, leaving the session running).
fn stream_session(client: &mut impl MuxTransport, session: &str) -> anyhow::Result<ExitCode> {
    use std::io::BufRead;

    let mut stdout = std::io::stdout();
    // Print the scrollback the server replays on attach before waiting on
    // stdin, or it stays invisible until the first keystroke. The reply
    // arrives asynchronously, so the drain waits for it rather than
    // returning on the first idle non-blocking read.
    if drain_mux_messages(client, &mut stdout, MUX_REPLY_TIMEOUT)? {
        return Ok(ExitCode::SUCCESS);
    }
    for line in std::io::stdin().lock().lines() {
        let mut bytes = line?.into_bytes();
        bytes.push(b'\n');
        client.send_input(session, &bytes)?;
        std::thread::sleep(MUX_ECHO_WAIT);
        if drain_mux_messages(client, &mut stdout, std::time::Duration::ZERO)? {
            return Ok(ExitCode::SUCCESS);
        }
    }
    let _ = client.send(&winter_app::mux::protocol::ClientMessage::Detach);
    Ok(ExitCode::SUCCESS)
}

/// Write every buffered server message to stdout: live output and replayed
/// scrollback alike, until the connection goes idle. When nothing has
/// arrived yet, keeps polling up to `first_wait` so an attach's reply is
/// not missed. Returns `true` when the session has ended (or the connection
/// has) and the caller should stop.
fn drain_mux_messages(
    client: &mut impl MuxTransport,
    stdout: &mut std::io::Stdout,
    first_wait: std::time::Duration,
) -> anyhow::Result<bool> {
    use std::io::Write;
    use winter_app::mux::protocol::ServerMessage;

    let deadline = std::time::Instant::now() + first_wait;
    loop {
        let mut seen_any = false;
        while let Some(msg) = client.recv()? {
            seen_any = true;
            match msg {
                ServerMessage::Output { bytes, .. } | ServerMessage::Scrollback { bytes, .. } => {
                    stdout.write_all(&bytes)?;
                    stdout.flush()?;
                }
                ServerMessage::Exit { code, .. } => {
                    eprintln!("session exited (code: {code:?})");
                    return Ok(true);
                }
                ServerMessage::Error { message } => eprintln!("winter mux: {message}"),
                ServerMessage::Attached { .. }
                | ServerMessage::Hello { .. }
                | ServerMessage::Resized { .. }
                | ServerMessage::SessionList { .. } => {}
            }
        }
        if client.eof() {
            eprintln!("winter mux: the connection closed");
            return Ok(true);
        }
        if seen_any || std::time::Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(MUX_POLL_INTERVAL);
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(words: &[&str]) -> Result<WorkspaceCommand, String> {
        let args: Vec<String> = words.iter().map(|w| w.to_string()).collect();
        parse_workspace_args(&args)
    }

    #[test]
    fn test_workspace_add_takes_a_dir_and_a_name_in_either_order() {
        let add = |dir: &str, name: Option<&str>| WorkspaceCommand::Add {
            dir: dir.into(),
            name: name.map(str::to_string),
        };
        assert_eq!(parse(&["add"]), Ok(add(".", None)));
        assert_eq!(parse(&["add", "src"]), Ok(add("src", None)));
        assert_eq!(parse(&["add", "--name", "w"]), Ok(add(".", Some("w"))));
        assert_eq!(
            parse(&["add", "src", "--name", "w"]),
            Ok(add("src", Some("w")))
        );
        assert_eq!(
            parse(&["add", "--name", "w", "src"]),
            Ok(add("src", Some("w")))
        );
        assert!(parse(&["add", "a", "b"]).is_err());
    }

    #[test]
    fn test_workspace_commands_needing_a_name_refuse_none() {
        for verb in ["switch", "rename", "remove"] {
            assert!(parse(&[verb]).is_err(), "{verb}");
        }
        assert!(parse(&[]).is_err());
        assert!(parse(&["frobnicate"]).is_err());
    }
}
